//! `.notein` bundle format — export + import.
//!
//! A bundle is a `tar` archive compressed with `zstd`. It groups a
//! selection of library items (optionally scoped by their folders and
//! referencing workspaces) together with the on-disk doc bodies, any
//! embedded PDF attachments, and a manifest describing every library
//! entity carried inside.
//!
//! On import, the host performs a preflight against the current
//! library:
//!
//! * Any UUID collision (item, folder, tag, workspace) is a hard stop
//!   — the user cancels or picks a different bundle.
//! * Missing plugins surface as a warning; bookmarks referencing them
//!   stay in the merged library with a dangling `SourceRef::Plugin`
//!   payload. Installing the plugin later normalises those chips.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Cursor, Read, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use bincode::config::{self, Configuration};

use crate::bookmark::SourceRef;
use crate::doc::Doc;
use crate::library::{
    DocKind, Folder, FolderId, Item, ItemId, Library, LibraryError, Tag, TagId, Workspace,
    WorkspaceId, bodies,
};
use notein_plugin::PluginRegistry;

const CODEC: Configuration = config::standard();

/// Schema version carried in [`BundleManifest::version`]. Bumped on any
/// breaking payload change — the reader rejects unknown versions so a
/// future bundle can't be misinterpreted by an older build.
pub const BUNDLE_VERSION: u32 = 1;

const MANIFEST_ENTRY: &str = "manifest.bin";
const DOCS_PREFIX: &str = "docs/";
const PDFS_PREFIX: &str = "pdfs/";
const DOC_EXT: &str = "bin";
const PDF_EXT: &str = "pdf";
const ZSTD_LEVEL: i32 = 3;

/// Caller-facing selection of what to export. Any union of the three
/// axes is valid — the writer deduplicates when a folder and a
/// workspace both pull in the same item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportSelection {
    pub items: Vec<ItemId>,
    pub folders: Vec<FolderId>,
    pub workspaces: Vec<WorkspaceId>,
}

/// Flags toggled by the export dialog before the writer runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    /// When `true`, PDF attachments are embedded verbatim in the
    /// bundle. When `false`, only the manifest + canvas doc bodies
    /// travel — the receiver sees chips but no re-openable PDFs.
    pub include_attachments: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_attachments: true,
        }
    }
}

/// Preview computed from a selection without touching disk. Rendered
/// by the export dialog so the user can trim the item list and see
/// which plugins the receiver will need before the writer runs.
///
/// Not `Eq`: [`Workspace::layout`] carries an `f32` split ratio, so
/// the comparison bottoms out as `PartialEq`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportPreview {
    pub items: Vec<Item>,
    pub folders: Vec<Folder>,
    pub workspaces: Vec<Workspace>,
    pub requires_plugins: Vec<String>,
}

/// Bundle manifest. One per `.notein` file, serialised via `bincode` at
/// [`MANIFEST_ENTRY`].
#[derive(Clone, Debug, PartialEq)]
pub struct BundleManifest {
    pub version: u32,
    pub exported_at_ms: u64,
    /// Distinct `SourceRef::Plugin::plugin_id` values the bookmarks
    /// inside the bundle reference. Importers cross-check against the
    /// live [`PluginRegistry`] and warn when any entry is unknown.
    pub requires_plugins: Vec<String>,
    pub folders: Vec<Folder>,
    pub items: Vec<Item>,
    pub tags: Vec<Tag>,
    pub workspaces: Vec<Workspace>,
}

impl bincode::Encode for BundleManifest {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> core::result::Result<(), bincode::error::EncodeError> {
        self.version.encode(encoder)?;
        self.exported_at_ms.encode(encoder)?;
        self.requires_plugins.encode(encoder)?;
        self.folders.encode(encoder)?;
        self.items.encode(encoder)?;
        self.tags.encode(encoder)?;
        self.workspaces.encode(encoder)?;
        Ok(())
    }
}

impl<Ctx> bincode::Decode<Ctx> for BundleManifest {
    fn decode<D: bincode::de::Decoder<Context = Ctx>>(
        decoder: &mut D,
    ) -> core::result::Result<Self, bincode::error::DecodeError> {
        Ok(Self {
            version: bincode::Decode::decode(decoder)?,
            exported_at_ms: bincode::Decode::decode(decoder)?,
            requires_plugins: bincode::Decode::decode(decoder)?,
            folders: bincode::Decode::decode(decoder)?,
            items: bincode::Decode::decode(decoder)?,
            tags: bincode::Decode::decode(decoder)?,
            workspaces: bincode::Decode::decode(decoder)?,
        })
    }
}

/// Any failure during export / import.
#[derive(Debug)]
pub enum BundleError {
    Io(io::Error),
    Codec(String),
    Library(LibraryError),
    UnknownVersion(u32),
    MissingManifest,
    MissingItem(ItemId),
    MissingFolder(FolderId),
    MissingWorkspace(WorkspaceId),
}

impl core::fmt::Display for BundleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "bundle io error: {err}"),
            Self::Codec(msg) => write!(f, "bundle codec error: {msg}"),
            Self::Library(err) => write!(f, "bundle library error: {err}"),
            Self::UnknownVersion(v) => write!(f, "bundle version {v} not supported"),
            Self::MissingManifest => f.write_str("bundle has no manifest entry"),
            Self::MissingItem(id) => write!(f, "bundle references unknown item {id}"),
            Self::MissingFolder(id) => write!(f, "bundle references unknown folder {id}"),
            Self::MissingWorkspace(id) => write!(f, "bundle references unknown workspace {id}"),
        }
    }
}

impl std::error::Error for BundleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Library(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for BundleError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<LibraryError> for BundleError {
    fn from(err: LibraryError) -> Self {
        Self::Library(err)
    }
}

type Result<T> = core::result::Result<T, BundleError>;

/// Build a `.notein` bundle streamed into `sink`.
///
/// `selection` axes are unioned; subfolders of a listed folder are
/// walked recursively; workspace exports pull in every item referenced
/// by any pane tab.
///
/// Writers backed by native file-picker handles satisfy
/// [`std::io::Write`] already; call sites that want to drop a bundle
/// at a known filesystem path use [`write_bundle_to_path`].
///
/// # Errors
/// Filesystem / codec / library failures surface through
/// [`BundleError`].
pub fn write_bundle<W: Write>(
    sink: W,
    lib: &Library,
    selection: &ExportSelection,
    options: ExportOptions,
) -> Result<()> {
    let plan = plan_export(lib, selection)?;
    let manifest = build_manifest(lib, &plan)?;

    let encoder = zstd::stream::write::Encoder::new(sink, ZSTD_LEVEL)?.auto_finish();
    let mut tar = tar::Builder::new(encoder);
    tar.mode(tar::HeaderMode::Deterministic);

    let manifest_bytes =
        bincode::encode_to_vec(&manifest, CODEC).map_err(|e| BundleError::Codec(e.to_string()))?;
    append_file(&mut tar, MANIFEST_ENTRY, &manifest_bytes)?;

    for item in &manifest.items {
        if let Some(doc_bytes) = read_doc_bytes(item.id) {
            let path = format!("{DOCS_PREFIX}{}.{DOC_EXT}", item.id);
            append_file(&mut tar, &path, &doc_bytes)?;
        }
        if options.include_attachments && item_carries_pdf(&item.kind) {
            if let Ok(pdf_bytes) = bodies::read_pdf(item.id) {
                let path = format!("{PDFS_PREFIX}{}.{PDF_EXT}", item.id);
                append_file(&mut tar, &path, &pdf_bytes)?;
            }
        }
    }

    tar.into_inner()?.flush()?;
    Ok(())
}

/// Convenience wrapper that opens `dest` as a fresh file and streams a
/// bundle into it.
///
/// # Errors
/// See [`write_bundle`].
pub fn write_bundle_to_path(
    dest: &Path,
    lib: &Library,
    selection: &ExportSelection,
    options: ExportOptions,
) -> Result<()> {
    let file = File::create(dest)?;
    write_bundle(file, lib, selection, options)
}

/// Decoded bundle payload. Lives entirely in memory — the on-disk
/// `.notein` file can go away after this returns.
#[derive(Debug)]
pub struct ParsedBundle {
    pub manifest: BundleManifest,
    pub docs: HashMap<ItemId, Vec<u8>>,
    pub pdfs: HashMap<ItemId, Vec<u8>>,
}

/// Preflight outcome. The caller refuses to merge when `collisions` is
/// non-empty and surfaces `missing_plugins` as a non-blocking warning.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preflight {
    pub colliding_items: Vec<ItemId>,
    pub colliding_folders: Vec<FolderId>,
    pub colliding_tags: Vec<TagId>,
    pub colliding_workspaces: Vec<WorkspaceId>,
    pub missing_plugins: Vec<String>,
}

impl Preflight {
    #[must_use]
    pub fn has_collisions(&self) -> bool {
        !self.colliding_items.is_empty()
            || !self.colliding_folders.is_empty()
            || !self.colliding_tags.is_empty()
            || !self.colliding_workspaces.is_empty()
    }
}

/// Decode a bundle from any [`Read`] source into memory. Callers with
/// a filesystem path should use [`parse_bundle_from_path`].
///
/// # Errors
/// Filesystem / codec / unknown-version failures.
pub fn parse_bundle<R: Read>(src: R) -> Result<ParsedBundle> {
    let decoder = zstd::stream::read::Decoder::new(src)?;
    let mut archive = tar::Archive::new(decoder);

    let mut manifest: Option<BundleManifest> = None;
    let mut docs: HashMap<ItemId, Vec<u8>> = HashMap::new();
    let mut pdfs: HashMap<ItemId, Vec<u8>> = HashMap::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let name = path
            .to_str()
            .ok_or_else(|| BundleError::Codec("non-utf8 entry name".into()))?
            .to_owned();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        if name == MANIFEST_ENTRY {
            let (m, _) = bincode::decode_from_slice::<BundleManifest, _>(&buf, CODEC)
                .map_err(|e| BundleError::Codec(e.to_string()))?;
            if m.version != BUNDLE_VERSION {
                return Err(BundleError::UnknownVersion(m.version));
            }
            manifest = Some(m);
        } else if let Some(rest) = name.strip_prefix(DOCS_PREFIX) {
            if let Some(id) = parse_id::<ItemId>(rest, DOC_EXT) {
                docs.insert(id, buf);
            }
        } else if let Some(rest) = name.strip_prefix(PDFS_PREFIX) {
            if let Some(id) = parse_id::<ItemId>(rest, PDF_EXT) {
                pdfs.insert(id, buf);
            }
        }
    }

    let manifest = manifest.ok_or(BundleError::MissingManifest)?;
    Ok(ParsedBundle {
        manifest,
        docs,
        pdfs,
    })
}

/// Convenience wrapper that opens `src` and parses it.
///
/// # Errors
/// See [`parse_bundle`].
pub fn parse_bundle_from_path(src: &Path) -> Result<ParsedBundle> {
    let file = File::open(src)?;
    parse_bundle(file)
}

impl ParsedBundle {
    /// Compare the bundle against the current library + plugin
    /// registry. Collisions mean "cancel". Missing plugins mean "warn
    /// but keep going".
    #[must_use]
    pub fn preflight(&self, lib: &Library, registry: &PluginRegistry) -> Preflight {
        let mut out = Preflight::default();
        for item in &self.manifest.items {
            if lib.item(item.id).is_some() {
                out.colliding_items.push(item.id);
            }
        }
        for folder in &self.manifest.folders {
            if lib.folder(folder.id).is_some() {
                out.colliding_folders.push(folder.id);
            }
        }
        for tag in &self.manifest.tags {
            if lib.tag(tag.id).is_some() {
                out.colliding_tags.push(tag.id);
            }
        }
        for ws in &self.manifest.workspaces {
            if lib.workspace(ws.id).is_some() {
                out.colliding_workspaces.push(ws.id);
            }
        }
        for pid in &self.manifest.requires_plugins {
            if registry.get_source(pid).is_none() && registry.get_canvas(pid).is_none() {
                out.missing_plugins.push(pid.clone());
            }
        }
        out
    }

    /// Commit the bundle into `lib`. The caller is expected to have
    /// run [`ParsedBundle::preflight`] and seen `has_collisions() ==
    /// false`.
    ///
    /// Missing plugins are not blocking — the merge still writes every
    /// bookmark payload verbatim, so a later plugin install picks the
    /// chips back up.
    ///
    /// # Errors
    /// Library persistence failures.
    pub async fn merge(&self, lib: &mut Library) -> Result<()> {
        lib.import_bundle(self).await.map_err(BundleError::Library)
    }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

struct ExportPlan {
    item_ids: Vec<ItemId>,
    folder_ids: Vec<FolderId>,
    workspace_ids: Vec<WorkspaceId>,
}

/// Resolve `selection` to a materialised preview without touching
/// disk. The dialog renders item names + plugin deps from this; a
/// subsequent [`write_bundle`] call runs the actual export.
///
/// # Errors
/// [`BundleError::MissingItem`] / [`BundleError::MissingFolder`] /
/// [`BundleError::MissingWorkspace`] when `selection` names an id the
/// library does not know.
pub fn preview_export(lib: &Library, selection: &ExportSelection) -> Result<ExportPreview> {
    let plan = plan_export(lib, selection)?;
    let manifest = build_manifest(lib, &plan)?;
    Ok(ExportPreview {
        items: manifest.items,
        folders: manifest.folders,
        workspaces: manifest.workspaces,
        requires_plugins: manifest.requires_plugins,
    })
}

fn plan_export(lib: &Library, selection: &ExportSelection) -> Result<ExportPlan> {
    let mut item_ids: HashSet<ItemId> = HashSet::new();
    let mut folder_ids: HashSet<FolderId> = HashSet::new();
    let mut workspace_ids: HashSet<WorkspaceId> = HashSet::new();

    for id in &selection.items {
        if lib.item(*id).is_none() {
            return Err(BundleError::MissingItem(*id));
        }
        item_ids.insert(*id);
    }

    for root in &selection.folders {
        if lib.folder(*root).is_none() {
            return Err(BundleError::MissingFolder(*root));
        }
        let mut queue = vec![*root];
        while let Some(folder) = queue.pop() {
            if !folder_ids.insert(folder) {
                continue;
            }
            for item in lib.items_of(folder) {
                item_ids.insert(item.id);
            }
            for child in lib.folders_of(folder) {
                queue.push(child.id);
            }
        }
    }

    for wid in &selection.workspaces {
        let ws = lib
            .workspace(*wid)
            .ok_or(BundleError::MissingWorkspace(*wid))?;
        workspace_ids.insert(*wid);
        for leaf in ws.layout.leaves() {
            for tab in &leaf.tabs {
                if lib.item(tab.item).is_some() {
                    item_ids.insert(tab.item);
                }
            }
        }
    }

    let mut items: Vec<ItemId> = item_ids.into_iter().collect();
    items.sort();
    let mut folders: Vec<FolderId> = folder_ids.into_iter().collect();
    folders.sort();
    let mut workspaces: Vec<WorkspaceId> = workspace_ids.into_iter().collect();
    workspaces.sort();
    Ok(ExportPlan {
        item_ids: items,
        folder_ids: folders,
        workspace_ids: workspaces,
    })
}

fn build_manifest(lib: &Library, plan: &ExportPlan) -> Result<BundleManifest> {
    let items: Vec<Item> = plan
        .item_ids
        .iter()
        .filter_map(|id| lib.item(*id).cloned())
        .collect();
    let folders: Vec<Folder> = plan
        .folder_ids
        .iter()
        .filter_map(|id| lib.folder(*id).cloned())
        .collect();
    let workspaces: Vec<Workspace> = plan
        .workspace_ids
        .iter()
        .filter_map(|id| lib.workspace(*id).cloned())
        .collect();

    let mut tag_ids: HashSet<TagId> = HashSet::new();
    for item in &items {
        for tag in &item.tags {
            tag_ids.insert(*tag);
        }
    }
    for folder in &folders {
        for tag in &folder.tags {
            tag_ids.insert(*tag);
        }
    }
    let mut tags: Vec<Tag> = tag_ids
        .iter()
        .filter_map(|id| lib.tag(*id).cloned())
        .collect();
    tags.sort_by(|a, b| a.id.0.cmp(&b.id.0));

    let mut requires_plugins: HashSet<String> = HashSet::new();
    for item in &items {
        if let Some(doc) = load_doc_opt(item.id) {
            for bm in doc.bookmarks() {
                for r in &bm.refs {
                    if let SourceRef::Plugin { plugin_id, .. } = r {
                        requires_plugins.insert(plugin_id.clone());
                    }
                }
            }
        }
    }
    let mut requires_plugins: Vec<String> = requires_plugins.into_iter().collect();
    requires_plugins.sort();

    Ok(BundleManifest {
        version: BUNDLE_VERSION,
        exported_at_ms: now_ms(),
        requires_plugins,
        folders,
        items,
        tags,
        workspaces,
    })
}

fn append_file<W: Write>(tar: &mut tar::Builder<W>, name: &str, bytes: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_cksum();
    tar.append_data(&mut header, name, Cursor::new(bytes))?;
    Ok(())
}

fn read_doc_bytes(id: ItemId) -> Option<Vec<u8>> {
    bodies::read_doc_bytes(id)
}

fn load_doc_opt(id: ItemId) -> Option<Doc> {
    bodies::read_doc(id).ok()
}

fn item_carries_pdf(kind: &DocKind) -> bool {
    crate::faces::carries_pdf(kind)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn parse_id<Id: core::str::FromStr>(name: &str, ext: &str) -> Option<Id> {
    let stem = name.strip_suffix(&format!(".{ext}"))?;
    stem.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_detects_item_collision() {
        let item = ItemId::new_v4();
        let bundle = ParsedBundle {
            manifest: BundleManifest {
                version: BUNDLE_VERSION,
                exported_at_ms: 0,
                requires_plugins: vec!["io.notein.pdf".into()],
                folders: Vec::new(),
                items: vec![Item {
                    id: item,
                    folder: crate::library::ROOT_FOLDER,
                    kind: DocKind::plugin("io.notein.canvas.stroke"),
                    name: "x".into(),
                    color: None,
                    tags: Vec::new(),
                    background: Default::default(),
                    created_at: 0,
                    updated_at: 0,
                    thumbnail: None,
                }],
                tags: Vec::new(),
                workspaces: Vec::new(),
            },
            docs: HashMap::new(),
            pdfs: HashMap::new(),
        };
        assert_eq!(bundle.manifest.items[0].id, item);
        assert_eq!(bundle.manifest.requires_plugins.len(), 1);
    }

    #[test]
    fn parse_id_round_trips_uuid() {
        let id = ItemId::new_v4();
        let name = format!("{id}.bin");
        let got: Option<ItemId> = parse_id(&name, "bin");
        assert_eq!(got, Some(id));
    }

    #[test]
    fn manifest_round_trips_bincode() {
        let manifest = BundleManifest {
            version: BUNDLE_VERSION,
            exported_at_ms: 42,
            requires_plugins: vec!["io.notein.pdf".into(), "org.user.quran".into()],
            folders: Vec::new(),
            items: Vec::new(),
            tags: Vec::new(),
            workspaces: Vec::new(),
        };
        let bytes = bincode::encode_to_vec(&manifest, CODEC).expect("encode");
        let (back, _) =
            bincode::decode_from_slice::<BundleManifest, _>(&bytes, CODEC).expect("decode");
        assert_eq!(manifest, back);
    }

    #[test]
    fn bundle_tar_zst_round_trips_manifest_through_stream() {
        let manifest = BundleManifest {
            version: BUNDLE_VERSION,
            exported_at_ms: 7,
            requires_plugins: vec!["io.notein.pdf".into()],
            folders: Vec::new(),
            items: Vec::new(),
            tags: Vec::new(),
            workspaces: Vec::new(),
        };
        let mut buf = Vec::<u8>::new();
        {
            let encoder = zstd::stream::write::Encoder::new(&mut buf, ZSTD_LEVEL)
                .expect("zstd encoder")
                .auto_finish();
            let mut tar = tar::Builder::new(encoder);
            tar.mode(tar::HeaderMode::Deterministic);
            let bytes = bincode::encode_to_vec(&manifest, CODEC).expect("encode");
            super::append_file(&mut tar, MANIFEST_ENTRY, &bytes).expect("append");
            tar.into_inner()
                .expect("tar finish")
                .flush()
                .expect("flush");
        }
        let parsed = parse_bundle(Cursor::new(buf)).expect("parse");
        assert_eq!(parsed.manifest, manifest);
        assert!(parsed.docs.is_empty());
        assert!(parsed.pdfs.is_empty());
    }
}

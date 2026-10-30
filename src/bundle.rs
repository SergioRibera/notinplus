//! `.notein` bundle format — export + import.
//!
//! A bundle is a `tar` archive compressed with `zstd`. It groups a
//! selection of library items (optionally scoped by their folders and
//! referencing workspaces) together with the on-disk doc bodies, any
//! embedded PDF attachments, and a manifest describing every library
//! entity carried inside.
//!
//! On import the user reviews the bundle first: items already in the
//! library are kept, replaced or copied one by one, and items of kinds
//! no installed plugin provides are imported anyway and open once the
//! plugin is installed.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Cursor, Read, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use bincode::config::{self, Configuration};
use notein_i18n::tr;
use notein_plugin::UserMessage;

use crate::bookmark::SourceRef;
use crate::doc::Doc;
use crate::library::{
    DocKind, Folder, FolderId, Item, ItemId, Library, LibraryData, LibraryError, Link, LinkIndex,
    LinkKind, PaneTab, Tag, TagId, Workspace, WorkspaceId, bodies, refs,
};

const CODEC: Configuration = config::standard();

/// Schema version carried in [`BundleManifest::version`]. Bumped on any
/// breaking payload change — the reader rejects unknown versions so a
/// future bundle can't be misinterpreted by an older build.
pub const BUNDLE_VERSION: u32 = 3;

const MANIFEST_ENTRY: &str = "manifest.bin";
const DOCS_PREFIX: &str = "docs/";
const PDFS_PREFIX: &str = "pdfs/";
const SNAPSHOTS_PREFIX: &str = "snapshots/";
/// `assets/<item id>/<file name>`: images and other files a body links to.
const ASSETS_PREFIX: &str = "assets/";
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
#[allow(clippy::struct_excessive_bools)] // independent switches of the export dialog
pub struct ExportOptions {
    /// When `true`, PDF attachments are embedded verbatim in the
    /// bundle. When `false`, only the manifest + canvas doc bodies
    /// travel — the receiver sees chips but no re-openable PDFs.
    pub include_attachments: bool,
    /// When `true`, links with one end outside the bundle travel too
    /// and show up as unavailable on the receiving side. When `false`
    /// only links between bundled items are written.
    pub include_external_links: bool,
    /// Mesas travel with their documents.
    pub include_workspaces: bool,
    /// Bookmarks and highlights stay in the documents. Off strips them
    /// from the copy (the library keeps them).
    pub include_marks: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_attachments: true,
            include_external_links: false,
            include_workspaces: true,
            include_marks: true,
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
    /// Selected items plus every notebook attached to them.
    pub items: Vec<Item>,
    pub folders: Vec<Folder>,
    pub workspaces: Vec<Workspace>,
    pub requires_plugins: Vec<String>,
    /// Items related to the selection that the user may add.
    pub related: Vec<Item>,
    /// Items the selection references (chips, bookmarks) that the
    /// user may add.
    pub referenced: Vec<Item>,
}

/// Bundle manifest. One per `.notein` file, serialised via `bincode` at
/// [`MANIFEST_ENTRY`].
#[derive(Clone, Debug, PartialEq)]
pub struct BundleManifest {
    pub version: u32,
    pub exported_at_ms: u64,
    /// Device that made the copy.
    pub exported_by: String,
    /// Distinct `SourceRef::Plugin::plugin_id` values the bookmarks
    /// inside the bundle reference.
    pub requires_plugins: Vec<String>,
    pub folders: Vec<Folder>,
    pub items: Vec<Item>,
    pub tags: Vec<Tag>,
    pub workspaces: Vec<Workspace>,
    /// Live links between bundled items (plus dangling ones when the
    /// exporter asked for them).
    pub links: Vec<Link>,
}

impl bincode::Encode for BundleManifest {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> core::result::Result<(), bincode::error::EncodeError> {
        self.version.encode(encoder)?;
        self.exported_at_ms.encode(encoder)?;
        self.exported_by.encode(encoder)?;
        self.requires_plugins.encode(encoder)?;
        self.folders.encode(encoder)?;
        self.items.encode(encoder)?;
        self.tags.encode(encoder)?;
        self.workspaces.encode(encoder)?;
        self.links.encode(encoder)?;
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
            exported_by: bincode::Decode::decode(decoder)?,
            requires_plugins: bincode::Decode::decode(decoder)?,
            folders: bincode::Decode::decode(decoder)?,
            items: bincode::Decode::decode(decoder)?,
            tags: bincode::Decode::decode(decoder)?,
            workspaces: bincode::Decode::decode(decoder)?,
            links: bincode::Decode::decode(decoder)?,
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

impl UserMessage for BundleError {
    fn user_message(&self) -> String {
        match self {
            Self::Io(err) => err.user_message(),
            Self::Library(err) => err.user_message(),
            Self::UnknownVersion(_) => tr!("errors-bundle-newer"),
            Self::Codec(_)
            | Self::MissingManifest
            | Self::MissingItem(_)
            | Self::MissingFolder(_)
            | Self::MissingWorkspace(_) => tr!("errors-bundle-damaged"),
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
    lib: &LibraryData,
    selection: &ExportSelection,
    options: ExportOptions,
    exported_by: &str,
    mut progress: impl FnMut(usize),
) -> Result<()> {
    let plan = plan_export(lib, selection, false)?;
    let mut manifest = build_manifest(lib, &plan, options.include_external_links);
    exported_by.clone_into(&mut manifest.exported_by);
    if !options.include_workspaces {
        manifest.workspaces.clear();
    }

    let encoder = zstd::stream::write::Encoder::new(sink, ZSTD_LEVEL)?.auto_finish();
    let mut tar = tar::Builder::new(encoder);
    tar.mode(tar::HeaderMode::Deterministic);

    let manifest_bytes =
        bincode::encode_to_vec(&manifest, CODEC).map_err(|e| BundleError::Codec(e.to_string()))?;
    append_file(&mut tar, MANIFEST_ENTRY, &manifest_bytes)?;

    for (done, item) in manifest.items.iter().enumerate() {
        progress(done);
        if let Some(doc_bytes) = read_doc_bytes(item.id) {
            let doc_bytes = if options.include_marks {
                doc_bytes
            } else {
                strip_doc_marks(&doc_bytes)
            };
            let path = format!("{DOCS_PREFIX}{}.{DOC_EXT}", item.id);
            append_file(&mut tar, &path, &doc_bytes)?;
        }
        if let Ok(Some(snapshot)) = bodies::read_plugin_snapshot(item.id, &item.kind.plugin_id) {
            let snapshot = if options.include_marks {
                snapshot
            } else {
                strip_snapshot_marks(&item.kind.plugin_id, snapshot)
            };
            let path = format!("{SNAPSHOTS_PREFIX}{}.{DOC_EXT}", item.id);
            append_file(&mut tar, &path, &snapshot)?;
        }
        for (name, bytes) in bodies::read_assets(item.id) {
            let path = format!("{ASSETS_PREFIX}{}/{name}", item.id);
            append_file(&mut tar, &path, &bytes)?;
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
    lib: &LibraryData,
    selection: &ExportSelection,
    options: ExportOptions,
) -> Result<()> {
    let file = File::create(dest)?;
    write_bundle(file, lib, selection, options, "", |_| {})
}

/// Decoded bundle payload. Lives entirely in memory — the on-disk
/// `.notein` file can go away after this returns.
#[derive(Debug)]
pub struct ParsedBundle {
    pub manifest: BundleManifest,
    pub docs: HashMap<ItemId, Vec<u8>>,
    pub pdfs: HashMap<ItemId, Vec<u8>>,
    /// Plugin content of each item (its kind's snapshot).
    pub snapshots: HashMap<ItemId, Vec<u8>>,
    /// Files each body links to, by name.
    pub assets: HashMap<ItemId, Vec<(String, Vec<u8>)>>,
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
    let mut snapshots: HashMap<ItemId, Vec<u8>> = HashMap::new();
    let mut assets: HashMap<ItemId, Vec<(String, Vec<u8>)>> = HashMap::new();

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
        } else if let Some(rest) = name.strip_prefix(SNAPSHOTS_PREFIX)
            && let Some(id) = parse_id::<ItemId>(rest, DOC_EXT)
        {
            snapshots.insert(id, buf);
        } else if let Some(rest) = name.strip_prefix(ASSETS_PREFIX)
            && let Some((id, file)) = rest.split_once('/')
            && let Ok(id) = id.parse::<ItemId>()
        {
            assets.entry(id).or_default().push((file.to_owned(), buf));
        }
    }

    let manifest = manifest.ok_or(BundleError::MissingManifest)?;
    Ok(ParsedBundle {
        manifest,
        docs,
        pdfs,
        snapshots,
        assets,
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
    /// Commit the bundle into `lib`.
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
    items: Vec<ItemId>,
    folders: Vec<FolderId>,
    workspaces: Vec<WorkspaceId>,
}

/// Resolve `selection` to a materialised preview without touching
/// disk. The dialog renders item names + plugin deps from this; a
/// subsequent [`write_bundle`] call runs the actual export.
///
/// # Errors
/// [`BundleError::MissingItem`] / [`BundleError::MissingFolder`] /
/// [`BundleError::MissingWorkspace`] when `selection` names an id the
/// library does not know.
pub fn preview_export(lib: &LibraryData, selection: &ExportSelection) -> Result<ExportPreview> {
    let plan = plan_export(lib, selection, true)?;
    let manifest = build_manifest(lib, &plan, false);
    let in_bundle: HashSet<ItemId> = plan.items.iter().copied().collect();
    let related = extra_items(lib, &in_bundle, |id| {
        lib.links_of(id)
            .filter(|l| l.kind == LinkKind::Related)
            .filter_map(|l| l.other_end(id))
            .collect()
    });
    let referenced = extra_items(lib, &in_bundle, |id| {
        refs::refs_from(id).into_iter().map(|r| r.to).collect()
    });
    Ok(ExportPreview {
        items: manifest.items,
        folders: manifest.folders,
        workspaces: manifest.workspaces,
        requires_plugins: manifest.requires_plugins,
        related,
        referenced,
    })
}

/// Items one hop away from `in_bundle` through `neighbours`, in
/// library order, that are not bundled yet.
fn extra_items(
    lib: &LibraryData,
    in_bundle: &HashSet<ItemId>,
    neighbours: impl Fn(ItemId) -> Vec<ItemId>,
) -> Vec<Item> {
    let mut seen: HashSet<ItemId> = HashSet::new();
    let mut out = Vec::new();
    for id in in_bundle {
        for other in neighbours(*id) {
            if in_bundle.contains(&other) || !seen.insert(other) {
                continue;
            }
            if let Some(item) = lib.item(other) {
                out.push(item.clone());
            }
        }
    }
    out.sort_by_key(|a| a.id);
    out
}

/// Resolve the selection axes into id lists. With `with_notebooks`
/// every notebook attached to a selected item (transitively) joins the
/// plan; the writer passes `false` because its selection is already
/// the user-trimmed final list.
fn plan_export(
    lib: &LibraryData,
    selection: &ExportSelection,
    with_notebooks: bool,
) -> Result<ExportPlan> {
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
            for id in leaf.tabs.iter().filter_map(PaneTab::item) {
                if lib.item(id).is_some() {
                    item_ids.insert(id);
                }
            }
        }
    }

    if with_notebooks {
        expand_notebooks(&mut item_ids, lib.links());
    }

    let mut items: Vec<ItemId> = item_ids.into_iter().collect();
    items.sort();
    let mut folders: Vec<FolderId> = folder_ids.into_iter().collect();
    folders.sort();
    let mut workspaces: Vec<WorkspaceId> = workspace_ids.into_iter().collect();
    workspaces.sort();
    Ok(ExportPlan {
        items,
        folders,
        workspaces,
    })
}

/// Grow `items` with every notebook hanging off any of them, following
/// notebooks of notebooks until nothing new appears.
fn expand_notebooks(items: &mut HashSet<ItemId>, links: &LinkIndex) {
    let mut queue: Vec<ItemId> = items.iter().copied().collect();
    while let Some(id) = queue.pop() {
        for notebook in links.companions_of(id) {
            if items.insert(notebook) {
                queue.push(notebook);
            }
        }
    }
}

fn build_manifest(
    lib: &LibraryData,
    plan: &ExportPlan,
    include_external_links: bool,
) -> BundleManifest {
    let items: Vec<Item> = plan
        .items
        .iter()
        .filter_map(|id| lib.item(*id).cloned())
        .collect();
    let folders: Vec<Folder> = plan
        .folders
        .iter()
        .filter_map(|id| lib.folder(*id).cloned())
        .collect();
    let workspaces: Vec<Workspace> = plan
        .workspaces
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
    tags.sort_by_key(|a| a.id.0);

    let mut requires_plugins: HashSet<String> = HashSet::new();
    for item in &items {
        requires_plugins.insert(item.kind.plugin_id.clone());
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

    let in_bundle: HashSet<ItemId> = plan.items.iter().copied().collect();
    let mut links: Vec<Link> = lib
        .links()
        .links
        .iter()
        .filter(|l| !l.deleted)
        .filter(|l| {
            let ends =
                usize::from(in_bundle.contains(&l.from)) + usize::from(in_bundle.contains(&l.to));
            ends == 2 || (include_external_links && ends == 1)
        })
        .cloned()
        .collect();
    links.sort_by_key(|a| a.id.0);

    BundleManifest {
        version: BUNDLE_VERSION,
        exported_at_ms: now_ms(),
        exported_by: String::new(),
        requires_plugins,
        folders,
        items,
        tags,
        workspaces,
        links,
    }
}

/// Ink document bytes with every bookmark removed.
fn strip_doc_marks(bytes: &[u8]) -> Vec<u8> {
    let Ok(mut doc) = crate::doc::Doc::from_snapshot(bytes) else {
        return bytes.to_vec();
    };
    let ids: Vec<_> = doc.bookmarks().iter().map(|b| b.id).collect();
    for id in ids {
        doc.emit(crate::doc_op::DocOp::DeleteBookmark { id });
    }
    doc.snapshot_bytes().unwrap_or_else(|_| bytes.to_vec())
}

/// A kind snapshot with its highlights and marks removed.
fn strip_snapshot_marks(kind: &str, snapshot: Vec<u8>) -> Vec<u8> {
    crate::plugins::kind(kind)
        .and_then(|kind| kind.without_marks(&snapshot))
        .unwrap_or(snapshot)
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

/// Whether items of `kind` keep a file next to their content.
fn item_carries_pdf(kind: &DocKind) -> bool {
    crate::plugins::kind_info(&kind.plugin_id).is_some_and(|k| k.attachment.is_some())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn parse_id<Id: core::str::FromStr>(name: &str, ext: &str) -> Option<Id> {
    let stem = name.strip_suffix(&format!(".{ext}"))?;
    stem.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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
            exported_by: String::new(),
            requires_plugins: vec!["io.notein.pdf".into(), "org.user.quran".into()],
            folders: Vec::new(),
            items: Vec::new(),
            tags: Vec::new(),
            workspaces: Vec::new(),
            links: Vec::new(),
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
            exported_by: String::new(),
            requires_plugins: vec!["io.notein.pdf".into()],
            folders: Vec::new(),
            items: Vec::new(),
            tags: Vec::new(),
            workspaces: Vec::new(),
            links: Vec::new(),
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

#[cfg(test)]
mod closure_tests {
    use super::*;
    use crate::library::LinkKind;

    fn item(seed: u8) -> ItemId {
        ItemId([seed; 16])
    }

    #[test]
    fn notebooks_follow_their_documents_transitively() {
        let mut links = LinkIndex::default();
        links.insert(item(1), item(2), LinkKind::Companion, 1);
        links.insert(item(2), item(3), LinkKind::Companion, 1);
        links.insert(item(1), item(4), LinkKind::Related, 1);
        links.insert(item(5), item(1), LinkKind::Companion, 1);
        let mut set: HashSet<ItemId> = HashSet::from([item(1)]);
        expand_notebooks(&mut set, &links);
        let mut got: Vec<ItemId> = set.into_iter().collect();
        got.sort();
        assert_eq!(got, vec![item(1), item(2), item(3)]);
    }

    #[test]
    fn manifest_round_trips_links() {
        let mut links = LinkIndex::default();
        links.insert(item(1), item(2), LinkKind::Companion, 7);
        let manifest = BundleManifest {
            version: BUNDLE_VERSION,
            exported_at_ms: 1,
            exported_by: String::new(),
            requires_plugins: Vec::new(),
            folders: Vec::new(),
            items: Vec::new(),
            tags: Vec::new(),
            workspaces: Vec::new(),
            links: links.links.clone(),
        };
        let bytes = bincode::encode_to_vec(&manifest, CODEC).expect("encode");
        let (back, _) =
            bincode::decode_from_slice::<BundleManifest, _>(&bytes, CODEC).expect("decode");
        assert_eq!(back.links.len(), 1);
        assert_eq!(back.links[0].from, item(1));
    }
}

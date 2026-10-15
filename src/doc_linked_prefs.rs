//! Per-item sidecar for the DocLinked split view state.
//!
//! A linked-doc item renders a PDF background with canvas annotations
//! on top (overlay mode). The user can elect to split the two surfaces
//! into side-by-side or stacked panes; the preference persists across
//! sessions so a mid-reading split survives a navigation back to Home.
//!
//! UI state only — layout is NOT CRDT doc state. Each device keeps its
//! own split preference and it never flows through the op log.
//!
//! Layout: `<data_dir>/doc_linked/<item_id>.bin`.

use std::fs;
use std::io;
use std::path::PathBuf;

use bincode::config::{self, Configuration};

use crate::library::bodies;
use crate::library::{ItemId, SplitOrientation};

const CODEC: Configuration = config::standard();
const SUBDIR: &str = "doc_linked";
const EXT: &str = "bin";

/// Split-view preference for a single DocLinked item.
///
/// `orientation == None` means the shell is in overlay mode (default
/// behaviour: PDF background with annotations layered on top in a
/// single surface). `Some(orientation)` splits the shell along the
/// given axis — PDF on the first side, canvas on the second — with
/// the divider at `ratio` fraction of the parent extent.
#[derive(Clone, Copy, Debug, PartialEq, bincode::Encode, bincode::Decode)]
pub struct DocLinkedSplit {
    pub orientation: Option<SplitOrientation>,
    pub ratio: f32,
}

impl Default for DocLinkedSplit {
    fn default() -> Self {
        Self {
            orientation: None,
            ratio: 0.5,
        }
    }
}

#[must_use]
fn path(id: ItemId) -> PathBuf {
    bodies::root_dir().join(SUBDIR).join(format!("{id}.{EXT}"))
}

/// Read the split sidecar for `id`. Returns the default (overlay mode)
/// when missing / corrupt — callers never have to juggle an "unset"
/// state separately from the explicit overlay choice.
#[must_use]
pub fn load(id: ItemId) -> DocLinkedSplit {
    let Ok(bytes) = fs::read(path(id)) else {
        return DocLinkedSplit::default();
    };
    bincode::decode_from_slice::<DocLinkedSplit, _>(&bytes, CODEC)
        .map(|(v, _)| v)
        .unwrap_or_default()
}

/// # Errors
/// Encode / fs failures.
pub fn save(id: ItemId, split: &DocLinkedSplit) -> io::Result<()> {
    let bytes = bincode::encode_to_vec(split, CODEC)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(&path(id), &bytes)
}

fn write_atomic(dest: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = dest.to_path_buf();
    let file_name = tmp
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    let mut with_ext = file_name;
    with_ext.push(".tmp");
    tmp.set_file_name(with_ext);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, dest)
}

//! Per-item sidecar persisting the pane-tree layout used by shells
//! that mount more than one pane for the same bundle id — currently
//! [`crate::doc_linked::DocLinkedView`], but intentionally shaped
//! generic so future shells (multi-source bundles, mind-map + outline
//! splits, …) can reuse the same storage.
//!
//! UI state only — layout is NOT CRDT doc state (see SOURCES_PLAN §4
//! "Qué NO es CRDT"). Each device keeps its own split preferences and
//! they never flow through the op log.
//!
//! Layout: `<data_dir>/pane_layouts/<item_id>.bin`.

use std::fs;
use std::io;
use std::path::PathBuf;

use bincode::config::{self, Configuration};

use crate::library::bodies;
use crate::library::{ItemId, PaneLayout};

const CODEC: Configuration = config::standard();
const SUBDIR: &str = "pane_layouts";
const EXT: &str = "bin";

#[must_use]
fn path(id: ItemId) -> PathBuf {
    bodies::root_dir().join(SUBDIR).join(format!("{id}.{EXT}"))
}

/// Read the layout sidecar for `id`. `None` when missing / corrupt —
/// callers fall back to their shell-specific default layout.
#[must_use]
pub fn load(id: ItemId) -> Option<PaneLayout> {
    let bytes = fs::read(path(id)).ok()?;
    bincode::decode_from_slice::<PaneLayout, _>(&bytes, CODEC)
        .ok()
        .map(|(l, _)| l)
}

/// # Errors
/// Encode / fs failures.
pub fn save(id: ItemId, layout: &PaneLayout) -> io::Result<()> {
    let bytes = bincode::encode_to_vec(layout, CODEC)
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

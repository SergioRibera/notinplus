//! Per-item persistence for the DocLinked split shell.
//!
//! Toggle (`show_canvas`) and divider ratio (`split_ratio`) are UI state
//! the user expects to survive relaunch *per item* — not globally, not
//! inside the doc body. Sidecar file keyed by [`ItemId`], same shape as
//! [`crate::prefs::DocView`] but isolated so viewport sidecars stay
//! untouched if this schema evolves.

use std::fs;
use std::io;
use std::path::PathBuf;

use bincode::config::{self, Configuration};

use crate::library::ItemId;
use crate::library::bodies;

const CODEC: Configuration = config::standard();
const SUBDIR: &str = "doc_linked";
const EXT: &str = "bin";

/// Default split ratio when no sidecar exists. Matches the hard-coded
/// default previously held in [`crate::doc_linked::DocLinkedView`].
pub const DEFAULT_SPLIT_RATIO: f32 = 0.55;

#[istmo::message]
#[derive(Clone, Copy, Debug)]
pub struct DocLinkedPrefs {
    pub show_canvas: bool,
    pub split_ratio: f32,
}

impl Default for DocLinkedPrefs {
    fn default() -> Self {
        Self {
            show_canvas: false,
            split_ratio: DEFAULT_SPLIT_RATIO,
        }
    }
}

#[must_use]
fn path(id: ItemId) -> PathBuf {
    bodies::root_dir()
        .join(SUBDIR)
        .join(format!("{id}.{EXT}"))
}

#[must_use]
pub fn load(id: ItemId) -> DocLinkedPrefs {
    let Ok(bytes) = fs::read(path(id)) else {
        return DocLinkedPrefs::default();
    };
    bincode::decode_from_slice::<DocLinkedPrefs, _>(&bytes, CODEC)
        .map(|(p, _)| p)
        .unwrap_or_default()
}

/// # Errors
/// Encode / fs failures.
pub fn save(id: ItemId, prefs: DocLinkedPrefs) -> io::Result<()> {
    let bytes = bincode::encode_to_vec(prefs, CODEC)
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

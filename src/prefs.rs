//! Session persistence for toolbar + per-doc view state.
//!
//! Doc bodies carry layers / strokes / active layer; they already round-trip
//! through [`crate::library::bodies`]. Two bits of state do **not** live in
//! the doc:
//!
//! * **Global toolbar prefs** — selected brush kind, per-kind size scale,
//!   last-used colour, draw / pan toggle. Users expect "marker at 0.25"
//!   to survive relaunch regardless of which doc opens next, so these
//!   land in a single global file.
//! * **Per-doc view state** — pan + zoom of the canvas viewport. Users
//!   expect each doc to reopen at the same place they left it, so each
//!   doc gets a sidecar keyed by [`ItemId`].
//!
//! Layout:
//!
//! ```text
//! <data_dir>/notinplus/
//!     prefs.bin                      // global toolbar prefs
//!     views/<item_id>.bin            // per-doc viewport sidecar
//! ```
//!
//! Writes are coalesced through [`crate::canvas::Board`]'s commit sinks
//! (`set_prefs_sink`, `set_view_sink`) and drained by the workers in
//! [`crate::route`].

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use bincode::config::{self, Configuration};

use crate::brush::{BrushKind, BrushPreset};
use crate::canvas::{InputMode, Viewport};
use crate::library::ItemId;
use crate::library::bodies;

const CODEC: Configuration = config::standard();
const PREFS_FILE: &str = "prefs.bin";
const VIEWS_SUBDIR: &str = "views";
const VIEW_EXT: &str = "bin";

/// Global toolbar prefs. `HashMap`-shaped state is serialised as
/// `Vec<(K, V)>` to keep bincode's derive requirements narrow.
#[istmo::message]
#[derive(Clone, Debug, Default)]
pub struct Prefs {
    pub current_preset: Option<BrushPreset>,
    pub current_color: Option<[u8; 4]>,
    pub size_scales: Vec<(BrushKind, f32)>,
    /// `true` → [`InputMode::Pan`]; `false` → [`InputMode::Draw`]. Encoded
    /// as `bool` so a future `InputMode` variant doesn't force a schema
    /// bump on existing files.
    pub input_mode_pan: bool,
}

impl Prefs {
    #[must_use]
    pub fn input_mode(&self) -> InputMode {
        if self.input_mode_pan {
            InputMode::Pan
        } else {
            InputMode::Draw
        }
    }

    #[must_use]
    pub fn size_scales_map(&self) -> HashMap<BrushKind, f32> {
        self.size_scales.iter().copied().collect()
    }
}

/// Per-doc view state (pan + zoom). Primitive fields so the encoded shape
/// is independent of [`Viewport`]'s derive stance.
#[istmo::message]
#[derive(Clone, Copy, Debug, Default)]
pub struct DocView {
    pub tx: f32,
    pub ty: f32,
    pub scale: f32,
}

impl DocView {
    #[must_use]
    pub const fn from_viewport(v: Viewport) -> Self {
        Self {
            tx: v.tx,
            ty: v.ty,
            scale: v.scale,
        }
    }

    #[must_use]
    pub const fn into_viewport(self) -> Viewport {
        Viewport {
            tx: self.tx,
            ty: self.ty,
            scale: self.scale,
        }
    }
}

#[must_use]
fn prefs_path() -> PathBuf {
    bodies::root_dir().join(PREFS_FILE)
}

#[must_use]
fn view_path(id: ItemId) -> PathBuf {
    bodies::root_dir()
        .join(VIEWS_SUBDIR)
        .join(format!("{}.{VIEW_EXT}", id.0))
}

/// Read [`Prefs`] from disk. Missing / corrupt payload → `Prefs::default`
/// so a first-run or schema-bump never surfaces to the user as an error.
#[must_use]
pub fn load_prefs() -> Prefs {
    let Ok(bytes) = fs::read(prefs_path()) else {
        return Prefs::default();
    };
    bincode::decode_from_slice::<Prefs, _>(&bytes, CODEC)
        .map(|(p, _)| p)
        .unwrap_or_default()
}

/// Atomically overwrite the prefs file.
///
/// # Errors
/// Encode / fs failures.
pub fn save_prefs(prefs: &Prefs) -> io::Result<()> {
    let bytes = bincode::encode_to_vec(prefs, CODEC)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(&prefs_path(), &bytes)
}

/// Read the view sidecar for `id`. `None` when missing / corrupt.
#[must_use]
pub fn load_view(id: ItemId) -> Option<DocView> {
    let bytes = fs::read(view_path(id)).ok()?;
    bincode::decode_from_slice::<DocView, _>(&bytes, CODEC)
        .ok()
        .map(|(v, _)| v)
}

/// Atomically overwrite the view sidecar for `id`.
///
/// # Errors
/// Encode / fs failures.
pub fn save_view(id: ItemId, view: DocView) -> io::Result<()> {
    let bytes = bincode::encode_to_vec(view, CODEC)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(&view_path(id), &bytes)
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

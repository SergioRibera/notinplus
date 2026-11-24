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
//!     local.bin                      // toolbar prefs ("prefs.bin"),
//!                                    // see [`crate::persist`]
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

use crate::brush::{BrushKind, BrushPreset};
use crate::canvas::{InputMode, Viewport};
use crate::library::bodies;
use crate::library::{ItemId, ViewSlot};
use crate::persist::Persisted;

const VIEWS_SUBDIR: &str = "views";
const VIEW_EXT: &str = "bin";
/// Global toolbar prefs. `HashMap`-shaped state is serialised as
/// `Vec<(K, V)>` to keep the derive requirements narrow.
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
    /// The writer's handwriting style, learned from beautified words.
    pub style: notin_beautify::StyleProfile,
}

impl Persisted for Prefs {
    const FILE: &'static str = "prefs.bin";
}

impl Prefs {
    #[must_use]
    pub const fn input_mode(&self) -> InputMode {
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

/// Per-doc view state: pan, zoom and the colour new text boxes start
/// with. Primitive fields so the encoded shape is independent of
/// [`Viewport`]'s derive stance. Fields only ever go at the end: older
/// builds read the leading ones and ignore the rest.
#[istmo::message]
#[derive(Clone, Copy, Debug, Default)]
pub struct DocView {
    pub tx: f32,
    pub ty: f32,
    pub scale: f32,
    /// Last text colour picked on this canvas; `None` follows the paper.
    pub text_color: Option<[u8; 4]>,
}

/// [`DocView`] as written before it carried a text colour.
#[istmo::message]
#[derive(Clone, Copy, Debug)]
struct LegacyDocView {
    tx: f32,
    ty: f32,
    scale: f32,
}

impl DocView {
    #[must_use]
    pub const fn from_viewport(v: Viewport, text_color: Option<[u8; 4]>) -> Self {
        Self {
            tx: v.tx,
            ty: v.ty,
            scale: v.scale,
            text_color,
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

/// Which view a viewport sidecar belongs to: the item alone (a
/// standalone mount) or the item as shown by one pane of one mesa.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ViewKey {
    pub item: ItemId,
    pub slot: Option<ViewSlot>,
}

#[must_use]
fn view_path(key: ViewKey) -> PathBuf {
    let views = bodies::root_dir().join(VIEWS_SUBDIR);
    key.slot.map_or_else(
        || views.join(format!("{}.{VIEW_EXT}", key.item)),
        |slot| {
            views
                .join(key.item.to_string())
                .join(format!("{}-{}.{VIEW_EXT}", slot.workspace, slot.pane))
        },
    )
}

/// Read [`Prefs`]. Missing / corrupt payload → `Prefs::default`
/// so a first-run or schema-bump never surfaces to the user as an error.
#[must_use]
#[sismo::function("prefs::load_prefs")]
pub fn load_prefs() -> Prefs {
    Prefs::load()
}

/// Store [`Prefs`].
///
/// # Errors
/// Encode / fs failures.
#[sismo::function("prefs::save_prefs")]
pub fn save_prefs(prefs: &Prefs) -> io::Result<()> {
    prefs.save()
}

/// Read the view sidecar for `key`. `None` when missing / corrupt.
#[must_use]
#[sismo::function("prefs::load_view")]
pub fn load_view(key: ViewKey) -> Option<DocView> {
    let bytes = fs::read(view_path(key)).ok()?;
    decode_view(&bytes)
}

/// A view sidecar, current or from before the text colour.
fn decode_view(bytes: &[u8]) -> Option<DocView> {
    notin_plugin::decode::<DocView>(bytes).ok().or_else(|| {
        let old = notin_plugin::decode::<LegacyDocView>(bytes).ok()?;
        Some(DocView {
            tx: old.tx,
            ty: old.ty,
            scale: old.scale,
            text_color: None,
        })
    })
}

/// Atomically overwrite the view sidecar for `key`.
///
/// # Errors
/// Encode / fs failures.
#[sismo::function("prefs::save_view")]
pub fn save_view(key: ViewKey, view: DocView) -> io::Result<()> {
    let bytes =
        notin_plugin::encode(&view).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(&view_path(key), &bytes)
}

#[sismo::function("prefs::write_atomic")]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_saved_before_the_text_colour_still_load() {
        let old = notin_plugin::encode(&LegacyDocView {
            tx: 1.0,
            ty: 2.0,
            scale: 3.0,
        })
        .expect("encode");
        let view = decode_view(&old).expect("legacy view");
        assert_eq!((view.tx, view.ty, view.scale), (1.0, 2.0, 3.0));
        assert_eq!(view.text_color, None);

        let new = notin_plugin::encode(&DocView {
            text_color: Some([1, 2, 3, 255]),
            ..view
        })
        .expect("encode");
        assert_eq!(
            decode_view(&new).expect("view").text_color,
            Some([1, 2, 3, 255])
        );
        // Older builds read the leading fields and ignore the colour.
        let read_by_old = notin_plugin::decode::<LegacyDocView>(&new).expect("old reader");
        assert_eq!(read_by_old.scale, 3.0);
    }
}

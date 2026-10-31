//! Picking tools, and the stylus shortcut that flips between two.

use super::{Board, InputMode};
use crate::brush::{BrushKind, BrushPreset};
use crate::settings::PenDoubleTap;

impl Board {
    /// Switch to `preset`, remembering the tool it replaces so the
    /// stylus shortcut can come back to it. Leaves lasso and text.
    #[sismo::function("tools::choose_preset")]
    pub fn choose_preset(&mut self, preset: BrushPreset) {
        let current = *self.current_preset();
        if current.kind != preset.kind {
            self.previous_preset = Some(current);
        }
        self.set_current_preset(preset);
        if matches!(self.input_mode(), InputMode::Lasso | InputMode::Text) {
            self.clear_selection();
            self.set_input_mode(InputMode::Draw);
        }
        self.notify();
    }

    /// The tool before the current one.
    #[must_use]
    pub const fn previous_preset(&self) -> Option<BrushPreset> {
        self.previous_preset
    }

    /// Run the stylus shortcut.
    #[sismo::function("tools::pen_shortcut")]
    pub fn pen_shortcut(&mut self, action: PenDoubleTap) {
        match action {
            PenDoubleTap::Nothing => {}
            PenDoubleTap::Eraser => {
                if self.current_preset().kind == BrushKind::Eraser {
                    let back = self.previous_preset.unwrap_or_else(BrushPreset::pen);
                    self.choose_preset(back);
                } else {
                    self.choose_preset(BrushPreset::eraser());
                }
            }
            PenDoubleTap::LastBrush => {
                if let Some(back) = self.previous_preset {
                    self.choose_preset(back);
                }
            }
        }
    }
}

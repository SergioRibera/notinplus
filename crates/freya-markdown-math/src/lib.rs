//! LaTeX formulas for [`freya_markdown_editor`], typeset by Typst.
//!
//! Add [`TypstMath`] as a decorator and every `$$ ... $$` block and inline
//! `$ ... $` formula is drawn instead of shown as TeX. `MiTeX` converts the
//! TeX to Typst math, so the usual LaTeX math commands work without a TeX
//! installation; fonts are embedded, so it works offline on every
//! platform.

mod typeset;

use freya::prelude::*;
use freya_markdown_editor::{Decorator, MathContext};

pub use typeset::{Formula, MathError, Style, typeset};

/// Draws formulas with Typst. See the [crate] docs.
#[derive(Clone, Copy, Debug, Default)]
pub struct TypstMath;

impl Decorator for TypstMath {
    #[cfg_attr(feature = "sismo", sismo::function("math::render"))]
    fn render_math(&self, math: &MathContext<'_>) -> Option<Element> {
        let tex = math.tex.trim();
        if tex.is_empty() {
            return None;
        }
        let colors = &math.theme.colors;
        let text = colors.text;
        // New Computer Modern runs small next to UI fonts.
        let style = Style::new(
            math.display,
            math.font_size * 1.12,
            [text.r(), text.g(), text.b()],
        );
        Some(match typeset(tex, &style) {
            Ok(formula) => {
                #[cfg(feature = "sismo")]
                sismo::scope!("math::svg_viewer");
                // Shrink what does not fit rather than overflow the note.
                let scale = if math.max_width > 0. && formula.width > math.max_width {
                    math.max_width / formula.width
                } else {
                    1.
                };
                SvgViewer::new(formula.svg)
                    .width(Size::px(formula.width * scale))
                    .height(Size::px(formula.height * scale))
                    .into_element()
            }
            Err(error) => label()
                .font_size(math.font_size * 0.85)
                .color(colors.caution)
                .max_lines(if math.display { 3 } else { 1 })
                .text(if math.display {
                    format!("⚠ {error}")
                } else {
                    format!("⚠ {tex}")
                })
                .into_element(),
        })
    }
}

//! An inline formula: drawn by a decorator when there is one, and pressed
//! to edit its TeX with the drawing updating as you type.

use freya::prelude::*;

use super::Shared;
use crate::controller::AtomSlot;
use crate::decorator::MathContext;
use crate::labels::Label;
use crate::popup::{Anchored, input_colors, popup_frame};

#[derive(Clone, PartialEq)]
pub(crate) struct MathAtom {
    pub(crate) tex: String,
    pub(crate) slot: AtomSlot,
    pub(crate) font_size: f32,
    pub(crate) max_width: f32,
    pub(crate) shared: Shared,
}

impl MathAtom {
    fn drawn(&self, tex: &str) -> Option<Element> {
        self.shared.decorators.render_math(&MathContext {
            tex,
            display: false,
            font_size: self.font_size,
            max_width: self.max_width,
            theme: &self.shared.theme,
        })
    }

    /// The TeX itself, when no decorator draws formulas.
    fn source(&self) -> Element {
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        rect()
            .padding((theme.px(1.), theme.px(6.)))
            .corner_radius(theme.px(6.))
            .background(colors.code_background)
            .child(
                label()
                    .font_size(self.font_size * 0.9)
                    .font_family(theme.monospace.clone())
                    .color(colors.code_text)
                    .max_lines(1)
                    .text(self.tex.clone()),
            )
            .into_element()
    }
}

impl Component for MathAtom {
    fn render(&self) -> impl IntoElement {
        let initial = self.tex.clone();
        let draft = use_state(move || initial);
        let shown = self.drawn(&self.tex).unwrap_or_else(|| self.source());
        if self.shared.read_only {
            return shown;
        }
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let controller = self.shared.controller;
        let slot = self.slot;
        let open = controller.atom_menu() == Some(slot);
        let frame = rect()
            .corner_radius(theme.px(4.))
            .background(if open {
                colors.selection
            } else {
                Color::TRANSPARENT
            })
            .cursor(CursorIcon::Pointer)
            .child(shown)
            .into_element();
        let popup = open.then(|| {
            let preview = self.drawn(&draft.read());
            popup_frame(colors)
                .padding(8.)
                .width(Size::px(300.))
                .spacing(8.)
                .child(
                    Input::new(draft)
                        .placeholder(theme.labels.get(Label::Formula))
                        .theme_colors(input_colors(colors))
                        .auto_focus(true)
                        .width(Size::fill())
                        .on_submit(move |tex: String| {
                            controller.set_math(slot, tex.trim());
                            controller.open_atom_menu(None);
                        }),
                )
                .maybe_child(preview.map(|preview| {
                    rect()
                        .width(Size::fill())
                        .min_height(Size::px(36.))
                        .padding(6.)
                        .corner_radius(6.)
                        .background(colors.code_background)
                        .center()
                        .child(preview)
                }))
                .into_element()
        });
        let current = self.tex.clone();
        let mut draft = draft;
        Anchored::new(frame, popup)
            .on_dismiss(move || {
                // Leaving the field keeps what was typed, like any other
                // text in the note.
                let typed = draft.peek().trim().to_owned();
                if typed != current {
                    controller.set_math(slot, &typed);
                }
                draft.set(current.clone());
                controller.open_atom_menu(None);
            })
            .into_element()
    }
}

//! Ajustes › Notas.

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{ChoiceCard, Group, ListRow, Section, Trailing, tokens};

use crate::settings::SettingsStore;

/// The Notas section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Notes;

/// A line of a note as it looks while typing: plain text, then the
/// bold word either formatted or between its marks, then the caret.
fn typing_preview(marks: bool) -> Rect {
    let p = palette();
    let word = if marks {
        label()
            .text("**negrita**")
            .font_size(tokens::text::BODY)
            .color(p.t3)
    } else {
        label()
            .text("negrita")
            .font_size(tokens::text::BODY)
            .font_weight(FontWeight::BOLD)
            .color(p.t1)
    };
    rect()
        .width(Size::fill())
        .height(Size::px(44.0))
        .padding((0.0, 12.0))
        .corner_radius(tokens::radius::MD)
        .background(p.bg)
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(4.0)
        .child(
            label()
                .text("Una idea")
                .font_size(tokens::text::BODY)
                .color(p.t1)
                .max_lines(1),
        )
        .child(word)
        .child(
            rect()
                .width(Size::px(2.0))
                .height(Size::px(20.0))
                .corner_radius(1.0)
                .background(p.accent),
        )
}

impl Component for Notes {
    fn render(&self) -> impl IntoElement {
        let store = SettingsStore::get();
        let notes = store.read().notes;
        let format = |marks: bool, title: &'static str, detail: &'static str| {
            ChoiceCard::new(typing_preview(marks), title)
                .detail(detail)
                .chosen(notes.show_markdown_marks == marks)
                .on_pick(move |()| {
                    let mut store = store;
                    store.update(|s| s.notes.show_markdown_marks = marks);
                })
        };
        rect()
            .width(Size::fill())
            .spacing(24.0)
            .child(
                Section::new("Formato al escribir").child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .content(Content::Flex)
                        .spacing(10.0)
                        .child(format(
                            false,
                            "Como en un procesador de texto",
                            "Ves negrita, no los asteriscos.",
                        ))
                        .child(format(
                            true,
                            "Mostrar marcas de markdown",
                            "Cerca del cursor aparece **negrita**.",
                        )),
                ),
            )
            .child(
                Section::new("Referencias").child(
                    Group::default().child(
                        ListRow::new("Abrir referencias al lado")
                            .subtitle("Tocar un versículo, una página o una nota citada la abre en el otro panel.")
                            .trailing(Trailing::Switch {
                                on: notes.open_refs_beside,
                                on_toggle: EventHandler::new(move |on| {
                                    let mut store = store;
                                    store.update(|s| s.notes.open_refs_beside = on);
                                }),
                            }),
                    ),
                ),
            )
            .into_element()
    }
}

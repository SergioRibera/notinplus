//! Ajustes › Notas.

use freya::prelude::*;
use notein_ui_kit::{Group, ListRow, Section, Segmented, Text, TextRole, Trailing};

use crate::settings::SettingsStore;

/// The Notas section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Notes;

impl Component for Notes {
    fn render(&self) -> impl IntoElement {
        let store = SettingsStore::get();
        let notes = store.read().notes;
        rect()
            .width(Size::fill())
            .spacing(24.0)
            .child(
                Section::new("Formato al escribir")
                    .child(
                        Segmented::new(
                            ["Como en un procesador de texto", "Mostrar marcas de markdown"],
                            usize::from(notes.show_markdown_marks),
                        )
                        .full_width()
                        .on_select(move |i: usize| {
                            let mut store = store;
                            store.update(|s| s.notes.show_markdown_marks = i == 1);
                        }),
                    )
                    .child(
                        Text::new(if notes.show_markdown_marks {
                            "Ves los ** y # cerca del cursor, como en un editor de markdown."
                        } else {
                            "Ves negritas y títulos ya formateados; las marcas quedan ocultas."
                        })
                        .role(TextRole::Hint),
                    ),
            )
            .child(
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
            )
            .into_element()
    }
}

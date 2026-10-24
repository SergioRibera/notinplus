//! Ajustes › Biblioteca: trash retention, copies in and out, space.

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Group, ListRow, ModalController, Section, Segmented, Text, TextRole, tokens};

use crate::flows::transfer::{self, ExportScope};
use crate::library::{ItemId, bodies};
use crate::settings::{SettingsStore, TrashRetention};
use crate::state::LibraryView;

/// Bytes on disk, by what holds them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Usage {
    documents: u64,
    pdfs: u64,
    trash: u64,
}

impl Usage {
    fn measure(live: &[ItemId], trashed: &[ItemId]) -> Self {
        let pdf = |id: ItemId| std::fs::metadata(bodies::pdf_path(id)).map_or(0, |m| m.len());
        let (documents, pdfs) = live.iter().fold((0, 0), |(docs, pdfs), id| {
            let pdf = pdf(*id);
            (
                docs + bodies::disk_usage(*id).saturating_sub(pdf),
                pdfs + pdf,
            )
        });
        Self {
            documents,
            pdfs,
            trash: trashed.iter().map(|id| bodies::disk_usage(*id)).sum(),
        }
    }

    const fn total(self) -> u64 {
        self.documents + self.pdfs + self.trash
    }
}

/// The Biblioteca section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Library;

impl Component for Library {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let store = SettingsStore::get();
        let retention = store.read().library.trash_retention;
        let mut usage = use_state(|| None::<Usage>);
        let data = LibraryView::get().data();
        use_hook({
            let data = data.clone();
            move || {
                let Some(data) = data else {
                    return;
                };
                let live: Vec<ItemId> = data.live_items().map(|i| i.id).collect();
                let trashed: Vec<ItemId> = data
                    .trash_entries()
                    .into_iter()
                    .filter_map(|e| match e.record {
                        crate::library::TrashedRecord::Item(item) => Some(item.id),
                        crate::library::TrashedRecord::Folder { .. } => None,
                    })
                    .collect();
                let (tx, rx) = flume::bounded(1);
                std::thread::spawn(move || {
                    let _ = tx.send(Usage::measure(&live, &trashed));
                });
                spawn(async move {
                    if let Ok(measured) = rx.recv_async().await {
                        usage.set(Some(measured));
                    }
                });
            }
        });
        let index = TrashRetention::ALL
            .iter()
            .position(|r| *r == retention)
            .unwrap_or(1);
        let trash_count = data
            .as_deref()
            .map_or(0, |d| crate::screens::trash::groups(d).len());
        let space = match *usage.read() {
            None => Text::new("Midiendo…").role(TextRole::Meta).into_element(),
            Some(u) => {
                let total = u.total().max(1);
                #[allow(clippy::cast_precision_loss)] // byte counts as proportions
                let share =
                    |n: u64| (n as f32 / total as f32 * 100.0).max(if n > 0 { 1.0 } else { 0.0 });
                let legend = |color: Color, label: &'static str, bytes: u64| {
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(6.0)
                        .child(
                            rect()
                                .width(Size::px(10.0))
                                .height(Size::px(10.0))
                                .corner_radius(3.0)
                                .background(color),
                        )
                        .child(
                            Text::new(format!("{label} {}", crate::format::size(bytes)))
                                .role(TextRole::Meta),
                        )
                };
                rect()
                    .width(Size::fill())
                    .spacing(10.0)
                    .child(Text::new(crate::format::size(u.total())).role(TextRole::Title))
                    .child(
                        rect()
                            .width(Size::fill())
                            .height(Size::px(10.0))
                            .corner_radius(tokens::radius::FULL)
                            .overflow(Overflow::Clip)
                            .background(p.s3)
                            .horizontal()
                            .child(
                                rect()
                                    .width(Size::percent(share(u.documents)))
                                    .height(Size::fill())
                                    .background(p.accent),
                            )
                            .child(
                                rect()
                                    .width(Size::percent(share(u.pdfs)))
                                    .height(Size::fill())
                                    .background(p.kinds.pdf),
                            )
                            .child(
                                rect()
                                    .width(Size::percent(share(u.trash)))
                                    .height(Size::fill())
                                    .background(p.t3),
                            ),
                    )
                    .child(
                        rect()
                            .horizontal()
                            .content(Content::wrap_spacing(8.0))
                            .spacing(14.0)
                            .child(legend(p.accent, "Documentos", u.documents))
                            .child(legend(p.kinds.pdf, "PDF originales", u.pdfs))
                            .child(legend(p.t3, "Papelera", u.trash)),
                    )
                    .into_element()
            }
        };
        rect()
            .width(Size::fill())
            .spacing(24.0)
            .child(
                Section::new("Papelera")
                    .child(
                        Text::new("Cuánto tiempo se guarda lo que borrás antes de borrarse solo.")
                            .role(TextRole::Hint),
                    )
                    .child(
                        Segmented::new(TrashRetention::ALL.map(TrashRetention::label), index)
                            .full_width()
                            .on_select(move |i: usize| {
                                let mut store = store;
                                store
                                    .update(|s| s.library.trash_retention = TrashRetention::ALL[i]);
                            }),
                    )
                    .child(
                        Group::default().child(
                            ListRow::new("Ver papelera")
                                .subtitle(crate::format::count(
                                    trash_count,
                                    "elemento",
                                    "elementos",
                                ))
                                .icon(lucide::trash_2())
                                .chevron()
                                .on_press(|_| {
                                    ModalController::get().close();
                                    crate::screens::trash::open();
                                }),
                        ),
                    ),
            )
            .child(
                Section::new("Copias").child(
                    Group::default()
                        .child(
                            ListRow::new("Exportar…")
                                .subtitle(
                                    "Una copia de seguridad o archivos para leer en otras apps.",
                                )
                                .icon(lucide::download())
                                .chevron()
                                .on_press(|_| transfer::export(ExportScope::Library)),
                        )
                        .child(
                            ListRow::new("Importar…")
                                .subtitle("Desde una copia .notein de este u otro dispositivo.")
                                .icon(lucide::upload())
                                .chevron()
                                .on_press(|_| transfer::import()),
                        ),
                ),
            )
            .child(Section::new("Espacio usado").child(space))
            .into_element()
    }
}

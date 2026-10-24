//! Ajustes › Biblioteca: trash retention, copies in and out, space.

use std::sync::Arc;

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Group, ListRow, ModalController, Section, Segmented, Text, TextRole, tokens};

use crate::flows::transfer::{self, ExportScope};
use crate::library::{ItemId, LibraryData, bodies};
use crate::settings::{SettingsStore, TrashRetention};
use crate::state::LibraryView;

/// Bytes on disk, by what holds them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Usage {
    documents: u64,
    pdfs: u64,
    trash: u64,
    /// The disk the library lives on; `None` when the system would not
    /// say.
    disk: Option<Disk>,
}

/// Size and free space of a disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Disk {
    total: u64,
    available: u64,
}

impl Disk {
    fn of(path: &std::path::Path) -> Option<Self> {
        fs4::statvfs(path)
            .inspect_err(|err| log::warn!("disk space of {}: {err}", path.display()))
            .ok()
            .map(|stats| Self {
                total: stats.total_space(),
                available: stats.available_space(),
            })
            .filter(|disk| disk.total > 0)
    }
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
            disk: Disk::of(&bodies::root_dir()),
        }
    }

    /// What the rest of the disk's contents take: other apps and files.
    fn others(self) -> u64 {
        self.disk.map_or(0, |d| {
            d.total
                .saturating_sub(d.available)
                .saturating_sub(self.total())
        })
    }

    const fn total(self) -> u64 {
        self.documents + self.pdfs + self.trash
    }
}

/// The Biblioteca section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Library;

/// Measure the space the library takes, off the UI thread, once.
fn use_usage(data: Option<Arc<LibraryData>>) -> State<Option<Usage>> {
    let mut usage = use_state(|| None::<Usage>);
    use_hook(move || {
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
            // The screen may be gone by then; nothing waits for it.
            let _ = tx.send(Usage::measure(&live, &trashed));
        });
        spawn(async move {
            if let Ok(measured) = rx.recv_async().await {
                usage.set(Some(measured));
            }
        });
    });
    usage
}

/// What Notein takes, against the whole disk it lives on: a bar split
/// by what holds the space, the rest of the disk's contents in a
/// quieter tone and the free part left empty.
fn usage_view(u: Usage) -> Element {
    let p = palette();
    let whole = u.disk.map_or_else(|| u.total(), |d| d.total).max(1);
    #[allow(clippy::cast_precision_loss)] // byte counts as proportions
    let share = |n: u64| (n as f32 / whole as f32 * 100.0).max(if n > 0 { 0.8 } else { 0.0 });
    let part = |bytes: u64, color: Color| {
        rect()
            .width(Size::percent(share(bytes)))
            .height(Size::fill())
            .background(color)
    };
    let legend = |color: Color, label: String| {
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
            .child(Text::new(label).role(TextRole::Meta))
    };
    let named = |label: &str, bytes: u64| format!("{label} {}", crate::format::size(bytes));
    let others = p.t3.with_a(120);
    let summary = u.disk.map_or_else(
        || "En este dispositivo".to_owned(),
        |d| {
            format!(
                "{} libres de {}",
                crate::format::size(d.available),
                crate::format::size(d.total)
            )
        },
    );
    rect()
        .width(Size::fill())
        .spacing(10.0)
        .child(
            rect()
                .width(Size::fill())
                .horizontal()
                .cross_align(Alignment::End)
                .content(Content::Flex)
                .spacing(8.0)
                .child(
                    Text::new(format!("{} de Notein", crate::format::size(u.total())))
                        .role(TextRole::Title)
                        .fill(),
                )
                .child(Text::new(summary).role(TextRole::Meta)),
        )
        .child(
            rect()
                .width(Size::fill())
                .height(Size::px(10.0))
                .corner_radius(tokens::radius::FULL)
                .overflow(Overflow::Clip)
                .background(p.s3)
                .horizontal()
                .child(part(u.documents, p.accent))
                .child(part(u.pdfs, p.kinds.pdf))
                .child(part(u.trash, p.warn))
                .child(part(u.others(), others)),
        )
        .child(
            rect()
                .horizontal()
                .content(Content::wrap_spacing(8.0))
                .spacing(14.0)
                .child(legend(p.accent, named("Documentos", u.documents)))
                .child(legend(p.kinds.pdf, named("PDF originales", u.pdfs)))
                .child(legend(p.warn, named("Papelera", u.trash)))
                .maybe_child(u.disk.is_some().then(|| {
                    legend(others, named("Otras apps y archivos", u.others()))
                }))
                .maybe_child(u.disk.map(|d| legend(p.s3, named("Libre", d.available)))),
        )
        .into_element()
}

/// How long the trash keeps things, and the way to it.
fn trash_section(retention: TrashRetention, trash_count: usize) -> Section {
    let index = TrashRetention::ALL
        .iter()
        .position(|r| *r == retention)
        .unwrap_or(1);
    Section::new("Papelera")
        .child(
            Text::new("Cuánto tiempo se guarda lo que borrás antes de borrarse solo.")
                .role(TextRole::Hint),
        )
        .child(
            Segmented::new(TrashRetention::ALL.map(TrashRetention::label), index)
                .full_width()
                .on_select(move |i: usize| {
                    SettingsStore::get().update(|s| s.library.trash_retention = TrashRetention::ALL[i]);
                }),
        )
        .child(
            Group::default().child(
                ListRow::new("Ver papelera")
                    .subtitle(crate::format::count(trash_count, "elemento", "elementos"))
                    .icon(freya::icons::lucide::trash_2())
                    .chevron()
                    .on_press(|_| {
                        ModalController::get().close();
                        crate::screens::trash::open();
                    }),
            ),
        )
}

impl Component for Library {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let retention = SettingsStore::get().read().library.trash_retention;
        let data = LibraryView::get().data();
        let trash_count = data
            .as_deref()
            .map_or(0, |d| crate::screens::trash::groups(d).len());
        let usage = use_usage(data);
        let space = (*usage.read()).map_or_else(
            || Text::new("Midiendo…").role(TextRole::Meta).into_element(),
            usage_view,
        );
        rect()
            .width(Size::fill())
            .spacing(24.0)
            .child(trash_section(retention, trash_count))
            .child(
                Section::new("Copias").child(
                    Group::default()
                        .child(
                            ListRow::new("Exportar…")
                                .subtitle("Una copia de seguridad o archivos para leer en otras apps.")
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

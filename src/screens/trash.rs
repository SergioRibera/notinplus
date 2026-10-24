//! The trash: what was deleted, from where, how long it stays, and the
//! way back.

use std::sync::Arc;

use freya::prelude::*;
use freya::router::RouterContext;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    BarItem, Button, EmptyState, IconButton, Leading, ListRow, SelectionBar, Text, TextRole, Toast,
    Toasts, Trailing, tokens, width_class,
};

use crate::library::{LibraryData, TrashEntry, TrashedRecord, days_left};
use crate::route::Route;
use crate::settings::SettingsStore;
use crate::state::LibraryView;

/// Everything one action sent to the trash; it restores together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashGroup {
    pub batch: u64,
    pub entries: Vec<TrashEntry>,
}

impl TrashGroup {
    fn at(&self) -> u64 {
        self.entries.first().map_or(0, |e| e.trashed.at)
    }

    fn title(&self) -> String {
        let first = self.entries.first().map_or("", TrashEntry::name);
        match self.entries.len() {
            0 | 1 => first.to_owned(),
            n => format!("{first} y {} más", n - 1),
        }
    }

    fn is_folder(&self) -> bool {
        self.entries
            .first()
            .is_some_and(|e| matches!(e.record, TrashedRecord::Folder { .. }))
    }

    fn kind(&self) -> Option<String> {
        self.entries.first().and_then(|e| match &e.record {
            TrashedRecord::Item(item) => Some(item.kind.plugin_id.clone()),
            TrashedRecord::Folder { .. } => None,
        })
    }

    /// Documents in the group, counting folder contents.
    fn documents(&self) -> usize {
        self.entries
            .iter()
            .map(|e| match &e.record {
                TrashedRecord::Item(_) => 1,
                TrashedRecord::Folder { items, .. } => *items,
            })
            .sum()
    }
}

/// Trash entries grouped by the action that trashed them, newest first.
#[must_use]
pub fn groups(data: &LibraryData) -> Vec<TrashGroup> {
    let mut out: Vec<TrashGroup> = Vec::new();
    for entry in data.trash_entries() {
        match out.iter_mut().find(|g| g.batch == entry.trashed.batch) {
            Some(group) => group.entries.push(entry),
            None => out.push(TrashGroup {
                batch: entry.trashed.batch,
                entries: vec![entry],
            }),
        }
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.at()));
    out
}

/// Open the trash.
pub fn open() {
    let _ = RouterContext::get().push(Route::Trash);
}

fn restore(batches: Vec<u64>) {
    crate::state::edit("restaurar", async move |lib| {
        let mut restored = 0;
        for batch in batches {
            restored += lib.restore_batch(batch).await?;
        }
        Toasts::get().show(Toast::new(format!(
            "{} de vuelta en la biblioteca",
            crate::format::count(restored, "documento", "documentos")
        )));
        Ok(())
    });
}

fn purge(batches: Vec<u64>) {
    crate::state::edit("borrar para siempre", async move |lib| {
        for batch in &batches {
            lib.purge_batch(*batch).await?;
        }
        Toasts::get().show(Toast::new("Borrado para siempre"));
        Ok(())
    });
}

/// The trash route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trash;

impl Component for Trash {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let insets = *crate::hooks::use_safe_area_insets().read();
        let data = LibraryView::get().data();
        let pad = if width_class().is_compact() {
            tokens::space::GUTTER_COMPACT
        } else {
            tokens::space::GUTTER
        };
        rect()
            .expanded()
            .background(p.bg)
            .padding((insets.top, insets.right, insets.bottom, insets.left))
            .on_global_key_down(|e: Event<KeyboardEventData>| {
                if e.key == Key::Named(NamedKey::Escape)
                    && !notein_ui_kit::ModalController::get().is_open()
                {
                    RouterContext::get().go_back();
                }
            })
            .maybe_child(data.map(|data| TrashList { data, pad }))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct TrashList {
    data: Arc<LibraryData>,
    pad: f32,
}

impl Component for TrashList {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let retention = SettingsStore::get().read().library.trash_retention;
        let mut selecting = use_state(|| false);
        let mut picked = use_state(Vec::<u64>::new);
        let mut confirming = use_state(|| false);
        let now = crate::format::now();
        let groups = groups(&self.data);
        let total: usize = groups.iter().map(TrashGroup::documents).sum();
        let size = crate::format::size(self.data.trash_size());
        let all_batches: Vec<u64> = groups.iter().map(|g| g.batch).collect();

        let header = rect()
            .width(Size::fill())
            .height(Size::px(tokens::size::BAR + 8.0))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(8.0)
            .child(
                IconButton::new(lucide::arrow_left(), "Volver")
                    .on_press(|_| RouterContext::get().go_back()),
            )
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .child(Text::new("Papelera").role(TextRole::Heading))
                    .child(
                        Text::new(retention.days().map_or_else(
                            || {
                                format!(
                                    "{} · {size} · se guardan hasta que los borres",
                                    crate::format::count(total, "documento", "documentos")
                                )
                            },
                            |days| {
                                format!(
                                    "{} · {size} · se borran solos a los {days} días",
                                    crate::format::count(total, "documento", "documentos")
                                )
                            },
                        ))
                        .role(TextRole::Meta),
                    ),
            )
            .maybe_child((!groups.is_empty()).then(|| {
                Button::new(if selecting() { "Listo" } else { "Seleccionar" })
                    .ghost()
                    .small()
                    .on_press(move |_| {
                        let next = !selecting();
                        selecting.set(next);
                        picked.write().clear();
                    })
            }));

        if groups.is_empty() {
            return rect()
                .expanded()
                .padding((0.0, self.pad))
                .child(header)
                .child(
                    EmptyState::new(lucide::trash_2(), "La papelera está vacía")
                        .body("Lo que borres queda acá un tiempo, por si cambiás de idea."),
                )
                .into_element();
        }

        let chosen = picked.read().clone();
        let rows: Vec<Element> = groups
            .iter()
            .map(|group| {
                let batch = group.batch;
                let from = group
                    .entries
                    .first()
                    .map(|e| crate::flows::forms::folder_path(&self.data, e.trashed.from))
                    .unwrap_or_default();
                let left =
                    days_left(group.at(), now, retention.millis()).map_or_else(String::new, |d| {
                        format!(
                            " · {}",
                            crate::format::count(
                                usize::try_from(d).unwrap_or(0),
                                "día más",
                                "días más"
                            )
                        )
                    });
                let subtitle = format!(
                    "Borrado {} · de {from}{left}",
                    crate::format::ago(group.at(), now)
                );
                let row = ListRow::new(group.title()).subtitle(subtitle);
                let row = if selecting() {
                    let on = chosen.contains(&batch);
                    row.leading(Leading::Check(on))
                        .selected(on)
                        .on_press(move |_| {
                            let mut picked = picked.write();
                            if let Some(at) = picked.iter().position(|b| *b == batch) {
                                picked.remove(at);
                            } else {
                                picked.push(batch);
                            }
                        })
                } else {
                    let row = if group.is_folder() {
                        row.kind(lucide::folder(), p.kinds.folder)
                    } else {
                        let info = group.kind().and_then(|k| crate::plugins::kind_info(&k));
                        row.kind(
                            info.as_ref().map_or_else(lucide::file, |k| k.icon.clone()),
                            info.map_or(p.t3, |k| crate::references::kind_color(&p, k.color)),
                        )
                    };
                    row.trailing(Trailing::Element(
                        Button::new("Restaurar")
                            .tonal()
                            .small()
                            .on_press(move |_| restore(vec![batch]))
                            .into_element(),
                    ))
                };
                rect().key(batch).child(row).into_element()
            })
            .collect();

        let empty_all = {
            let batches = all_batches;
            if confirming() {
                rect()
                    .width(Size::fill())
                    .padding(12.0)
                    .corner_radius(tokens::radius::LG)
                    .background(p.s1)
                    .spacing(10.0)
                    .child(
                        Text::new(
                            "No se puede deshacer. También se borran en tus otros dispositivos.",
                        )
                        .role(TextRole::Hint),
                    )
                    .child(
                        rect()
                            .horizontal()
                            .spacing(8.0)
                            .child(
                                Button::new("Cancelar")
                                    .ghost()
                                    .on_press(move |_| confirming.set(false)),
                            )
                            .child(
                                Button::new(format!(
                                    "Borrar {} para siempre",
                                    crate::format::count(total, "documento", "documentos")
                                ))
                                .variant(notein_ui_kit::ButtonVariant::DangerFill)
                                .on_press(move |_| {
                                    confirming.set(false);
                                    purge(batches.clone());
                                }),
                            ),
                    )
                    .into_element()
            } else {
                Button::new("Vaciar papelera")
                    .danger()
                    .icon(lucide::trash_2())
                    .on_press(move |_| confirming.set(true))
                    .into_element()
            }
        };

        let bar = (selecting() && !chosen.is_empty()).then(|| {
            let (to_restore, to_purge) = (chosen.clone(), chosen.clone());
            rect()
                .position(Position::new_absolute().left(0.0).bottom(16.0))
                .width(Size::fill())
                .horizontal()
                .main_align(Alignment::Center)
                .child(SelectionBar::new([
                    BarItem::Caption(
                        crate::format::count(chosen.len(), "elegido", "elegidos").into(),
                    ),
                    BarItem::Divider,
                    BarItem::action("Restaurar", lucide::undo_2(), move |()| {
                        restore(to_restore.clone());
                        selecting.set(false);
                    }),
                    BarItem::action("Borrar para siempre", lucide::trash_2(), move |()| {
                        purge(to_purge.clone());
                        selecting.set(false);
                    }),
                ]))
        });

        rect()
            .expanded()
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::fill())
                    .child(
                        rect()
                            .width(Size::fill())
                            .padding((0.0, self.pad, 120.0, self.pad))
                            .spacing(16.0)
                            .child(header)
                            .child(notein_ui_kit::Group::default().children(rows))
                            .child(empty_all),
                    ),
            )
            .maybe_child(bar)
            .into_element()
    }
}

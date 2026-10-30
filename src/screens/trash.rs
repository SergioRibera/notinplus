//! The trash: what was deleted, from where, how long it stays, and the
//! way back.

use std::sync::Arc;

use freya::prelude::*;
use freya::router::RouterContext;
use notein_i18n::tr;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    BarItem, Button, EmptyState, IconButton, Leading, ListRow, SelectionBar, Text, TextRole, Toast,
    Toasts, Trailing, tokens, width_class,
};

use crate::library::{LibraryData, TrashEntry, TrashedRecord, days_left};
use crate::route::Route;
use crate::settings::{SettingsStore, TrashRetention};
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
            n => tr!("trash-group-title", "first" => first, "more" => n - 1),
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
    crate::state::edit(crate::state::LibraryEdit::Restore, async move |lib| {
        let mut restored = 0;
        for batch in batches {
            restored += lib.restore_batch(batch).await?;
        }
        Toasts::get().show(Toast::new(tr!("trash-restored", "count" => restored)));
        Ok(())
    });
}

fn purge(batches: Vec<u64>) {
    crate::state::edit(crate::state::LibraryEdit::DeleteForever, async move |lib| {
        for batch in &batches {
            lib.purge_batch(*batch).await?;
        }
        Toasts::get().show(Toast::new(tr!("trash-purged")));
        Ok(())
    });
}

/// The trash route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trash;

impl Component for Trash {
    fn render(&self) -> impl IntoElement {
        use_hook(|| sismo::marker("screen: trash"));
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
        let retention = SettingsStore::get().read().library.trash_retention;
        let ui = TrashUi {
            selecting: use_state(|| false),
            picked: use_state(Vec::<u64>::new),
            confirming: use_state(|| false),
        };
        let groups = groups(&self.data);
        let total: usize = groups.iter().map(TrashGroup::documents).sum();
        let header = ui.header(
            retention,
            total,
            &crate::format::size(self.data.trash_size()),
            !groups.is_empty(),
        );
        if groups.is_empty() {
            return rect()
                .expanded()
                .padding((0.0, self.pad))
                .child(header)
                .child(
                    EmptyState::new(freya::icons::lucide::trash_2(), tr!("trash-empty-title"))
                        .body(tr!("trash-empty-body")),
                )
                .into_element();
        }
        let now = crate::format::now();
        let rows: Vec<Element> = groups
            .iter()
            .map(|group| ui.row(&self.data, group, now, retention))
            .collect();
        let all_batches: Vec<u64> = groups.iter().map(|g| g.batch).collect();
        rect()
            .expanded()
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::fill())
                    .child(notein_ui_kit::ScrollContent::new(
                        rect()
                            .width(Size::fill())
                            .padding((0.0, self.pad, 120.0, self.pad))
                            .spacing(16.0)
                            .child(header)
                            .child(notein_ui_kit::Group::default().children(rows))
                            .child(ui.empty_all(all_batches, total)),
                    )),
            )
            .maybe_child(ui.selection_bar())
            .into_element()
    }
}

/// The list's own state: choosing groups, and confirming the purge.
#[derive(Clone, Copy)]
struct TrashUi {
    selecting: State<bool>,
    picked: State<Vec<u64>>,
    confirming: State<bool>,
}

impl TrashUi {
    /// Back, the title, how much there is and how long it stays.
    fn header(self, retention: TrashRetention, total: usize, size: &str, any: bool) -> Rect {
        let (mut selecting, mut picked) = (self.selecting, self.picked);
        let summary = retention.days().map_or_else(
            || tr!("trash-summary-kept", "count" => total, "size" => size),
            |days| tr!("trash-summary-days", "count" => total, "size" => size, "days" => days),
        );
        rect()
            .width(Size::fill())
            .height(Size::px(tokens::size::BAR + 8.0))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(8.0)
            .child(
                IconButton::new(freya::icons::lucide::arrow_left(), tr!("trash-back"))
                    .on_press(|_| RouterContext::get().go_back()),
            )
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .child(Text::new(tr!("common-trash")).role(TextRole::Heading))
                    .child(Text::new(summary).role(TextRole::Meta)),
            )
            .maybe_child(any.then(|| {
                Button::new(if selecting() {
                    tr!("common-done")
                } else {
                    tr!("trash-select")
                })
                .ghost()
                .small()
                .on_press(move |_| {
                    let next = !selecting();
                    selecting.set(next);
                    picked.write().clear();
                })
            }))
    }

    /// One group: what it was, where from, how long it has left, and
    /// Restaurar (or a check while choosing).
    fn row(
        self,
        data: &LibraryData,
        group: &TrashGroup,
        now: u64,
        retention: TrashRetention,
    ) -> Element {
        use freya::icons::lucide;
        let p = palette();
        let mut picked = self.picked;
        let batch = group.batch;
        let from = group
            .entries
            .first()
            .map(|e| crate::flows::forms::folder_path(data, e.trashed.from))
            .unwrap_or_default();
        let ago = crate::format::ago(group.at(), now);
        let meta = days_left(group.at(), now, retention.millis()).map_or_else(
            || tr!("trash-row-meta", "ago" => ago.clone(), "from" => from.clone()),
            |days| tr!("trash-row-meta-left", "ago" => ago.clone(), "from" => from.clone(), "days" => days),
        );
        let row = ListRow::new(group.title()).subtitle(meta);
        let row = if *self.selecting.read() {
            let on = picked.read().contains(&batch);
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
                Button::new(tr!("trash-restore"))
                    .tonal()
                    .small()
                    .on_press(move |_| restore(vec![batch]))
                    .into_element(),
            ))
        };
        rect().key(batch).child(row).into_element()
    }

    /// "Vaciar papelera", confirmed in place.
    fn empty_all(self, batches: Vec<u64>, total: usize) -> Element {
        let mut confirming = self.confirming;
        if !confirming() {
            return Button::new(tr!("trash-empty-all"))
                .danger()
                .icon(freya::icons::lucide::trash_2())
                .on_press(move |_| confirming.set(true))
                .into_element();
        }
        rect()
            .width(Size::fill())
            .padding(12.0)
            .corner_radius(tokens::radius::LG)
            .background(palette().s1)
            .spacing(10.0)
            .child(Text::new(tr!("trash-empty-all-warning")).role(TextRole::Hint))
            .child(
                rect()
                    .horizontal()
                    .spacing(8.0)
                    .child(
                        Button::new(tr!("common-cancel"))
                            .ghost()
                            .on_press(move |_| confirming.set(false)),
                    )
                    .child(
                        Button::new(tr!("trash-purge-count", "count" => total))
                            .variant(notein_ui_kit::ButtonVariant::DangerFill)
                            .on_press(move |_| {
                                confirming.set(false);
                                purge(batches.clone());
                            }),
                    ),
            )
            .into_element()
    }

    /// Restore or purge the chosen groups.
    fn selection_bar(self) -> Option<Rect> {
        use freya::icons::lucide;
        let mut selecting = self.selecting;
        let chosen = self.picked.read().clone();
        (*selecting.read() && !chosen.is_empty()).then(|| {
            let (to_restore, to_purge) = (chosen.clone(), chosen.clone());
            rect()
                .position(Position::new_absolute().left(0.0).bottom(16.0))
                .width(Size::fill())
                .horizontal()
                .main_align(Alignment::Center)
                .child(SelectionBar::new([
                    BarItem::Caption(tr!("trash-chosen-count", "count" => chosen.len()).into()),
                    BarItem::Divider,
                    BarItem::action(tr!("trash-restore"), lucide::undo_2(), move |()| {
                        restore(to_restore.clone());
                        selecting.set(false);
                    }),
                    BarItem::action(tr!("trash-purge"), lucide::trash_2(), move |()| {
                        purge(to_purge.clone());
                        selecting.set(false);
                    }),
                ]))
        })
    }
}

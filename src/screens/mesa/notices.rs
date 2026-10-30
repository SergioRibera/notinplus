//! Strips under a pane's header that tell what is going on with its
//! document, never blocking it: offline, deleted elsewhere, read only.

use std::sync::Arc;

use freya::prelude::*;
use notein_i18n::tr;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Button, Icon, Text, TextRole, tokens};

use super::Ops;
use crate::library::{LibraryData, PaneId, PaneTab, TabTarget, days_left};
use crate::settings::SettingsStore;

/// What a notice says and the actions it offers.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Notice {
    Offline,
    Trashed {
        name: String,
        days: Option<u64>,
        batch: u64,
    },
    Gone {
        name: String,
    },
    ReadOnly {
        owner: String,
    },
}

impl Notice {
    fn for_tab(tab: &PaneTab, data: &LibraryData) -> Option<Self> {
        let TabTarget::Item(id) = tab.target else {
            return None;
        };
        let Some(item) = data.item(id) else {
            return Some(Self::Gone {
                name: tr!("mesa-notice-this-document"),
            });
        };
        if let Some(trashed) = item.trashed {
            let retention = SettingsStore::get().read().library.trash_retention.millis();
            return Some(Self::Trashed {
                name: item.name.clone(),
                days: days_left(trashed.at, crate::format::now(), retention),
                batch: trashed.batch,
            });
        }
        let share = crate::sync::share_of(id)?;
        if !share.can_edit {
            return Some(Self::ReadOnly {
                owner: share.owner.name,
            });
        }
        // A shared document travels with the session: it is offline when
        // nobody else in it is reachable, whatever this user's own devices do.
        let snapshot = crate::sync::snapshot();
        let offline = !share.anyone_here(snapshot.device) && !snapshot.relay_online;
        offline.then_some(Self::Offline)
    }
}

/// The notice of the active tab of `pane`, if any.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TabNotice {
    pub(super) data: Arc<LibraryData>,
    pub(super) pane: PaneId,
    pub(super) index: usize,
    pub(super) tab: PaneTab,
    pub(super) ops: Ops,
}

impl Component for TabNotice {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let _sync = crate::sync::use_sync().read().online.len();
        let Some(notice) = Notice::for_tab(&self.tab, &self.data) else {
            return rect().into_element();
        };
        let (pane, index, ops) = (self.pane, self.index, self.ops);
        let close_tab = self.tab.clone();
        let close = move || ops.close_tab(pane, index, &close_tab);
        let (icon, tone, text, actions): (Bytes, Color, String, Vec<Element>) = match notice {
            Notice::Offline => (
                lucide::wifi_off(),
                p.warn,
                tr!("mesa-notice-offline"),
                Vec::new(),
            ),
            Notice::ReadOnly { owner } => (
                lucide::eye(),
                p.accent,
                tr!("mesa-notice-read-only", "owner" => owner),
                Vec::new(),
            ),
            Notice::Trashed { name, days, batch } => (
                lucide::trash_2(),
                p.danger,
                days.map_or_else(
                    || tr!("mesa-notice-trashed", "name" => name.clone()),
                    |days| tr!("mesa-notice-trashed-days", "name" => name.clone(), "days" => days),
                ),
                vec![
                    Button::new(tr!("mesa-notice-restore"))
                        .tonal()
                        .small()
                        .on_press(move |_| {
                            crate::state::edit(
                                crate::state::LibraryEdit::Restore,
                                async move |lib| lib.restore_batch(batch).await.map(drop),
                            );
                        })
                        .into_element(),
                    Button::new(tr!("mesa-tab-close"))
                        .ghost()
                        .small()
                        .on_press(move |_| close())
                        .into_element(),
                ],
            ),
            Notice::Gone { name } => (
                lucide::file_x(),
                p.danger,
                tr!("mesa-notice-gone", "name" => name),
                vec![
                    Button::new(tr!("mesa-tab-close"))
                        .ghost()
                        .small()
                        .on_press(move |_| close())
                        .into_element(),
                ],
            ),
        };
        rect()
            .width(Size::fill())
            .padding((8.0, 12.0))
            .background(tone.with_a(28))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(10.0)
            .a11y_role(AccessibilityRole::Status)
            .child(Icon::new(icon).size(tokens::size::ICON_SM).color(tone))
            .child(Text::new(text).role(TextRole::Meta).color(p.t1).fill())
            .children(actions)
            .into_element()
    }
}

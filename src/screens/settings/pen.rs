//! Settings › Pen.

use freya::prelude::*;
use notin_ui_kit::theme::palette;
use notin_ui_kit::{
    Action, ActionEntry, ActionList, Button, Group, Icon, KindBadge, ListRow, ModalController,
    Section, Segmented, Sheet, Text, TextRole, Trailing, tokens,
};

use notin_i18n::tr;

use crate::references::kind_color;
use crate::settings::{EntryKey, FingerDraw, PenDoubleTap, SettingsStore};

/// Pick what shortcut `slot` creates, in a sheet over Settings.
#[sismo::function("settings::pen_pick_shortcut")]
fn pick_shortcut(slot: usize, current: Option<&EntryKey>) {
    let p = palette();
    let choose = move |key: Option<EntryKey>| {
        move |()| {
            ModalController::get().close();
            let key = key.clone();
            SettingsStore::get().update(|s| s.pen.fab_shortcuts[slot] = key);
        }
    };
    let mut entries = vec![ActionEntry::Item(
        Action::new(tr!("settings-pen-shortcut-none"), choose(None))
            .icon(freya::icons::lucide::circle_off())
            .checked(current.is_none()),
    )];
    entries.extend(
        crate::plugins::catalog()
            .entries()
            .into_iter()
            .map(|entry| {
                let key = EntryKey::new(entry.plugin.clone(), entry.entry.id.to_string());
                let chosen = current == Some(&key);
                ActionEntry::Item(
                    Action::new(entry.entry.label.clone(), choose(Some(key)))
                        .subtitle(entry.entry.subtitle.clone())
                        .icon(entry.entry.icon.clone())
                        .kind_color(kind_color(&p, entry.entry.color))
                        .checked(chosen),
                )
            }),
    );
    Sheet::titled(slot_title(slot))
        .subtitle(tr!("settings-pen-shortcut-pick-hint"))
        .on_back(|()| ModalController::get().close())
        .without_close()
        .width(420.0)
        .child(ActionList::new(entries))
        .push();
}

#[sismo::function("settings::pen_slot_title")]
fn slot_title(slot: usize) -> String {
    tr!("settings-pen-shortcut-slot", "slot" => slot + 1)
}

/// The Pen section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Pen;

/// What shortcut bubble `slot` creates.
#[sismo::function("settings::pen_shortcut_row")]
fn shortcut_row(slot: usize, current: Option<EntryKey>) -> Element {
    let p = palette();
    let entry = current.as_ref().and_then(crate::flows::create::entry_for);
    let value = entry.as_ref().map_or_else(
        || tr!("settings-pen-shortcut-none"),
        |e| e.entry.label.to_string(),
    );
    let badge = entry.as_ref().map_or_else(
        || {
            Icon::new(freya::icons::lucide::circle_plus())
                .size(18.0)
                .color(p.t3)
                .into_element()
        },
        |e| KindBadge::new(e.entry.icon.clone(), kind_color(&p, e.entry.color)).into_element(),
    );
    ListRow::new(slot_title(slot))
        .subtitle(value)
        .trailing(Trailing::Element(
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(10.0)
                .child(badge)
                .child(
                    Button::new(tr!("settings-pen-shortcut-change"))
                        .small()
                        .on_press(move |_| {
                            pick_shortcut(slot, current.as_ref());
                        }),
                )
                .into_element(),
        ))
        .into_element()
}

/// A section with a choice and a hint under it.
#[sismo::function("settings::pen_choice_section")]
fn choice_section<const N: usize>(
    title: impl Into<String>,
    labels: [String; N],
    selected: usize,
    hint: impl Into<String>,
    on_select: impl FnMut(usize) + 'static,
) -> Section {
    Section::new(title.into())
        .child(
            Segmented::new(labels, selected)
                .full_width()
                .on_select(on_select),
        )
        .child(Text::new(hint.into()).role(TextRole::Hint))
}

impl Component for Pen {
    #[sismo::function("settings::pen")]
    fn render(&self) -> impl IntoElement {
        let pen = SettingsStore::get().read().pen;
        let finger_index = FingerDraw::ALL
            .iter()
            .position(|f| *f == pen.finger_draws)
            .unwrap_or(0);
        let tap_index = PenDoubleTap::ALL
            .iter()
            .position(|t| *t == pen.double_tap)
            .unwrap_or(0);
        rect()
            .width(Size::fill())
            .spacing(24.0)
            .child(choice_section(
                tr!("settings-pen-finger"),
                FingerDraw::ALL.map(FingerDraw::label),
                finger_index,
                tr!("settings-pen-finger-hint"),
                |i| SettingsStore::get().update(|s| s.pen.finger_draws = FingerDraw::ALL[i]),
            ))
            .child(
                Group::default().child(
                    ListRow::new(tr!("settings-pen-palm"))
                        .subtitle(tr!("settings-pen-palm-hint"))
                        .trailing(Trailing::Switch {
                            on: pen.palm_rejection,
                            on_toggle: EventHandler::new(|on| {
                                SettingsStore::get().update(|s| s.pen.palm_rejection = on);
                            }),
                        }),
                ),
            )
            .child(choice_section(
                tr!("settings-pen-double-tap"),
                PenDoubleTap::ALL.map(PenDoubleTap::label),
                tap_index,
                tr!("settings-pen-double-tap-hint"),
                |i| SettingsStore::get().update(|s| s.pen.double_tap = PenDoubleTap::ALL[i]),
            ))
            .child(
                Section::new(tr!("settings-pen-shortcuts"))
                    .child(Text::new(tr!("settings-pen-shortcuts-hint")).role(TextRole::Hint))
                    .child(
                        Group::default()
                            .child(shortcut_row(0, pen.fab_shortcuts[0].clone()))
                            .child(shortcut_row(1, pen.fab_shortcuts[1].clone())),
                    ),
            )
            .child(rect().height(Size::px(tokens::space::SM)))
            .into_element()
    }
}

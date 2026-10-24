//! Ajustes › Lápiz y escritura.

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Action, ActionEntry, ActionList, Group, Leading, ListRow, Popover, PopoverAlign, PopoverSide,
    Section, Segmented, Text, TextRole, Trailing, tokens,
};

use crate::ink_glyph::GlyphFont;
use crate::references::kind_color;
use crate::settings::{EntryKey, FingerDraw, PenDoubleTap, SettingsStore};

fn anchor_of(e: &Event<PressEventData>) -> Area {
    let at = match e.data() {
        PressEventData::Mouse(m) => m.global_location,
        _ => CursorPoint::new(0.0, 0.0),
    };
    #[allow(clippy::cast_possible_truncation)] // window coordinates
    Area::new((at.x as f32, at.y as f32).into(), (1.0, 1.0).into())
}

/// Pick what shortcut `slot` creates.
fn pick_shortcut(slot: usize, current: Option<&EntryKey>, anchor: Area) {
    let p = palette();
    let mut entries = vec![ActionEntry::Item(
        Action::new("Ninguno", move |()| {
            notein_ui_kit::ModalController::get().close();
            let mut store = SettingsStore::get();
            store.update(|s| s.pen.fab_shortcuts[slot] = None);
        })
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
                    Action::new(entry.entry.label.clone(), move |()| {
                        notein_ui_kit::ModalController::get().close();
                        let key = key.clone();
                        let mut store = SettingsStore::get();
                        store.update(|s| s.pen.fab_shortcuts[slot] = Some(key));
                    })
                    .icon(entry.entry.icon.clone())
                    .kind_color(kind_color(&p, entry.entry.color))
                    .checked(chosen),
                )
            }),
    );
    Popover::new(anchor, ActionList::new(entries).width(260.0))
        .width(260.0)
        .side(PopoverSide::Below)
        .align(PopoverAlign::End)
        .sheet_title(if slot == 0 {
            "Atajo de arriba"
        } else {
            "Atajo de abajo"
        })
        .open();
}

/// The Lápiz y escritura section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Pen;

/// A font for smoothed handwriting, previewed in itself.
fn font_row(font: GlyphFont, chosen: bool) -> Element {
    let p = palette();
    ListRow::new(font.name())
        .leading(Leading::Element(
            rect()
                .width(Size::px(190.0))
                .overflow(Overflow::Clip)
                .child(
                    label()
                        .text("Hola, Romanos 8")
                        .text_overflow(TextOverflow::Ellipsis)
                        .font_family(font.name())
                        .font_size(20.0)
                        .color(p.t1)
                        .max_lines(1),
                )
                .into_element(),
        ))
        .selected(chosen)
        .trailing(if chosen {
            Trailing::Element(
                notein_ui_kit::Icon::new(freya::icons::lucide::check())
                    .size(16.0)
                    .color(p.accent)
                    .into_element(),
            )
        } else {
            Trailing::Text("".into())
        })
        .on_press(move |_| {
            font.select();
            SettingsStore::get().update(|s| s.pen.smooth_font = font);
        })
        .into_element()
}

/// What shortcut bubble `slot` creates.
fn shortcut_row(slot: usize, title: &'static str, current: Option<EntryKey>) -> Element {
    let p = palette();
    let entry = current.as_ref().and_then(crate::flows::create::entry_for);
    let value = entry
        .as_ref()
        .map_or_else(|| "Ninguno".to_owned(), |e| e.entry.label.to_string());
    let row = ListRow::new(title).trailing(Trailing::Text(value.into()));
    let row = match &entry {
        Some(e) => row.kind(e.entry.icon.clone(), kind_color(&p, e.entry.color)),
        None => row.icon(freya::icons::lucide::circle_plus()),
    };
    row.chevron()
        .on_press(move |e: Event<PressEventData>| {
            pick_shortcut(slot, current.as_ref(), anchor_of(&e));
        })
        .into_element()
}

/// A section with a choice and a hint under it.
fn choice_section<const N: usize>(
    title: &'static str,
    labels: [&'static str; N],
    selected: usize,
    hint: &'static str,
    on_select: impl FnMut(usize) + 'static,
) -> Section {
    Section::new(title)
        .child(Segmented::new(labels, selected).full_width().on_select(on_select))
        .child(Text::new(hint).role(TextRole::Hint))
}

impl Component for Pen {
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
                "Dibujar con el dedo",
                FingerDraw::ALL.map(FingerDraw::label),
                finger_index,
                "Automático: el dedo dibuja hasta que uses un lápiz en este dispositivo; después mueve la hoja.",
                |i| SettingsStore::get().update(|s| s.pen.finger_draws = FingerDraw::ALL[i]),
            ))
            .child(
                Group::default().child(
                    ListRow::new("Ignorar la palma")
                        .subtitle("Mientras el lápiz toca la pantalla, la mano no dibuja ni mueve.")
                        .trailing(Trailing::Switch {
                            on: pen.palm_rejection,
                            on_toggle: EventHandler::new(|on| {
                                SettingsStore::get().update(|s| s.pen.palm_rejection = on);
                            }),
                        }),
                ),
            )
            .child(choice_section(
                "Doble toque del lápiz",
                PenDoubleTap::ALL.map(PenDoubleTap::label),
                tap_index,
                "En lápices que lo permiten (Apple Pencil 2, S Pen, lápices con botón).",
                |i| SettingsStore::get().update(|s| s.pen.double_tap = PenDoubleTap::ALL[i]),
            ))
            .child(
                Section::new("Escritura suavizada")
                    .child(
                        Text::new("La letra con la que la herramienta de firma redibuja lo que escribís.")
                            .role(TextRole::Hint),
                    )
                    .child(Group::default().children(
                        GlyphFont::ALL.into_iter().map(|font| font_row(font, font == pen.smooth_font)),
                    )),
            )
            .child(
                Section::new("Atajos sobre el +")
                    .child(
                        Text::new("Dos burbujas encima del botón + para crear algo con un toque.")
                            .role(TextRole::Hint),
                    )
                    .child(
                        Group::default()
                            .child(shortcut_row(0, "Atajo de arriba", pen.fab_shortcuts[0].clone()))
                            .child(shortcut_row(1, "Atajo de abajo", pen.fab_shortcuts[1].clone())),
                    ),
            )
            .child(rect().height(Size::px(tokens::space::SM)))
            .into_element()
    }
}

//! Settings › Pen and handwriting.

use freya::prelude::*;
use freya_engine::prelude::{Color as SkColor, Paint, PaintStyle};
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Action, ActionEntry, ActionList, Button, ChoiceCard, Group, Icon, KindBadge, ListRow,
    ModalController, Section, Segmented, Sheet, Text, TextRole, Trailing, tokens,
};

use notein_i18n::tr;

use crate::ink_glyph::GlyphFont;
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

/// The Pen and handwriting section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Pen;

/// A sample phrase drawn with a font's own outlines: the fonts are not
/// installed as text fonts, only baked as ink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FontSample {
    font: GlyphFont,
    color: [u8; 3],
}

impl Component for FontSample {
    #[sismo::function("settings::font_sample")]
    fn render(&self) -> impl IntoElement {
        let Self {
            font,
            color: [r, g, b],
        } = *self;
        let sample = tr!("settings-pen-font-sample");
        rect()
            .background(RenderCallback::new(move |ctx| {
                let RenderTarget::Background { canvas, .. } = &ctx.target else {
                    return;
                };
                // Already clipped to the element and with its origin at 0, 0.
                let fit = |height| font.sample(&sample, (0.0, 0.0), height);
                let Some((mut path, width)) = fit(ctx.size.height) else {
                    return;
                };
                if width > ctx.size.width {
                    // Narrow cards: smaller, never cut.
                    if let Some((smaller, _)) = fit(ctx.size.height * ctx.size.width / width) {
                        path = smaller;
                    }
                }
                let mut paint = Paint::default();
                paint.set_color(SkColor::from_rgb(r, g, b));
                paint.set_style(PaintStyle::Fill);
                paint.set_anti_alias(true);
                canvas.draw_path(&path, &paint);
            }))
            .width(Size::fill())
            .height(Size::px(26.0))
    }
}

/// A font for smoothed handwriting, previewed in itself.
#[sismo::function("settings::pen_font_card")]
fn font_card(font: GlyphFont, chosen: bool) -> Element {
    let p = palette();
    let ink = if chosen { p.accent_text } else { p.t1 };
    ChoiceCard::new(
        FontSample {
            font,
            color: [ink.r(), ink.g(), ink.b()],
        },
        font.name(),
    )
    .chosen(chosen)
    .on_pick(move |()| {
        font.select();
        SettingsStore::get().update(|s| s.pen.smooth_font = font);
    })
    .into_element()
}

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
                Section::new(tr!("settings-pen-smooth"))
                    .child(Text::new(tr!("settings-pen-smooth-hint")).role(TextRole::Hint))
                    .child(rect().width(Size::fill()).spacing(10.0).children(
                        GlyphFont::ALL.chunks(2).map(|pair| {
                            rect()
                                .width(Size::fill())
                                .horizontal()
                                .content(Content::Flex)
                                .spacing(10.0)
                                .children(
                                    pair.iter()
                                        .map(|font| font_card(*font, *font == pen.smooth_font)),
                                )
                                .into_element()
                        }),
                    )),
            )
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

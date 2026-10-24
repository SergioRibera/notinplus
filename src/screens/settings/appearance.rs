//! Ajustes › Apariencia: theme, accent, text size and motion.

use freya::prelude::*;
use notein_ui_kit::theme::{ACCENT_SWATCHES, Palette, ThemeKind, palette, try_theme};
use notein_ui_kit::{
    ColorWheel, Group, Icon, ListRow, Notice, NoticeTone, Section, Segmented, Text, TextRole,
    Trailing, tokens,
};

use crate::settings::{SettingsStore, TextSizePref, ThemePref};

const THUMB_W: f32 = 116.0;
const THUMB_H: f32 = 78.0;

fn thumbnail(pal: Palette) -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(pal.bg)
        .padding(8.0)
        .spacing(5.0)
        .child(
            rect()
                .width(Size::percent(60.0))
                .height(Size::px(6.0))
                .corner_radius(3.0)
                .background(pal.t1),
        )
        .child(
            rect()
                .width(Size::fill())
                .height(Size::px(30.0))
                .corner_radius(6.0)
                .background(pal.s2)
                .padding(5.0)
                .spacing(4.0)
                .child(
                    rect()
                        .width(Size::percent(70.0))
                        .height(Size::px(4.0))
                        .corner_radius(2.0)
                        .background(pal.t2),
                )
                .child(
                    rect()
                        .width(Size::percent(40.0))
                        .height(Size::px(4.0))
                        .corner_radius(2.0)
                        .background(pal.t3),
                ),
        )
        .child(
            rect()
                .width(Size::px(34.0))
                .height(Size::px(8.0))
                .corner_radius(4.0)
                .background(pal.accent),
        )
}

fn theme_card(
    pref: ThemePref,
    chosen: bool,
    accent: Option<Color>,
    on_pick: impl Fn() + 'static,
) -> Element {
    let p = palette();
    let paint = |kind: ThemeKind| {
        let base = kind.base();
        accent.map_or(base, |a| base.with_accent(a).0)
    };
    let preview = match pref {
        ThemePref::Automatic => rect()
            .width(Size::fill())
            .height(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .height(Size::fill())
                    .child(thumbnail(paint(ThemeKind::Dia))),
            )
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .height(Size::fill())
                    .child(thumbnail(paint(ThemeKind::Noche))),
            ),
        ThemePref::Noche => thumbnail(paint(ThemeKind::Noche)),
        ThemePref::Dia => thumbnail(paint(ThemeKind::Dia)),
        ThemePref::Sepia => thumbnail(paint(ThemeKind::Sepia)),
        ThemePref::Contraste => thumbnail(paint(ThemeKind::Contraste)),
        ThemePref::Oled => thumbnail(paint(ThemeKind::Oled)),
    };
    rect()
        .width(Size::px(THUMB_W))
        .spacing(6.0)
        .a11y_role(AccessibilityRole::RadioButton)
        .a11y_alt(pref.label())
        .a11y_focusable(true)
        .on_press(move |_| on_pick())
        .child(
            rect()
                .width(Size::px(THUMB_W))
                .height(Size::px(THUMB_H))
                .corner_radius(tokens::radius::LG)
                .overflow(Overflow::Clip)
                .border(
                    Border::new()
                        .fill(if chosen { p.accent } else { p.line })
                        .width(if chosen { 2.5 } else { 1.0 })
                        .alignment(BorderAlignment::Outer),
                )
                .child(preview),
        )
        .child(
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(4.0)
                .maybe_child(chosen.then(|| {
                    Icon::new(freya::icons::lucide::check())
                        .size(14.0)
                        .color(p.accent)
                }))
                .child(Text::new(pref.label()).role(if chosen {
                    TextRole::Strong
                } else {
                    TextRole::Body
                })),
        )
        .into_element()
}

fn rgb(color: Color) -> [u8; 3] {
    [color.r(), color.g(), color.b()]
}

/// The Apariencia section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Appearance;

impl Component for Appearance {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let store = SettingsStore::get();
        let settings = store.read().appearance;
        let mut custom_open = use_state(|| false);
        let accent = settings.accent.map(|[r, g, b]| Color::from_rgb(r, g, b));
        let resolved = try_theme().map(|t| t.resolved());

        let themes = ThemePref::ALL.into_iter().map(|pref| {
            theme_card(pref, settings.theme == pref, accent, move || {
                let mut store = store;
                store.update(|s| s.appearance.theme = pref);
            })
        });

        let swatch = move |color: Option<Color>, label: &'static str| {
            let chosen = color == accent;
            let fill = color.unwrap_or_else(|| resolved.map_or(p.accent, |r| r.kind.base().accent));
            rect()
                .width(Size::px(34.0))
                .height(Size::px(34.0))
                .corner_radius(tokens::radius::FULL)
                .background(fill)
                .center()
                .a11y_role(AccessibilityRole::RadioButton)
                .a11y_alt(label)
                .border(
                    Border::new()
                        .fill(if chosen { p.t1 } else { Color::TRANSPARENT })
                        .width(2.0)
                        .alignment(BorderAlignment::Outer),
                )
                .on_press(move |_| {
                    let mut store = store;
                    store.update(|s| s.appearance.accent = color.map(rgb));
                })
                .maybe_child(chosen.then(|| {
                    Icon::new(lucide::check())
                        .size(16.0)
                        .color(notein_ui_kit::theme::on_color(fill))
                }))
                .into_element()
        };
        let custom_chosen = accent.is_some_and(|a| !ACCENT_SWATCHES.contains(&a));
        let custom = rect()
            .width(Size::px(34.0))
            .height(Size::px(34.0))
            .corner_radius(tokens::radius::FULL)
            .background(if custom_chosen {
                accent.unwrap_or(p.s3)
            } else {
                p.s3
            })
            .border(
                Border::new()
                    .fill(if custom_chosen { p.t1 } else { p.line_strong })
                    .width(if custom_chosen { 2.0 } else { 1.0 })
                    .alignment(BorderAlignment::Outer),
            )
            .center()
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt("Otro color")
            .on_press(move |_| {
                let next = !custom_open();
                custom_open.set(next);
            })
            .child(Icon::new(lucide::pipette()).size(16.0).color(p.t1));

        let sizes = ["Chico", "Normal", "Grande", "Más grande"];
        let size_index = TextSizePref::ALL
            .iter()
            .position(|s| *s == settings.text_size)
            .unwrap_or(1);

        rect()
            .width(Size::fill())
            .spacing(24.0)
            .child(
                Section::new("Tema").child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .content(Content::wrap_spacing(14.0))
                        .spacing(12.0)
                        .children(themes),
                ),
            )
            .child(
                Section::new("Color de acento")
                    .child(
                        Text::new("Se usa en botones, selección, enlaces y la pestaña activa. Los documentos no cambian.")
                            .role(TextRole::Hint),
                    )
                    .child(
                        rect()
                            .width(Size::fill())
                            .horizontal()
                            .content(Content::wrap_spacing(10.0))
                            .spacing(10.0)
                            .child(swatch(None, "Del tema"))
                            .children(ACCENT_SWATCHES.into_iter().map(|c| swatch(Some(c), "Color")))
                            .child(custom),
                    )
                    .maybe_child(custom_open().then(|| {
                        ColorWheel::new()
                            .swatches(Vec::new())
                            .allow_custom(true)
                            .initial(accent.unwrap_or(p.accent))
                            .width(300.0)
                            .on_change(move |color: Color| {
                                let mut store = store;
                                store.update(|s| s.appearance.accent = Some(rgb(color)));
                            })
                    }))
                    .maybe_child(resolved.filter(|r| r.accent_adjusted).map(|r| {
                        let theme = r.kind.label();
                        let verb = if r.palette.dark { "lo aclaramos" } else { "lo oscurecemos" };
                        Notice::new(NoticeTone::Info, format!("En {theme} {verb} para que se lea."))
                    })),
            )
            .child(
                Section::new("Tamaño del texto")
                    .child(
                        Segmented::new(sizes, size_index).full_width().on_select(move |i: usize| {
                            let mut store = store;
                            store.update(|s| s.appearance.text_size = TextSizePref::ALL[i]);
                        }),
                    )
                    .child(Text::new("Así se ven los nombres de tus documentos.").role(TextRole::Meta)),
            )
            .child(
                Group::default().child(
                    ListRow::new("Reducir movimiento")
                        .subtitle("Sin animaciones al abrir paneles ni destellos al saltar a una referencia.")
                        .trailing(Trailing::Switch {
                            on: settings.reduce_motion,
                            on_toggle: EventHandler::new(move |on| {
                                let mut store = store;
                                store.update(|s| s.appearance.reduce_motion = on);
                            }),
                        }),
                ),
            )
            .into_element()
    }
}

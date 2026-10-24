//! Ajustes › Apariencia: theme, accent, text size and motion.

use freya::prelude::*;
use notein_ui_kit::theme::{ACCENT_SWATCHES, Palette, ThemeKind, palette, try_theme};
use notein_ui_kit::{
    Button, ColorWheel, Group, Icon, ListRow, ModalController, Notice, NoticeTone, Popover,
    PopoverAlign, Section, Segmented, Text, TextRole, Trailing, tokens,
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

/// An accent swatch; `None` is the theme's own accent, which also
/// stands for a pick of the very same colour.
fn accent_swatch(color: Option<Color>, label: &'static str, accent: Option<Color>, theme_accent: Color) -> Element {
    let p = palette();
    let chosen = color.map_or_else(
        || accent.is_none_or(|a| a == theme_accent),
        |c| accent == Some(c),
    );
    let fill = color.unwrap_or(theme_accent);
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
            SettingsStore::get().update(|s| s.appearance.accent = color.map(rgb));
        })
        .maybe_child(chosen.then(|| {
            Icon::new(freya::icons::lucide::check())
                .size(16.0)
                .color(notein_ui_kit::theme::on_color(fill))
        }))
        .into_element()
}

/// The button that opens the free colour picker; filled with the
/// accent when it is not one of the swatches.
fn custom_swatch(accent: Option<Color>, theme_accent: Color, mut area: State<Area>) -> Rect {
    let p = palette();
    let chosen = accent.is_some_and(|a| a != theme_accent && !ACCENT_SWATCHES.contains(&a));
    rect()
        .width(Size::px(34.0))
        .height(Size::px(34.0))
        .corner_radius(tokens::radius::FULL)
        .background(if chosen { accent.unwrap_or(p.s3) } else { p.s3 })
        .border(
            Border::new()
                .fill(if chosen { p.t1 } else { p.line_strong })
                .width(if chosen { 2.0 } else { 1.0 })
                .alignment(BorderAlignment::Outer),
        )
        .center()
        .a11y_role(AccessibilityRole::Button)
        .a11y_alt("Otro color")
        .on_sized(move |e: Event<SizedEventData>| area.set_if_modified(e.area))
        .on_press(move |_| {
            let initial = accent.unwrap_or(theme_accent);
            Popover::new(*area.peek(), AccentPicker { initial })
                .width(PICKER_WIDTH + 32.0)
                .estimated_height(380.0)
                .align(PopoverAlign::End)
                .sheet_title("Otro color")
                .open();
        })
        .child(Icon::new(freya::icons::lucide::pipette()).size(16.0).color(p.t1))
}

const PICKER_WIDTH: f32 = 268.0;

/// The free colour picker: nothing changes until «Usar».
#[derive(Clone, Copy, Debug, PartialEq)]
struct AccentPicker {
    initial: Color,
}

impl AccentPicker {
    /// How many themes show `color` as it is, without lightening or
    /// darkening it.
    fn readable_in(color: Color) -> usize {
        ThemeKind::ALL
            .into_iter()
            .filter(|kind| !kind.base().with_accent(color).1)
            .count()
    }
}

impl Component for AccentPicker {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let initial = self.initial;
        let mut draft = use_state(move || initial);
        let color = *draft.read();
        let readable = Self::readable_in(color);
        let all = ThemeKind::ALL.len();
        let (tone, note) = if readable == all {
            (NoticeTone::Success, format!("Se lee bien en los {all} temas"))
        } else {
            (
                NoticeTone::Info,
                format!("Se ajusta en {} de los {all} temas para que se lea", all - readable),
            )
        };
        rect()
            .width(Size::fill())
            .padding(16.0)
            .spacing(14.0)
            .child(
                ColorWheel::new()
                    .swatches(Vec::new())
                    .allow_custom(true)
                    .alpha(false)
                    .bare()
                    .initial(initial)
                    .width(PICKER_WIDTH)
                    .on_change(move |c: Color| draft.set(c)),
            )
            .child(Notice::new(tone, note))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .main_align(Alignment::End)
                    .cross_align(Alignment::Center)
                    .spacing(8.0)
                    .child(
                        Button::new("Cancelar")
                            .ghost()
                            .on_press(|_| ModalController::get().close()),
                    )
                    .child(Button::new("Usar").primary().on_press(move |_| {
                        let color = *draft.peek();
                        ModalController::get().close();
                        SettingsStore::get().update(|s| s.appearance.accent = Some(rgb(color)));
                    })),
            )
            .background(p.s2)
    }
}

/// Swatches, the picker, and a note when the theme had to adjust it.
fn accent_section(accent: Option<Color>, custom: State<Area>) -> Section {
    let p = palette();
    let resolved = try_theme().map(|t| t.resolved());
    let theme_accent = resolved.map_or(p.accent, |r| r.kind.base().accent);
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
                .child(accent_swatch(None, "Del tema", accent, theme_accent))
                .children(
                    // The theme's own accent is already the first swatch.
                    ACCENT_SWATCHES
                        .into_iter()
                        .filter(|c| *c != theme_accent)
                        .map(|c| accent_swatch(Some(c), "Color", accent, theme_accent)),
                )
                .child(custom_swatch(accent, theme_accent, custom)),
        )
        .maybe_child(resolved.filter(|r| r.accent_adjusted).map(|r| {
            let theme = r.kind.label();
            let verb = if r.palette.dark { "lo aclaramos" } else { "lo oscurecemos" };
            Notice::new(NoticeTone::Info, format!("En {theme} {verb} para que se lea."))
        }))
}

impl Component for Appearance {
    fn render(&self) -> impl IntoElement {
        let store = SettingsStore::get();
        let settings = store.read().appearance;
        let custom = use_state(Area::default);
        let accent = settings.accent.map(|[r, g, b]| Color::from_rgb(r, g, b));
        let themes = ThemePref::ALL.into_iter().map(|pref| {
            theme_card(pref, settings.theme == pref, accent, move || {
                SettingsStore::get().update(|s| s.appearance.theme = pref);
            })
        });
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
            .child(accent_section(accent, custom))
            .child(
                Section::new("Tamaño del texto")
                    .child(
                        Segmented::new(["Chico", "Normal", "Grande", "Más grande"], size_index)
                            .full_width()
                            .on_select(move |i: usize| {
                                SettingsStore::get()
                                    .update(|s| s.appearance.text_size = TextSizePref::ALL[i]);
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
                                SettingsStore::get().update(|s| s.appearance.reduce_motion = on);
                            }),
                        }),
                ),
            )
            .into_element()
    }
}

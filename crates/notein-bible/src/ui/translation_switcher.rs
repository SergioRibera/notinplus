//! Translation picker modal.
//!
//! Rendered as a floating card above the reader. Lists every installed
//! Bible; tapping a row swaps the viewer to that translation. A trailing
//! row ("Agregar traducción…") invokes `on_install`, which the host
//! wires to the catalogue flow.

use freya::prelude::*;

use crate::db::BibleRow;

use super::theme;

const CARD_WIDTH: f32 = 320.0;
const ROW_HEIGHT: f32 = 56.0;

pub fn translation_switcher<FP, FI>(
    installed: Vec<BibleRow>,
    current: &str,
    on_pick: FP,
    on_install: FI,
) -> impl IntoElement
where
    FP: FnMut(String) + Clone + 'static,
    FI: FnMut() + 'static,
{
    let mut card = rect()
        .width(Size::px(CARD_WIDTH))
        .background(parse_rgb(theme::CHIP_BG))
        .with_corner_radius(14.0)
        .padding((6., 6.))
        .spacing(4.0);

    for b in installed {
        let is_current = b.id == current;
        let on_pick = on_pick.clone();
        card = card.child(bible_row(b, is_current, on_pick));
    }

    card = card.child(install_row(on_install));
    card
}

fn bible_row<FP>(b: BibleRow, is_current: bool, mut on_pick: FP) -> impl IntoElement
where
    FP: FnMut(String) + 'static,
{
    let id = b.id.clone();
    let indicator_color = if is_current {
        parse_rgb(theme::TEXT_PRIMARY)
    } else {
        parse_rgb(theme::VERSE_NUM)
    };
    rect()
        .height(Size::px(ROW_HEIGHT))
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .padding((0., 14.))
        .spacing(10.0)
        .with_corner_radius(10.0)
        .on_press(move |_: Event<PressEventData>| on_pick(id.clone()))
        .child(
            rect()
                .width(Size::px(8.0))
                .height(Size::px(8.0))
                .with_corner_radius(4.0)
                .background(indicator_color),
        )
        .child(
            rect()
                .spacing(2.0)
                .child(
                    label()
                        .font_size(14.0)
                        .color(parse_rgb(theme::TEXT_PRIMARY))
                        .text(if b.name_local.is_empty() {
                            b.id.clone()
                        } else {
                            b.name_local.clone()
                        }),
                )
                .child(
                    label()
                        .font_size(11.0)
                        .color(parse_rgb(theme::VERSE_NUM))
                        .text(b.abbrev),
                ),
        )
}

fn install_row<F>(mut on_install: F) -> impl IntoElement
where
    F: FnMut() + 'static,
{
    rect()
        .height(Size::px(ROW_HEIGHT))
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .padding((0., 14.))
        .spacing(10.0)
        .with_corner_radius(10.0)
        .on_press(move |_: Event<PressEventData>| on_install())
        .child(
            SvgViewer::new(freya::icons::lucide::plus())
                .color(parse_rgb(theme::TEXT_PRIMARY))
                .width(Size::px(16.0))
                .height(Size::px(16.0)),
        )
        .child(
            label()
                .font_size(14.0)
                .color(parse_rgb(theme::TEXT_PRIMARY))
                .text("Agregar traducción…"),
        )
}

fn parse_rgb(s: &str) -> Color {
    let trimmed = s.trim_start_matches("rgb(").trim_end_matches(')');
    let parts: Vec<u8> = trimmed
        .split(',')
        .filter_map(|t| t.trim().parse::<u8>().ok())
        .collect();
    if parts.len() == 3 {
        Color::from_rgb(parts[0], parts[1], parts[2])
    } else {
        Color::WHITE
    }
}

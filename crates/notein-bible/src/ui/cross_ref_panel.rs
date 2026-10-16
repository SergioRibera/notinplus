//! Side panel listing every cross-reference in the current chapter,
//! grouped by source verse. Each group is collapsible — tap the verse
//! number header to expand/collapse. Click any ref chip inside to
//! navigate to the target.
//!
//! Mounted as an absolute-positioned overlay on the right side of the
//! reader. The host owns the open/closed signal + the on_jump handler;
//! this module is a pure builder.

use std::collections::{BTreeMap, HashSet};

use freya::prelude::*;

use super::reader::{CrossRefView, OnJump};
use super::theme;

const PANEL_WIDTH: f32 = 320.0;

pub fn cross_ref_panel(
    chapter_label: &str,
    refs: &[CrossRefView],
    on_jump: OnJump,
    on_close: impl FnMut() + 'static,
) -> impl IntoElement {
    let mut on_close = on_close;
    let grouped = group_by_verse(refs);

    let mut body = rect()
        .width(Size::fill())
        .padding((8., 10.))
        .spacing(6.0);
    if grouped.is_empty() {
        body = body.child(
            label()
                .font_size(13.0)
                .color(parse_rgb(theme::VERSE_NUM))
                .text("Sin referencias cruzadas en este capítulo."),
        );
    } else {
        for (verse, refs) in grouped {
            body = body.child(VerseCollapsible {
                verse,
                refs,
                on_jump: on_jump.clone(),
            });
        }
    }

    rect()
        .width(Size::px(PANEL_WIDTH))
        .height(Size::fill())
        .background(parse_rgb(theme::CHIP_BG))
        .padding((0., 0.))
        .spacing(0.0)
        .child(
            rect()
                .width(Size::fill())
                .height(Size::px(44.0))
                .direction(Direction::Horizontal)
                .cross_align(Alignment::Center)
                .main_align(Alignment::SpaceBetween)
                .padding((0., 12.))
                .background(parse_rgb(theme::BG))
                .child(
                    label()
                        .font_size(13.5)
                        .color(parse_rgb(theme::TEXT_PRIMARY))
                        .text(format!("Referencias — {chapter_label}")),
                )
                .child(
                    rect()
                        .padding((4., 10.))
                        .with_corner_radius(4.0)
                        .on_press(move |_: Event<PressEventData>| on_close())
                        .child(
                            label()
                                .font_size(13.0)
                                .color(parse_rgb(theme::TEXT_PRIMARY))
                                .text("Cerrar"),
                        ),
                ),
        )
        .child(ScrollView::new().child(body))
}

fn group_by_verse(refs: &[CrossRefView]) -> BTreeMap<u16, Vec<CrossRefView>> {
    let mut out: BTreeMap<u16, Vec<CrossRefView>> = BTreeMap::new();
    for r in refs {
        out.entry(r.src_verse).or_default().push(r.clone());
    }
    out
}

#[derive(Clone, PartialEq, Debug)]
struct VerseCollapsible {
    verse: u16,
    refs: Vec<CrossRefView>,
    on_jump: OnJump,
}

impl Component for VerseCollapsible {
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| false);
        let is_open = *open.read();
        let verse = self.verse;
        let count = self.refs.len();

        let header = rect()
            .width(Size::fill())
            .padding((8., 10.))
            .direction(Direction::Horizontal)
            .cross_align(Alignment::Center)
            .main_align(Alignment::SpaceBetween)
            .with_corner_radius(6.0)
            .background(parse_rgb(theme::BG))
            .on_press(move |_: Event<PressEventData>| {
                let cur = *open.read();
                open.set(!cur);
            })
            .child(
                label()
                    .font_size(13.0)
                    .color(parse_rgb(theme::TEXT_PRIMARY))
                    .text(format!("Vers. {verse} — {count}")),
            )
            .child(
                label()
                    .font_size(13.0)
                    .color(parse_rgb(theme::VERSE_NUM))
                    .text(if is_open { "▾" } else { "▸" }),
            );

        let mut body = rect().width(Size::fill()).padding((4., 10., 10., 10.)).spacing(4.0);
        if is_open {
            for xref in &self.refs {
                body = body.child(ref_row(xref.clone(), self.on_jump.clone()));
            }
        }

        rect()
            .width(Size::fill())
            .spacing(2.0)
            .child(header)
            .child(body)
    }
}

fn ref_row(xref: CrossRefView, on_jump: OnJump) -> impl IntoElement {
    let target = (xref.tgt_book.clone(), xref.tgt_chap, xref.tgt_verse);
    rect()
        .width(Size::fill())
        .padding((6., 10.))
        .spacing(2.0)
        .with_corner_radius(6.0)
        .on_press(move |_: Event<PressEventData>| on_jump.call(target.clone()))
        .child(
            label()
                .font_size(13.0)
                .color(Color::from_rgb(100, 150, 230))
                .text(xref.label.clone()),
        )
        .child(
            label()
                .font_size(12.0)
                .color(parse_rgb(theme::TEXT_SECONDARY))
                .text(xref.tooltip),
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

// Silence unused — HashSet import kept for future "already expanded" set.
#[allow(dead_code)]
fn _unused() -> HashSet<u16> {
    HashSet::new()
}

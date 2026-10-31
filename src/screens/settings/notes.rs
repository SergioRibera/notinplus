//! Settings › Notes.

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{ChoiceCard, Group, ListRow, Section, Trailing, tokens};

use notein_i18n::tr;

use crate::settings::SettingsStore;

/// The Notes section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Notes;

/// A line of a note as it looks while typing: plain text, then the
/// bold word either formatted or between its marks, then the caret.
#[sismo::function("settings::notes_typing_preview")]
fn typing_preview(marks: bool) -> Rect {
    let p = palette();
    let word = if marks {
        label()
            .text(
                tr!("settings-notes-preview-marked", "word" => tr!("settings-notes-preview-bold")),
            )
            .font_size(tokens::text::BODY)
            .color(p.t3)
    } else {
        label()
            .text(tr!("settings-notes-preview-bold"))
            .font_size(tokens::text::BODY)
            .font_weight(FontWeight::BOLD)
            .color(p.t1)
    };
    rect()
        .width(Size::fill())
        .height(Size::px(44.0))
        .padding((0.0, 12.0))
        .corner_radius(tokens::radius::MD)
        .background(p.bg)
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(4.0)
        .child(
            label()
                .text(tr!("settings-notes-preview-line"))
                .font_size(tokens::text::BODY)
                .color(p.t1)
                .max_lines(1),
        )
        .child(word)
        .child(
            rect()
                .width(Size::px(2.0))
                .height(Size::px(20.0))
                .corner_radius(1.0)
                .background(p.accent),
        )
}

/// A quoted verse as it sits in a note: the text, its source, then either
/// the mark of a quote that follows its source or a caret inside the text.
#[sismo::function("settings::notes_quote_preview")]
fn quote_preview(live: bool) -> Rect {
    let p = palette();
    let text = rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(2.0)
        .child(
            label()
                .text(tr!("settings-notes-preview-quote"))
                .font_size(tokens::text::BODY)
                .color(p.t1)
                .max_lines(1),
        )
        .maybe(!live, |r| {
            r.child(
                rect()
                    .width(Size::px(2.0))
                    .height(Size::px(18.0))
                    .corner_radius(1.0)
                    .background(p.accent),
            )
        });
    let source = rect()
        .width(Size::fill())
        .horizontal()
        .content(Content::Flex)
        .cross_align(Alignment::Center)
        .spacing(6.0)
        .child(
            label()
                .width(Size::flex(1.0))
                .text(tr!("settings-notes-preview-source"))
                .font_size(tokens::text::CAPTION)
                .color(p.t2)
                .max_lines(1),
        )
        .maybe(live, |r| {
            r.child(
                SvgViewer::new(freya::icons::lucide::refresh_cw())
                    .color(p.accent)
                    .width(Size::px(12.0))
                    .height(Size::px(12.0)),
            )
        });
    rect()
        .width(Size::fill())
        .padding(8.0)
        .corner_radius(tokens::radius::MD)
        .background(p.bg)
        .child(
            rect()
                .width(Size::fill())
                .padding((2.0, 10.0))
                .spacing(4.0)
                .border(
                    Border::new()
                        .width(BorderWidth {
                            top: 0.0,
                            right: 0.0,
                            bottom: 0.0,
                            left: 3.0,
                        })
                        .alignment(BorderAlignment::Inner)
                        .fill(if live { p.kinds.bible } else { p.line_strong }),
                )
                .child(text)
                .child(source),
        )
}

impl Component for Notes {
    #[sismo::function("settings::notes")]
    fn render(&self) -> impl IntoElement {
        let store = SettingsStore::get();
        let notes = store.read().notes;
        let quoting = |live: bool, title: String, detail: String| {
            ChoiceCard::new(quote_preview(live), title)
                .detail(detail)
                .chosen(notes.live_quotes == live)
                .on_pick(move |()| {
                    let mut store = store;
                    store.update(|s| s.notes.live_quotes = live);
                })
        };
        let format = |marks: bool, title: String, detail: String| {
            ChoiceCard::new(typing_preview(marks), title)
                .detail(detail)
                .chosen(notes.show_markdown_marks == marks)
                .on_pick(move |()| {
                    let mut store = store;
                    store.update(|s| s.notes.show_markdown_marks = marks);
                })
        };
        rect()
            .width(Size::fill())
            .spacing(24.0)
            .child(
                Section::new(tr!("settings-notes-format")).child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .content(Content::Flex)
                        .spacing(10.0)
                        .child(format(
                            false,
                            tr!("settings-notes-format-rich"),
                            tr!("settings-notes-format-rich-hint"),
                        ))
                        .child(format(
                            true,
                            tr!("settings-notes-format-marks"),
                            tr!("settings-notes-format-marks-hint"),
                        )),
                ),
            )
            .child(
                Section::new(tr!("settings-notes-quoting")).child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .content(Content::Flex)
                        .spacing(10.0)
                        .child(quoting(
                            true,
                            tr!("settings-notes-quoting-live"),
                            tr!("settings-notes-quoting-live-hint"),
                        ))
                        .child(quoting(
                            false,
                            tr!("settings-notes-quoting-copy"),
                            tr!("settings-notes-quoting-copy-hint"),
                        )),
                ),
            )
            .child(
                Section::new(tr!("settings-notes-references")).child(
                    Group::default().child(
                        ListRow::new(tr!("settings-notes-references-beside"))
                            .subtitle(tr!("settings-notes-references-beside-hint"))
                            .trailing(Trailing::Switch {
                                on: notes.open_refs_beside,
                                on_toggle: EventHandler::new(move |on| {
                                    let mut store = store;
                                    store.update(|s| s.notes.open_refs_beside = on);
                                }),
                            }),
                    ),
                ),
            )
            .into_element()
    }
}

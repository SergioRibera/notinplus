//! Cross-references of a verse: a panel beside the text when the reader
//! has room for it, a sheet from the bottom when it does not.

use freya::prelude::*;
use notein_i18n::tr;
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    IconButton, ModalController, Sheet, SidePanel, Text, TextRole, is_mouse, tokens,
};

use super::Reader;

/// Width of the panel beside the text.
const PANEL_WIDTH: f32 = 340.0;
/// The reader needs this much width besides the panel to keep it open
/// beside the text; narrower, references open in a sheet.
pub(super) const ROOM_FOR_PANEL: f32 = 640.0 + PANEL_WIDTH;

/// The cross-references of one verse, with the text of each target.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct VerseRefs {
    book: String,
    chapter: u16,
    pub(super) verse: u16,
    /// The verse they start from ("Juan 3:16").
    place: String,
    targets: Vec<Target>,
}

#[derive(Clone, Debug, PartialEq)]
struct Target {
    /// Full place ("2 Corintios 4:6").
    place: String,
    text: String,
    book: String,
    chapter: u16,
    verse: u16,
}

impl VerseRefs {
    /// The references of `verse` in the chapter `reader` shows.
    fn of(reader: &Reader, verse: u16) -> Self {
        let st = reader.state.peek();
        let (bible_id, book, chapter) = (
            st.bible_id.clone(),
            st.current_book.clone(),
            st.current_chapter,
        );
        let book_name = st
            .current_chapter_data()
            .map_or_else(|| book.clone(), |c| c.book_name.clone());
        drop(st);
        let db = &reader.db;
        let books = db.list_books(&bible_id).unwrap_or_default();
        let book_of = |id: &str| books.iter().find(|b| b.book_id.eq_ignore_ascii_case(id));
        let mut rows: Vec<_> = db
            .cross_refs_for_chapter(&bible_id, &book, chapter)
            .unwrap_or_default()
            .into_iter()
            .filter(|row| row.src_verse == verse)
            .collect();
        // In the order of the books, not of their ids.
        rows.sort_by_key(|row| {
            let order = book_of(&row.tgt_book).map_or(u16::MAX, |b| b.order_num);
            (order, row.tgt_chap, row.tgt_verse)
        });
        let targets = rows
            .into_iter()
            .map(|row| {
                let found = book_of(&row.tgt_book);
                let name = found.map_or_else(
                    || crate::canon::short_name(&row.tgt_book),
                    |b| b.name.clone(),
                );
                Target {
                    place: format!("{name} {}:{}", row.tgt_chap, row.tgt_verse),
                    text: db
                        .verse_text(&bible_id, &row.tgt_book, row.tgt_chap, row.tgt_verse)
                        .ok()
                        .flatten()
                        .unwrap_or_default(),
                    book: found.map_or_else(|| row.tgt_book.clone(), |b| b.book_id.clone()),
                    chapter: row.tgt_chap,
                    verse: row.tgt_verse,
                }
            })
            .collect();
        Self {
            place: format!("{book_name} {chapter}:{verse}"),
            book,
            chapter,
            verse,
            targets,
        }
    }

    /// Whether they start in the chapter `reader` shows.
    pub(super) fn here(&self, reader: &Reader) -> bool {
        let st = reader.state.read();
        st.current_book == self.book && st.current_chapter == self.chapter
    }

    fn count(&self) -> String {
        tr!("reader-refs-count", "count" => self.targets.len())
    }
}

/// Show the references of `verse`: in the panel when `wide`, in a sheet
/// otherwise. Pressing the open verse again closes the panel.
pub(super) fn open(reader: &Reader, verse: u16, wide: bool) {
    let refs = VerseRefs::of(reader, verse);
    if wide {
        let mut open = reader.refs;
        let same = open
            .peek()
            .as_ref()
            .is_some_and(|o| o.verse == verse && o.here(reader));
        open.set((!same).then_some(refs));
        return;
    }
    Sheet::titled(tr!("reader-refs-title", "place" => refs.place.clone()))
        .subtitle(refs.count())
        .child(List {
            reader: reader.clone(),
            refs,
            in_sheet: true,
        })
        .open();
}

/// The panel beside the text. It slides in from the right edge and back
/// out, pushing the text aside.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Panel {
    pub(super) reader: Reader,
}

impl Component for Panel {
    fn render(&self) -> impl IntoElement {
        let open = self.reader.refs;
        let current = open.read().clone();
        // The last references stay on screen while the panel slides out.
        let mut shown = use_state(|| current.clone());
        if current.is_some() && *shown.peek() != current {
            shown.set(current.clone());
        }
        let body = shown
            .read()
            .clone()
            .map_or_else(|| rect().into_element(), |refs| self.body(refs));
        SidePanel::new(current.is_some(), PANEL_WIDTH, body).into_element()
    }
}

impl Panel {
    /// The verse and the count over the list.
    fn body(&self, refs: VerseRefs) -> Element {
        let open = self.reader.refs;
        let p = palette();
        rect()
            .expanded()
            .background(p.s1)
            .border(Border::new().fill(p.line).width(BorderWidth {
                left: 1.0,
                ..BorderWidth::default()
            }))
            .content(Content::Flex)
            .child(
                rect()
                    .width(Size::fill())
                    .padding((12.0, 8.0, 8.0, 16.0))
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .child(
                        rect()
                            .width(Size::flex(1.0))
                            .spacing(2.0)
                            .child(
                                Text::new(tr!("reader-refs-title", "place" => refs.place.clone()))
                                    .role(TextRole::Strong),
                            )
                            .child(Text::new(refs.count()).role(TextRole::Meta)),
                    )
                    .child(
                        IconButton::new(freya::icons::lucide::x(), tr!("reader-refs-close"))
                            .size(32.0)
                            .on_press(move |_| {
                                let mut open = open;
                                open.set(None);
                            }),
                    ),
            )
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::flex(1.0))
                    .child(notein_ui_kit::ScrollContent::new(
                        rect().padding((0.0, 8.0, 16.0, 8.0)).child(List {
                            reader: self.reader.clone(),
                            refs,
                            in_sheet: false,
                        }),
                    )),
            )
            .into_element()
    }
}

/// The references, each with its text; pressing one goes there.
#[derive(Clone, Debug, PartialEq)]
struct List {
    reader: Reader,
    refs: VerseRefs,
    in_sheet: bool,
}

impl Component for List {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            .spacing(2.0)
            .children(self.refs.targets.iter().enumerate().map(|(i, target)| {
                rect()
                    .key(i)
                    .width(Size::fill())
                    .child(Row {
                        reader: self.reader.clone(),
                        target: target.clone(),
                        in_sheet: self.in_sheet,
                    })
                    .into_element()
            }))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Row {
    reader: Reader,
    target: Target,
    in_sheet: bool,
}

impl Component for Row {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut hovered = use_state(|| false);
        let (reader, target, in_sheet) = (self.reader.clone(), self.target.clone(), self.in_sheet);
        rect()
            .width(Size::fill())
            .padding((10.0, 12.0))
            .spacing(4.0)
            .corner_radius(tokens::radius::MD)
            .maybe(*hovered.read(), |r| r.background(p.s2))
            .a11y_role(AccessibilityRole::Button)
            .on_pointer_enter(move |e: Event<PointerEventData>| hovered.set(is_mouse(&e)))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_tap(move |_| {
                if in_sheet {
                    ModalController::get().close();
                }
                reader.go(&target.book, target.chapter, target.verse);
            })
            .child(
                Text::new(self.target.place.clone())
                    .role(TextRole::Strong)
                    .color(p.accent_text),
            )
            .maybe_child((!self.target.text.is_empty()).then(|| {
                label()
                    .width(Size::fill())
                    .text(self.target.text.clone())
                    .font_family(crate::fonts::FAMILY)
                    .font_size(tokens::text::BODY)
                    .color(p.t2)
                    .max_lines(3)
                    .text_overflow(TextOverflow::Ellipsis)
            }))
    }
}

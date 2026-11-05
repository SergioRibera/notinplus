//! Cross-references of a verse: a panel beside the text when the reader
//! has room for it, a sheet from the bottom when it does not.

use freya::prelude::*;
use notein_i18n::tr;
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    IconButton, ModalController, Segmented, Sheet, SidePanel, Text, TextRole, is_mouse, tokens,
};

use super::Reader;
use super::originals::{self, PanelTab};

/// Width of the panel beside the text.
const PANEL_WIDTH: f32 = 340.0;
/// The reader needs this much width besides the panel to keep it open
/// beside the text; narrower, references open in a sheet.
pub(super) const ROOM_FOR_PANEL: f32 = 640.0 + PANEL_WIDTH;

/// The cross-references of one verse, with the text of each target.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct VerseRefs {
    pub(super) book: String,
    pub(super) chapter: u16,
    pub(super) verse: u16,
    /// The verse they start from ("Juan 3:16").
    place: String,
    /// Its text in the open translation.
    text: String,
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
    #[cfg_attr(feature = "sismo", sismo::function("bible::refs_load"))]
    pub(super) fn of(reader: &Reader, verse: u16) -> Self {
        let st = reader.state.peek();
        let (bible_id, book, chapter) = (
            st.bible_id.clone(),
            st.current_book.clone(),
            st.current_chapter,
        );
        let (book_name, text) = st.current_chapter_data().map_or_else(
            || (book.clone(), String::new()),
            |c| {
                let text = c
                    .verses
                    .iter()
                    .find(|v| v.verse == verse)
                    .map(|v| v.text.clone())
                    .unwrap_or_default();
                (c.book_name.clone(), text)
            },
        );
        drop(st);
        let db = &reader.db;
        let books = db.list_books(&bible_id).unwrap_or_default();
        let book_of = |id: &str| books.iter().find(|b| b.book_id.eq_ignore_ascii_case(id));
        let mut rows: Vec<_> = db
            .cross_refs_for_chapter(&book, chapter)
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
            text,
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

    /// The verse itself, quoted over the tabs so the panel reads on its
    /// own; nothing when the chapter has no text for it.
    fn quote(&self, side_padding: f32) -> Option<Element> {
        if self.text.is_empty() {
            return None;
        }
        let p = palette();
        Some(
            rect()
                .width(Size::fill())
                .padding((0.0, side_padding, 10.0, side_padding))
                .child(
                    rect()
                        .width(Size::fill())
                        .padding((2.0, 0.0, 2.0, 12.0))
                        .border(Border::new().fill(p.accent).width(BorderWidth {
                            left: 3.0,
                            ..BorderWidth::default()
                        }))
                        .child(
                            label()
                                .width(Size::fill())
                                .text(self.text.clone())
                                .font_family(crate::fonts::FAMILY)
                                .font_size(tokens::text::BODY)
                                .font_slant(FontSlant::Italic)
                                .color(p.t2),
                        ),
                )
                .into_element(),
        )
    }
}

/// Show the references of `verse`: in the panel when `wide`, in a sheet
/// otherwise. Pressing the open verse again closes the panel.
#[cfg_attr(feature = "sismo", sismo::function("bible::refs_open"))]
pub(super) fn open(reader: &Reader, verse: u16, wide: bool) {
    let refs = VerseRefs::of(reader, verse);
    if wide {
        let mut tab = reader.panel_tab;
        let mut open = reader.refs;
        let mut occurrences = reader.occurrences;
        occurrences.set(None);
        let same = open
            .peek()
            .as_ref()
            .is_some_and(|o| o.verse == verse && o.here(reader))
            && *tab.peek() == PanelTab::Refs;
        tab.set(PanelTab::Refs);
        open.set((!same).then_some(refs));
        return;
    }
    open_sheet(reader, refs, PanelTab::Refs);
}

/// The verse in a sheet from the bottom, on `tab`; the other tab is a
/// segment away when the original languages are on.
#[cfg_attr(feature = "sismo", sismo::function("bible::refs_open_sheet"))]
pub(super) fn open_sheet(reader: &Reader, refs: VerseRefs, tab: PanelTab) {
    let mut title = reader.sheet_title;
    title.set(tab_title(tab, &refs));
    Sheet::default()
        .title_state(title)
        .subtitle(refs.count())
        .child(SheetBody {
            reader: reader.clone(),
            refs,
            tab,
        })
        .open();
}

/// The tabs and the content of one of them, in the sheet.
#[derive(Clone, Debug, PartialEq)]
struct SheetBody {
    reader: Reader,
    refs: VerseRefs,
    tab: PanelTab,
}

impl Component for SheetBody {
    #[cfg_attr(feature = "sismo", sismo::function("bible::refs_sheet_body"))]
    fn render(&self) -> impl IntoElement {
        let tab = use_state(|| self.tab);
        let tabbed = originals::enabled(&self.reader);
        let current = if tabbed { *tab.read() } else { PanelTab::Refs };
        use_side_effect({
            let (mut title, refs) = (self.reader.sheet_title, self.refs.clone());
            move || {
                let next = tab_title(*tab.read(), &refs);
                if *title.peek() != next {
                    title.set(next);
                }
            }
        });
        rect()
            .width(Size::fill())
            .spacing(8.0)
            .maybe_child(self.refs.quote(8.0))
            .maybe_child(tabbed.then(|| tabs(tab, 8.0)))
            .child(tab_body(&self.reader, &self.refs, current, true))
    }
}

/// What the verse panel or sheet is called while it shows `tab`.
fn tab_title(tab: PanelTab, refs: &VerseRefs) -> String {
    match tab {
        PanelTab::Refs => tr!("reader-refs-title", "place" => refs.place.clone()),
        PanelTab::Original => tr!("originals-title", "place" => refs.place.clone()),
    }
}

/// The Original · Referencias segments, `tab` selected.
fn tabs(tab: State<PanelTab>, side_padding: f32) -> impl IntoElement {
    let mut tab = tab;
    let current = *tab.read();
    rect()
        .width(Size::fill())
        .padding((0.0, side_padding, 8.0, side_padding))
        .child(
            Segmented::new(
                [tr!("originals-tab"), tr!("originals-tab-refs")],
                match current {
                    PanelTab::Original => 0,
                    PanelTab::Refs => 1,
                },
            )
            .full_width()
            .on_select(move |index: usize| {
                tab.set(if index == 0 {
                    PanelTab::Original
                } else {
                    PanelTab::Refs
                });
            }),
        )
}

/// The content of `tab` for the verse `refs` describe.
fn tab_body(reader: &Reader, refs: &VerseRefs, tab: PanelTab, in_sheet: bool) -> Element {
    match tab {
        PanelTab::Refs => List {
            reader: reader.clone(),
            refs: refs.clone(),
            in_sheet,
        }
        .into_element(),
        PanelTab::Original => originals::Body {
            reader: reader.clone(),
            book: refs.book.clone(),
            chapter: refs.chapter,
            verse: refs.verse,
            in_sheet,
        }
        .into_element(),
    }
}

/// The panel beside the text. It slides in from the right edge and back
/// out, pushing the text aside.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Panel {
    pub(super) reader: Reader,
}

impl Component for Panel {
    #[cfg_attr(feature = "sismo", sismo::function("bible::refs_panel"))]
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
            .map_or_else(|| rect().into_element(), |refs| self.body(&refs));
        SidePanel::new(current.is_some(), PANEL_WIDTH, body).into_element()
    }
}

impl Panel {
    /// The verse and the count over the list.
    #[cfg_attr(feature = "sismo", sismo::function("bible::refs_body"))]
    fn body(&self, refs: &VerseRefs) -> Element {
        let open = self.reader.refs;
        let tab = self.reader.panel_tab;
        let mut occurrences = self.reader.occurrences;
        let tabbed = originals::enabled(&self.reader);
        let current = if tabbed { *tab.read() } else { PanelTab::Refs };
        // The occurrences of a word take the whole panel: its header
        // says so, with the way back to the verse.
        let nested = (*occurrences.read())
            .and_then(|lemma| originals::occurrences_header(lemma).map(|head| (lemma, head)));
        let (title, subtitle) = match &nested {
            Some((_, (title, count))) => (title.clone(), count.clone()),
            None => (tab_title(current, refs), refs.count()),
        };
        let content = match &nested {
            Some((lemma, _)) => originals::occurrences_body(&self.reader, *lemma, false),
            None => tab_body(&self.reader, refs, current, false),
        };
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
                    .spacing(4.0)
                    .maybe_child(nested.is_some().then(|| {
                        IconButton::new(
                            freya::icons::lucide::arrow_left(),
                            tr!("originals-occurrences-back"),
                        )
                        .size(32.0)
                        .on_press(move |_| occurrences.set(None))
                    }))
                    .child(
                        rect()
                            .width(Size::flex(1.0))
                            .spacing(2.0)
                            .child(Text::new(title).role(TextRole::Strong).lines(1))
                            .child(Text::new(subtitle).role(TextRole::Meta)),
                    )
                    .child(
                        IconButton::new(freya::icons::lucide::x(), tr!("reader-refs-close"))
                            .size(32.0)
                            .on_press(move |_| {
                                let mut open = open;
                                open.set(None);
                                occurrences.set(None);
                            }),
                    ),
            )
            .maybe_child(refs.quote(16.0).filter(|_| nested.is_none()))
            .maybe_child((tabbed && nested.is_none()).then(|| tabs(tab, 12.0)))
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::flex(1.0))
                    .child(notein_ui_kit::ScrollContent::new(
                        rect().padding((0.0, 8.0, 16.0, 8.0)).child(content),
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
    #[cfg_attr(feature = "sismo", sismo::function("bible::refs_list"))]
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
    #[cfg_attr(feature = "sismo", sismo::function("bible::refs_row"))]
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

//! The «Original» tab of the verse panel: every Hebrew or Greek word of
//! the verse, one row each, with its pronunciation, what it means here
//! and in general, its grammar in words and where else it appears.
//!
//! Without the packs on this device the tab shows the card that
//! downloads them. The tab only exists while the setting is on.

use std::rc::Rc;
use std::sync::Arc;

use freya::icons::lucide;
use freya::prelude::*;
use notin_i18n::{Lang, tr};
use notin_sdk::bookmark::SourceRef;
use notin_sdk::services::{HostToast, ToastTone};
use notin_sdk::ui::SelectionExtra;
use notin_ui_kit::theme::palette;
use notin_ui_kit::{
    Button, IconButton, ModalController, Notice, NoticeTone, ProgressBar, Sheet, TapExt, Text,
    TextRole, is_mouse, tokens,
};

use super::Reader;
use crate::anchor::{BibleAnchor, BibleRef, OriginalRef};
use crate::db::LinkRow;
use crate::originals::describe::{self, Also, Meaning};
use crate::originals::{self, LemmaRef, Progress, Store, WordView, morph};
use crate::plugin_config;

/// Which tab the verse panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PanelTab {
    #[default]
    Refs,
    Original,
}

/// Whether the reader offers the original languages at all.
pub(super) fn enabled(reader: &Reader) -> bool {
    plugin_config::effective_originals(reader.kind.config())
}

/// Re-render the caller whenever the packs land or the download moves.
#[must_use]
pub(super) fn use_changes() -> State<u64> {
    let tick = use_state(|| 0u64);
    use_hook(move || {
        let Some(service) = originals::service() else {
            return;
        };
        let rx = service.changes();
        let mut tick = tick;
        spawn(async move {
            while rx.recv_async().await.is_ok() {
                while rx.try_recv().is_ok() {}
                let next = tick.peek().wrapping_add(1);
                tick.set(next);
            }
        });
    });
    tick
}

/// Show the original of `verse`: in the panel when `wide`, in a sheet
/// otherwise.
#[cfg_attr(feature = "sismo", sismo::function("bible::originals_open"))]
pub(super) fn open(reader: &Reader, verse: u16, wide: bool) {
    let refs = super::refs::VerseRefs::of(reader, verse);
    if wide {
        let mut tab = reader.panel_tab;
        tab.set(PanelTab::Original);
        let mut occurrences = reader.occurrences;
        occurrences.set(None);
        let mut open = reader.refs;
        open.set(Some(refs));
        return;
    }
    super::refs::open_sheet(reader, refs, PanelTab::Original);
}

/// Show what a cited place of the original text points at: its verse in
/// the «Original» tab with the word open, or the occurrences of a term.
pub(super) fn follow(reader: &Reader, passage: &crate::anchor::BibleAnchor, original: OriginalRef) {
    let verse = passage.start_verse;
    let wide = *reader.wide.peek();
    match original {
        OriginalRef::Word { position } => {
            let mut open_word = reader.open_word;
            open_word.set(Some(position));
            open(reader, verse, wide);
        }
        OriginalRef::Term { position } => {
            open(reader, verse, wide);
            let lemma = originals::service()
                .and_then(|service| service.store())
                .and_then(|store| {
                    store
                        .words(
                            &passage.book_id,
                            passage.start_chapter,
                            verse,
                            Lang::current().tag(),
                        )?
                        .into_iter()
                        .find(|w| w.position == position)
                        .map(|w| w.lemma)
                });
            if let Some(lemma) = lemma {
                if wide {
                    let mut occurrences = reader.occurrences;
                    occurrences.set(Some(lemma));
                } else {
                    open_occurrences_sheet(reader, lemma);
                }
            }
        }
        OriginalRef::Interlinear => open(reader, verse, wide),
    }
}

/// The selection bar's «Original» button, for a selection of one run of
/// verses on a chapter the packs cover; `None` otherwise.
pub(super) fn selection_extra(
    reader: &Reader,
    book: &str,
    verse: Option<u16>,
    wide: bool,
) -> Option<SelectionExtra> {
    if !enabled(reader) || originals::testament_of(book).is_none() {
        return None;
    }
    let verse = verse?;
    let reader = reader.clone();
    let action: Rc<dyn Fn()> = Rc::new(move || open(&reader, verse, wide));
    Some((tr!("originals-tab").into(), lucide::languages(), action))
}

/// One word as the row shows it, with what the translation makes of it.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    word: WordView,
    /// How the open translation renders it in this verse, when aligned.
    here: Option<String>,
    language: morph::Language,
}

/// The words of a verse, loaded once per verse.
#[derive(Clone, Debug, PartialEq)]
struct VerseWords {
    /// The verse, in the open translation.
    place: BibleAnchor,
    bible_id: String,
    bible_name: String,
    rows: Vec<Row>,
    testament: originals::Testament,
}

impl VerseWords {
    #[cfg_attr(feature = "sismo", sismo::function("bible::originals_load"))]
    fn of(reader: &Reader, store: &Store, book: &str, chapter: u16, verse: u16) -> Option<Self> {
        let testament = originals::testament_of(book)?;
        let words = store.words(book, chapter, verse, Lang::current().tag())?;
        let st = reader.state.peek();
        let bible_id = st.bible_id.clone();
        drop(st);
        let bible_name = reader
            .db
            .list_bibles()
            .unwrap_or_default()
            .into_iter()
            .find(|b| b.id == bible_id)
            .map_or_else(|| bible_id.clone(), |b| b.display_name().to_owned());
        let links: Vec<LinkRow> = reader
            .kind
            .originals()
            .links(&bible_id, book, chapter, verse);
        let verse_text = reader
            .db
            .verse_text(&bible_id, book, chapter, verse)
            .ok()
            .flatten()
            .unwrap_or_default();
        let language = describe::language_of(testament);
        let rows = words
            .into_iter()
            .map(|word| Row {
                here: describe::here_span(&links, &verse_text, word.position),
                word,
                language,
            })
            .collect();
        Some(Self {
            place: BibleAnchor::single_verse(&bible_id, book, chapter, verse),
            bible_id,
            bible_name,
            rows,
            testament,
        })
    }
}

/// The tab's content for one verse.
#[derive(Clone, PartialEq)]
pub(super) struct Body {
    pub(super) reader: Reader,
    pub(super) book: String,
    pub(super) chapter: u16,
    pub(super) verse: u16,
    pub(super) in_sheet: bool,
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OriginalsBody")
            .field("book", &self.book)
            .field("chapter", &self.chapter)
            .field("verse", &self.verse)
            .finish_non_exhaustive()
    }
}

impl Component for Body {
    #[cfg_attr(feature = "sismo", sismo::function("bible::originals_body"))]
    fn render(&self) -> impl IntoElement {
        let _tick = use_changes();
        let Some(service) = originals::service() else {
            return rect().into_element();
        };
        let Some(store) = service.store() else {
            return Card {
                progress: service.progress(),
            }
            .into_element();
        };
        let Some(words) =
            VerseWords::of(&self.reader, &store, &self.book, self.chapter, self.verse)
        else {
            return rect()
                .width(Size::fill())
                .padding((16.0, 16.0))
                .child(Text::new(tr!("originals-no-verse")).role(TextRole::Meta))
                .into_element();
        };
        Words {
            reader: self.reader.clone(),
            store,
            verse: Rc::new(words),
            in_sheet: self.in_sheet,
            progress: service.progress(),
        }
        .into_element()
    }
}

/// The list of words; one row open at a time.
#[derive(Clone)]
struct Words {
    reader: Reader,
    store: Arc<Store>,
    verse: Rc<VerseWords>,
    in_sheet: bool,
    progress: Progress,
}

impl PartialEq for Words {
    fn eq(&self, other: &Self) -> bool {
        self.reader == other.reader
            && Arc::ptr_eq(&self.store, &other.store)
            && self.verse == other.verse
            && self.in_sheet == other.in_sheet
            && self.progress == other.progress
    }
}

impl Component for Words {
    #[cfg_attr(feature = "sismo", sismo::function("bible::originals_words"))]
    fn render(&self) -> impl IntoElement {
        let mut open: State<Option<u16>> = use_state(|| None);
        // A cited word that was followed opens itself.
        use_side_effect({
            let mut wanted = self.reader.open_word;
            move || {
                let position = *wanted.read();
                if let Some(position) = position {
                    open.set(Some(position));
                    wanted.set(None);
                }
            }
        });
        let p = palette();
        let aligning = match &self.progress {
            Progress::Aligning { bible_id } if *bible_id == self.verse.bible_id => {
                Some(tr!("originals-aligning", "bible" => self.verse.bible_name.clone()))
            }
            _ => None,
        };
        let source = match self.verse.testament {
            originals::Testament::Old => tr!("originals-source-ot"),
            originals::Testament::New => tr!("originals-source-nt"),
        };
        let cite_verse = {
            let ui = Rc::clone(&self.reader.ui);
            let target = cite_target(&self.reader, &self.verse.place, OriginalRef::Interlinear);
            rect()
                .width(Size::fill())
                .padding((0.0, 12.0, 6.0, 12.0))
                .horizontal()
                .main_align(Alignment::End)
                .child(
                    Button::new(tr!("originals-cite-verse"))
                        .small()
                        .tonal()
                        .icon(lucide::quote())
                        .on_press(move |event: Event<PressEventData>| {
                            ui.cite(target.clone(), area_of(&event));
                        }),
                )
        };
        rect()
            .width(Size::fill())
            .spacing(2.0)
            .child(cite_verse)
            .maybe_child(aligning.map(|text| {
                rect()
                    .width(Size::fill())
                    .padding((4.0, 12.0, 8.0, 12.0))
                    .child(Text::new(text).role(TextRole::Meta))
            }))
            .children(self.verse.rows.iter().map(|row| {
                let is_open = *open.read() == Some(row.word.position);
                rect()
                    .key(row.word.position)
                    .width(Size::fill())
                    .child(WordRow {
                        reader: self.reader.clone(),
                        store: Arc::clone(&self.store),
                        words: Rc::clone(&self.verse),
                        row: row.clone(),
                        open: is_open,
                        toggle: open,
                        in_sheet: self.in_sheet,
                    })
                    .into_element()
            }))
            .child(
                rect()
                    .width(Size::fill())
                    .padding((12.0, 12.0, 8.0, 12.0))
                    .child(Text::new(source).role(TextRole::Meta).color(p.t3)),
            )
            .into_element()
    }
}

/// One word: the compact line, and the detail when open.
#[derive(Clone)]
struct WordRow {
    reader: Reader,
    store: Arc<Store>,
    words: Rc<VerseWords>,
    row: Row,
    open: bool,
    toggle: State<Option<u16>>,
    in_sheet: bool,
}

impl PartialEq for WordRow {
    fn eq(&self, other: &Self) -> bool {
        self.row == other.row && self.open == other.open && Arc::ptr_eq(&self.store, &other.store)
    }
}

impl Component for WordRow {
    #[cfg_attr(feature = "sismo", sismo::function("bible::originals_row"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut hovered = use_state(|| false);
        let position = self.row.word.position;
        let mut toggle = self.toggle;
        let word = &self.row.word;
        let compact_gloss = self
            .row
            .here
            .clone()
            .or_else(|| word.gloss.clone())
            .unwrap_or_else(|| word.gloss_en.clone());
        rect()
            .width(Size::fill())
            .padding((8.0, 12.0))
            .spacing(2.0)
            .corner_radius(tokens::radius::MD)
            .maybe(self.open, |r| r.background(p.s2))
            .maybe(*hovered.read() && !self.open, |r| r.background(p.s2))
            .a11y_role(AccessibilityRole::Button)
            .on_pointer_enter(move |e: Event<PointerEventData>| hovered.set(is_mouse(&e)))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_tap(move |_| {
                let next = (toggle.peek().as_ref() != Some(&position)).then_some(position);
                toggle.set(next);
            })
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::End)
                    .spacing(8.0)
                    .child(original_text(&word.surface, 20.0, p.t1, self.row.language))
                    .child(
                        label()
                            .text(word.translit.replace('.', ""))
                            .font_size(tokens::text::META)
                            .font_slant(FontSlant::Italic)
                            .color(p.t3),
                    )
                    .maybe_child(self.open.then(|| self.copy_reference())),
            )
            .child(
                label()
                    .width(Size::fill())
                    .text(compact_gloss)
                    .font_size(tokens::text::BODY)
                    .color(p.t2)
                    .max_lines(2)
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(self.open.then(|| self.detail()))
    }
}

impl WordRow {
    /// Base form, pronunciation, meaning here and in general, grammar
    /// and occurrences.
    #[cfg_attr(feature = "sismo", sismo::function("bible::originals_detail"))]
    fn detail(&self) -> Element {
        let p = palette();
        let word = &self.row.word;
        let lemma = self.store.lemma(word.lemma);
        let lang = Lang::current();
        let function_word = morph::is_function_word(&word.morph, self.row.language);

        let (base, respelling) = lemma.as_ref().map_or_else(
            || (word.surface.clone(), String::new()),
            |l| {
                (
                    l.lemma.clone(),
                    describe::respelling(l, self.row.language, lang),
                )
            },
        );
        let Meaning {
            here,
            here_english,
            also,
        } = describe::meaning(
            self.reader.kind.originals(),
            &self.store,
            &self.words.bible_id,
            &self.words.bible_name,
            word,
            self.row.here.as_deref(),
            self.row.language,
            lang,
        );
        let grammar = describe::grammar_line(word, self.row.language);
        let derivation = lemma
            .as_ref()
            .map(|l| describe::derivation_text(&self.store, &l.derivation_en, lang))
            .filter(|d| !d.is_empty());
        let count = lemma.as_ref().map_or(0, |l| l.count);
        let testament = match self.row.language {
            morph::Language::Hebrew => tr!("originals-testament-ot"),
            morph::Language::Greek => tr!("originals-testament-nt"),
        };
        let pronounced = (!respelling.is_empty())
            .then(|| tr!("originals-pronounced", "respelling" => respelling.clone()));

        rect()
            .width(Size::fill())
            .padding((10.0, 0.0, 2.0, 0.0))
            .spacing(6.0)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(1.0))
                    .background(p.line),
            )
            .child(
                rect()
                    .width(Size::fill())
                    .spacing(2.0)
                    .child(
                        Text::new(tr!("originals-base-form"))
                            .role(TextRole::Meta)
                            .color(p.t3),
                    )
                    .child(original_text(&base, 18.0, p.t1, self.row.language))
                    .maybe_child(pronounced.map(|text| {
                        label()
                            .text(text)
                            .font_size(tokens::text::META)
                            .font_slant(FontSlant::Italic)
                            .color(p.t3)
                    })),
            )
            .child(labelled(&tr!("originals-here"), here, here_english))
            .maybe_child((!function_word).then(|| also_view(&also)).flatten())
            .maybe_child((!grammar.is_empty()).then(|| {
                label()
                    .width(Size::fill())
                    .text(grammar)
                    .font_size(tokens::text::META)
                    .color(p.t3)
            }))
            .maybe_child(derivation.map(|derivation| origin_block(derivation, lang)))
            .maybe_child(function_word.then(|| {
                label()
                    .text(tr!("originals-no-meaning"))
                    .font_size(tokens::text::META)
                    .color(p.t3)
            }))
            .child(self.actions(count, testament))
            .into_element()
    }

    /// This word as a place to cite.
    fn target(&self) -> SourceRef {
        let original = OriginalRef::Word {
            position: self.row.word.position,
        };
        cite_target(&self.reader, &self.words.place, original)
    }

    /// The icon at the end of the word's line that copies a reference to
    /// it, as a chip to paste.
    fn copy_reference(&self) -> Element {
        let target = self.target();
        let label = self.reader.kind.original_label(
            &self.words.place,
            OriginalRef::Word {
                position: self.row.word.position,
            },
        );
        let host = Arc::clone(&self.reader.host);
        rect()
            .width(Size::flex(1.0))
            .horizontal()
            .main_align(Alignment::End)
            .child(
                IconButton::new(lucide::copy(), tr!("originals-copy-reference"))
                    .size(tokens::size::BUTTON_SM)
                    .on_press(move |event: Event<PressEventData>| {
                        event.stop_propagation();
                        let markdown =
                            format!("[{}]({})", label.replace(['[', ']'], ""), target.to_url());
                        if freya::clipboard::Clipboard::set(markdown).is_ok() {
                            host.toast(HostToast::new(
                                tr!("originals-reference-copied"),
                                ToastTone::Info,
                            ));
                        }
                    }),
            )
            .into_element()
    }

    /// The occurrence count with «see all» and «cite».
    fn actions(&self, count: usize, testament: String) -> impl IntoElement {
        let p = palette();
        let lemma_ref = self.row.word.lemma;
        let (reader, in_sheet) = (self.reader.clone(), self.in_sheet);
        let ui = Rc::clone(&self.reader.ui);
        let target = self.target();
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(8.0)
            .child(
                label()
                    .width(Size::flex(1.0))
                    .text(tr!("originals-count", "count" => count, "testament" => testament))
                    .font_size(tokens::text::META)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(p.accent_text),
            )
            .maybe_child((count > 1).then(|| {
                Button::new(tr!("originals-see-all", "count" => count))
                    .small()
                    .tonal()
                    .on_press(move |_| {
                        // In the panel the occurrences take its header; in
                        // a sheet they come as a sheet of their own.
                        if in_sheet {
                            open_occurrences_sheet(&reader, lemma_ref);
                        } else {
                            let mut occurrences = reader.occurrences;
                            occurrences.set(Some(lemma_ref));
                        }
                    })
            }))
            .child(
                Button::new(tr!("originals-cite"))
                    .small()
                    .primary()
                    .icon(lucide::quote())
                    .on_press(move |event: Event<PressEventData>| {
                        ui.cite(target.clone(), area_of(&event));
                    }),
            )
    }
}

/// `original` inside `place`, as a place of this reader's document to
/// cite.
fn cite_target(reader: &Reader, place: &BibleAnchor, original: OriginalRef) -> SourceRef {
    SourceRef::Plugin {
        plugin_id: crate::kind::KIND.to_owned(),
        item: reader.doc.0,
        anchor: notin_sdk::Payload::to_bytes(&BibleRef::original(place.clone(), original)),
        selection: None,
    }
}

/// Where a press landed, as the host takes it (`x, y, width, height`).
fn area_of(event: &Event<PressEventData>) -> (f32, f32, f32, f32) {
    let area = notin_ui_kit::press_anchor(event);
    (area.min_x(), area.min_y(), area.width(), area.height())
}

/// The block under «Aquí»; `None` when there is nothing to say.
fn also_view(also: &Also) -> Option<Element> {
    let p = palette();
    match also {
        Also::Renderings { items, .. } if items.is_empty() => None,
        Also::Senses { items, .. } if items.is_empty() => None,
        Also::Renderings { bible, items } => Some(
            rect()
                .width(Size::fill())
                .spacing(4.0)
                .child(
                    paragraph()
                        .width(Size::fill())
                        .font_size(tokens::text::META)
                        .color(p.t3)
                        .span(
                            Span::new(bible.clone())
                                .font_weight(FontWeight::BOLD)
                                .color(p.t2),
                        )
                        .span(Span::new(format!(" {}", tr!("originals-renders")))),
                )
                .children(items.iter().enumerate().map(|(i, (token, count))| {
                    rect()
                        .key(i)
                        .width(Size::fill())
                        .horizontal()
                        .content(Content::Flex)
                        .cross_align(Alignment::End)
                        .spacing(8.0)
                        .child(
                            label()
                                .width(Size::flex(1.0))
                                .text(token.clone())
                                .font_size(tokens::text::BODY)
                                .color(p.t1),
                        )
                        .child(
                            label()
                                .text(tr!("originals-times", "count" => *count))
                                .font_size(tokens::text::META)
                                .color(p.t3),
                        )
                        .into_element()
                }))
                .into_element(),
        ),
        Also::Senses { items, note } => Some(
            rect()
                .width(Size::fill())
                .spacing(4.0)
                .child(
                    label()
                        .width(Size::fill())
                        .text(note.as_ref().map_or_else(
                            || tr!("originals-senses"),
                            |note| tr!("originals-senses-noted", "note" => note.clone()),
                        ))
                        .font_size(tokens::text::META)
                        .font_weight(FontWeight::BOLD)
                        .color(p.t3),
                )
                .child(
                    label()
                        .width(Size::fill())
                        .text(items.join(", "))
                        .font_size(tokens::text::BODY)
                        .color(p.t1),
                )
                .into_element(),
        ),
    }
}

/// The dictionary's derivation under its caption, which says it is
/// English when the interface is not.
fn origin_block(derivation: String, lang: Lang) -> impl IntoElement {
    let p = palette();
    rect()
        .width(Size::fill())
        .spacing(4.0)
        .child(
            label()
                .width(Size::fill())
                .text(if lang == Lang::En {
                    tr!("originals-origin")
                } else {
                    tr!("originals-origin-noted", "note" => tr!("originals-meaning-english"))
                })
                .font_size(tokens::text::META)
                .font_weight(FontWeight::BOLD)
                .color(p.t3),
        )
        .child(
            label()
                .width(Size::fill())
                .text(derivation)
                .font_size(tokens::text::BODY)
                .color(p.t1),
        )
}

/// A caption in the margin and its text.
fn labelled(caption: &str, text: String, muted: bool) -> impl IntoElement {
    let p = palette();
    rect()
        .width(Size::fill())
        .horizontal()
        .spacing(8.0)
        .child(
            label()
                .width(Size::px(60.0))
                .text(caption.to_owned())
                .font_size(tokens::text::META)
                .font_weight(FontWeight::BOLD)
                .color(p.t3),
        )
        .child(
            label()
                .width(Size::flex(1.0))
                .text(text)
                .font_size(tokens::text::BODY)
                .color(if muted { p.t3 } else { p.t1 }),
        )
}

/// Hebrew or Greek text, in the font the plugin bundles for `language`.
fn original_text(
    text: &str,
    size: f32,
    color: Color,
    language: morph::Language,
) -> impl IntoElement {
    label()
        .text(text.to_owned())
        .font_family(crate::fonts::original_family(
            language == morph::Language::Hebrew,
        ))
        .font_size(size)
        .color(color)
}

/// The title and the count line over the occurrences of `lemma`;
/// `None` without the packs.
pub(super) fn occurrences_header(lemma: LemmaRef) -> Option<(String, String)> {
    let store = originals::service()?.store()?;
    let name = store.lemma(lemma).map(|l| l.lemma).unwrap_or_default();
    let count = store.occurrences(lemma).len();
    let testament = match lemma.testament {
        originals::Testament::Old => tr!("originals-testament-ot"),
        originals::Testament::New => tr!("originals-testament-nt"),
    };
    Some((
        tr!("originals-occurrences-title", "lemma" => name),
        tr!("originals-count", "count" => count, "testament" => testament),
    ))
}

/// The occurrences of `lemma` as a list; nothing without the packs.
pub(super) fn occurrences_body(reader: &Reader, lemma: LemmaRef, in_sheet: bool) -> Element {
    let Some(store) = originals::service().and_then(|service| service.store()) else {
        return rect().into_element();
    };
    Occurrences {
        reader: reader.clone(),
        store,
        lemma,
        in_sheet,
    }
    .into_element()
}

/// The occurrences of `lemma` in a sheet over the verse's, with its own
/// title and the way back.
fn open_occurrences_sheet(reader: &Reader, lemma: LemmaRef) {
    let Some((title, count)) = occurrences_header(lemma) else {
        return;
    };
    Sheet::titled(title)
        .subtitle(count)
        .on_back(|()| ModalController::get().close())
        .child(occurrences_body(reader, lemma, true))
        .push();
}

/// Every place a lemma occurs, in canon order; pressing one goes there.
#[derive(Clone)]
struct Occurrences {
    reader: Reader,
    store: Arc<Store>,
    lemma: LemmaRef,
    in_sheet: bool,
}

impl PartialEq for Occurrences {
    fn eq(&self, other: &Self) -> bool {
        self.lemma == other.lemma && Arc::ptr_eq(&self.store, &other.store)
    }
}

/// Rows shown at most: the article has twenty thousand.
const MAX_OCCURRENCES: usize = 300;

impl Component for Occurrences {
    #[cfg_attr(feature = "sismo", sismo::function("bible::originals_occurrences"))]
    fn render(&self) -> impl IntoElement {
        let (bible_id, book, chapter) = {
            let st = self.reader.state.peek();
            (
                st.bible_id.clone(),
                st.current_book.clone(),
                st.current_chapter,
            )
        };
        let books = self.reader.db.list_books(&bible_id).unwrap_or_default();
        let name_of = |id: &str| {
            books
                .iter()
                .find(|b| b.book_id.eq_ignore_ascii_case(id))
                .map_or_else(|| crate::canon::short_name(id), |b| b.name.clone())
        };
        let all = self.store.occurrences(self.lemma);
        rect()
            .width(Size::fill())
            .spacing(2.0)
            .children(all.iter().take(MAX_OCCURRENCES).enumerate().map(|(i, occurrence)| {
                let here = occurrence.book_id.eq_ignore_ascii_case(&book)
                    && occurrence.chapter == chapter;
                let place = format!(
                    "{} {}:{}{}",
                    name_of(&occurrence.book_id),
                    occurrence.chapter,
                    occurrence.verse,
                    if here {
                        format!(" ({})", tr!("originals-occurrences-here"))
                    } else {
                        String::new()
                    }
                );
                self.row(i, occurrence, &bible_id, place, here)
            }))
            .maybe_child((all.len() > MAX_OCCURRENCES).then(|| {
                rect()
                    .width(Size::fill())
                    .padding((8.0, 12.0))
                    .child(
                        Text::new(tr!("originals-count", "count" => all.len(), "testament" => match self.lemma.testament {
                            originals::Testament::Old => tr!("originals-testament-ot"),
                            originals::Testament::New => tr!("originals-testament-nt"),
                        }))
                        .role(TextRole::Meta),
                    )
            }))
    }
}

impl Occurrences {
    /// One occurrence: its place and the verse in the open translation;
    /// pressing it goes there.
    fn row(
        &self,
        index: usize,
        occurrence: &crate::originals::Occurrence,
        bible_id: &str,
        place: String,
        here: bool,
    ) -> Element {
        let p = palette();
        let text = self
            .reader
            .db
            .verse_text(
                bible_id,
                &occurrence.book_id,
                occurrence.chapter,
                occurrence.verse,
            )
            .ok()
            .flatten()
            .unwrap_or_default();
        let reader = self.reader.clone();
        let in_sheet = self.in_sheet;
        let (book, chapter, verse) = (
            occurrence.book_id.clone(),
            occurrence.chapter,
            occurrence.verse,
        );
        rect()
            .key(index)
            .width(Size::fill())
            .padding((8.0, 12.0))
            .spacing(2.0)
            .corner_radius(tokens::radius::MD)
            .maybe(here, |r| r.background(p.s2))
            .a11y_role(AccessibilityRole::Button)
            .on_tap(move |_| {
                if in_sheet {
                    // This sheet and the verse's under it.
                    let mut modals = ModalController::get();
                    modals.close();
                    modals.close();
                }
                reader.go(&book, chapter, verse);
            })
            .child(Text::new(place).role(TextRole::Strong).color(p.accent_text))
            .maybe_child((!text.is_empty()).then(|| {
                label()
                    .width(Size::fill())
                    .text(text)
                    .font_family(crate::fonts::FAMILY)
                    .font_size(tokens::text::BODY)
                    .color(p.t2)
                    .max_lines(2)
                    .text_overflow(TextOverflow::Ellipsis)
            }))
            .into_element()
    }
}

/// The card that downloads the packs, with the download's progress.
#[derive(Clone, Debug, PartialEq)]
struct Card {
    progress: Progress,
}

impl Component for Card {
    #[cfg_attr(feature = "sismo", sismo::function("bible::originals_card"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let download = move |_: Event<PressEventData>| {
            if let Some(service) = originals::service() {
                service.spawn_download();
            }
        };
        let action: Element = match &self.progress {
            Progress::Downloading { done, total } => {
                #[allow(clippy::cast_precision_loss)] // file counts
                let fraction = if *total == 0 {
                    0.0
                } else {
                    *done as f32 / *total as f32
                };
                rect()
                    .width(Size::fill())
                    .spacing(6.0)
                    .child(Text::new(tr!("originals-card-downloading")).role(TextRole::Strong))
                    .child(ProgressBar::new(fraction.max(0.02)))
                    .child(
                        Text::new(
                            tr!("originals-card-progress", "done" => *done, "total" => *total),
                        )
                        .role(TextRole::Meta),
                    )
                    .into_element()
            }
            Progress::Aligning { .. } => Text::new(tr!("originals-card-downloading"))
                .role(TextRole::Meta)
                .into_element(),
            Progress::Failed(message) => rect()
                .width(Size::fill())
                .spacing(8.0)
                .child(Notice::new(NoticeTone::Warning, message.clone()))
                .child(
                    Button::new(tr!("originals-card-retry"))
                        .primary()
                        .on_press(download),
                )
                .into_element(),
            Progress::Idle => Button::new(tr!("originals-card-download"))
                .primary()
                .icon(lucide::download())
                .on_press(download)
                .into_element(),
        };
        rect().width(Size::fill()).padding((16.0, 16.0)).child(
            rect()
                .width(Size::fill())
                .padding((20.0, 20.0))
                .spacing(10.0)
                .corner_radius(tokens::radius::LG)
                .background(p.s2)
                .child(Text::new(tr!("originals-card-title")).role(TextRole::Title))
                .child(Text::new(tr!("originals-card-body")).role(TextRole::Body))
                .child(
                    Text::new(tr!("originals-card-sources"))
                        .role(TextRole::Meta)
                        .color(p.t3),
                )
                .child(action),
        )
    }
}

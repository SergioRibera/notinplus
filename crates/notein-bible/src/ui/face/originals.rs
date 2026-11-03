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
use notein_i18n::{Lang, tr};
use notein_sdk::services::{HostToast, ToastTone};
use notein_sdk::ui::SelectionExtra;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, IconButton, ModalController, Notice, NoticeTone, ProgressBar, TapExt, Text, TextRole,
    is_mouse, tokens,
};

use super::Reader;
use crate::db::LinkRow;
use crate::originals::{self, LemmaRef, Progress, Store, WordView, align, morph, respell};
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
        let mut open = reader.refs;
        open.set(Some(refs));
        return;
    }
    super::refs::open_sheet(reader, refs, PanelTab::Original);
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
    let action: Rc<dyn Fn()> = Rc::new(move || {
        reader.ui.close_selection();
        open(&reader, verse, wide);
    });
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
            .map_or_else(|| bible_id.clone(), |b| b.abbrev);
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
        let language = match testament {
            originals::Testament::Old => morph::Language::Hebrew,
            originals::Testament::New => morph::Language::Greek,
        };
        let rows = words
            .into_iter()
            .map(|word| {
                let here = links
                    .iter()
                    .filter(|l| l.position == word.position)
                    .map(|l| align::span_text(&verse_text, l))
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" … ");
                Row {
                    here: (!here.is_empty()).then_some(here),
                    word,
                    language,
                }
            })
            .collect();
        Some(Self {
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

/// The list of words; one row open at a time; the occurrences of a
/// lemma replace the list until «back».
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
        let open: State<Option<u16>> = use_state(|| None);
        let occurrences: State<Option<LemmaRef>> = use_state(|| None);
        if let Some(lemma) = *occurrences.read() {
            return Occurrences {
                reader: self.reader.clone(),
                store: Arc::clone(&self.store),
                lemma,
                back: occurrences,
                in_sheet: self.in_sheet,
            }
            .into_element();
        }
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
        rect()
            .width(Size::fill())
            .spacing(2.0)
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
                        occurrences,
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
    occurrences: State<Option<LemmaRef>>,
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
                    .child(original_text(&word.surface, 20.0, p.t1))
                    .child(
                        label()
                            .text(word.translit.replace('.', ""))
                            .font_size(tokens::text::META)
                            .font_slant(FontSlant::Italic)
                            .color(p.t3),
                    ),
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
        let hebrew = self.row.language == morph::Language::Hebrew;
        let function_word = morph::is_function_word(&word.morph, self.row.language);

        let (base, respelling) = lemma.as_ref().map_or_else(
            || (word.surface.clone(), String::new()),
            |l| {
                let respelling = if hebrew {
                    respell::hebrew(lang, &l.translit, &l.pronunciation_strong)
                } else {
                    respell::greek(lang, &l.lemma, &l.pronunciation_strong)
                };
                (l.lemma.clone(), respelling)
            },
        );
        let Meaning {
            here,
            here_english,
            also,
            also_tag,
        } = self.meaning(lemma.as_ref(), lang);
        let grammar = grammar_line(word, self.row.language);
        let derivation = lemma
            .as_ref()
            .map(|l| l.derivation_en.clone())
            .filter(|d| !d.is_empty());
        let count = lemma.as_ref().map_or(0, |l| l.count);
        let testament = match self.row.language {
            morph::Language::Hebrew => tr!("originals-testament-ot"),
            morph::Language::Greek => tr!("originals-testament-nt"),
        };
        let pronounced = (!respelling.is_empty())
            .then(|| tr!("originals-pronounced", "respelling" => respelling.clone()));
        let copy_text = format!(
            "{base} ({}) — {here}",
            if respelling.is_empty() {
                word.translit.replace('.', "")
            } else {
                respelling
            }
        );

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
                    .child(original_text(&base, 18.0, p.t1))
                    .maybe_child(pronounced.map(|text| {
                        label()
                            .text(text)
                            .font_size(tokens::text::META)
                            .font_slant(FontSlant::Italic)
                            .color(p.t3)
                    })),
            )
            .child(labelled(&tr!("originals-here"), here, here_english))
            .maybe_child((!function_word && !also.is_empty()).then(|| {
                rect()
                    .width(Size::fill())
                    .spacing(2.0)
                    .child(labelled(&tr!("originals-also"), also.join(" · "), false))
                    .maybe_child(
                        also_tag
                            .map(|tag| label().text(tag).font_size(tokens::text::META).color(p.t3)),
                    )
            }))
            .maybe_child((!grammar.is_empty()).then(|| {
                label()
                    .width(Size::fill())
                    .text(
                        derivation
                            .map_or_else(|| grammar.clone(), |d| format!("{grammar}  ·  {d}")),
                    )
                    .font_size(tokens::text::META)
                    .color(p.t3)
            }))
            .maybe_child(function_word.then(|| {
                label()
                    .text(tr!("originals-no-meaning"))
                    .font_size(tokens::text::META)
                    .color(p.t3)
            }))
            .child(self.actions(count, testament, copy_text))
            .into_element()
    }

    /// The occurrence count with «see all» and «copy».
    fn actions(&self, count: usize, testament: String, copy_text: String) -> impl IntoElement {
        let p = palette();
        let lemma_ref = self.row.word.lemma;
        let mut occurrences = self.occurrences;
        let host = Arc::clone(&self.reader.host);
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
                    .on_press(move |_| occurrences.set(Some(lemma_ref)))
            }))
            .child(
                Button::new(tr!("originals-copy"))
                    .small()
                    .ghost()
                    .on_press(move |_| {
                        if freya::clipboard::Clipboard::set(copy_text.clone()).is_ok() {
                            host.toast(HostToast::new(tr!("originals-copied"), ToastTone::Info));
                        }
                    }),
            )
    }
}

/// What the row says the word means.
struct Meaning {
    /// How it reads in this verse.
    here: String,
    /// `here` fell back to the English gloss.
    here_english: bool,
    /// Its other renderings or senses.
    also: Vec<String>,
    /// A note under `also`: whose renderings they are, or that they
    /// are machine translated or English.
    also_tag: Option<String>,
}

impl WordRow {
    /// Here: the aligned span, else the pack's contextual gloss. Also:
    /// how this translation renders the lemma elsewhere, else the senses
    /// in the interface language, else the English ones.
    fn meaning(&self, lemma: Option<&crate::originals::LemmaView>, lang: Lang) -> Meaning {
        let word = &self.row.word;
        let (here, here_english) = match (&self.row.here, &word.gloss) {
            (Some(span), _) => (span.clone(), false),
            (None, Some(gloss)) => (gloss.clone(), false),
            (None, None) => (word.gloss_en.clone(), lang != Lang::En),
        };
        let renderings = self
            .reader
            .kind
            .originals()
            .renderings(&self.words.bible_id, word.lemma);
        if !renderings.is_empty() {
            return Meaning {
                here,
                here_english,
                also: renderings
                    .iter()
                    .map(|r| format!("{} {}", r.token, r.count))
                    .collect(),
                also_tag: Some(tr!("originals-renders", "bible" => self.words.bible_name.clone())),
            };
        }
        let senses = lemma
            .map(|l| self.store.senses(&l.strong, lang.tag()))
            .unwrap_or_default();
        if senses.is_empty() {
            return Meaning {
                here,
                here_english,
                also: lemma.map(|l| l.senses_en.clone()).unwrap_or_default(),
                also_tag: (lang != Lang::En).then(|| tr!("originals-meaning-english")),
            };
        }
        let automatic = senses
            .iter()
            .all(|s| s.origin == originals_format::Origin::Auto);
        Meaning {
            here,
            here_english,
            also: senses.into_iter().map(|s| s.text).collect(),
            also_tag: automatic.then(|| tr!("originals-automatic")),
        }
    }
}

/// The grammar of a word in words, its prefixes and suffixes after it.
fn grammar_line(word: &WordView, language: morph::Language) -> String {
    let mut grammar = morph::describe(&word.morph, language);
    for affix in &word.affixes {
        let affix_grammar = morph::describe(&affix.morph, language).join(", ");
        grammar.push(format!(
            "+ {} ({}{})",
            affix.lemma,
            affix.gloss_en,
            if affix_grammar.is_empty() {
                String::new()
            } else {
                format!(", {affix_grammar}")
            }
        ));
    }
    grammar.join(", ")
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

/// Hebrew or Greek text, in whatever font the system has for the script.
fn original_text(text: &str, size: f32, color: Color) -> impl IntoElement {
    label().text(text.to_owned()).font_size(size).color(color)
}

/// Every place a lemma occurs, in canon order; pressing one goes there.
#[derive(Clone)]
struct Occurrences {
    reader: Reader,
    store: Arc<Store>,
    lemma: LemmaRef,
    back: State<Option<LemmaRef>>,
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
        let lemma = self.store.lemma(self.lemma);
        let title = tr!("originals-occurrences-title", "lemma" => lemma.as_ref().map(|l| l.lemma.clone()).unwrap_or_default());
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
        let mut back = self.back;
        rect()
            .width(Size::fill())
            .spacing(2.0)
            .child(
                rect()
                    .width(Size::fill())
                    .padding((4.0, 8.0, 8.0, 8.0))
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(4.0)
                    .child(
                        IconButton::new(lucide::arrow_left(), tr!("originals-occurrences-back"))
                            .size(32.0)
                            .on_press(move |_| back.set(None)),
                    )
                    .child(
                        rect()
                            .width(Size::flex(1.0))
                            .child(Text::new(title).role(TextRole::Strong).lines(1)),
                    ),
            )
            .children(all.iter().take(MAX_OCCURRENCES).enumerate().map(|(i, occurrence)| {
                let here = occurrence.book_id.eq_ignore_ascii_case(&book)
                    && occurrence.chapter == chapter;
                let place = format!(
                    "{} {}:{}{}",
                    name_of(&occurrence.book_id),
                    occurrence.chapter,
                    occurrence.verse,
                    if here {
                        format!(" · {}", tr!("originals-occurrences-here"))
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
                    ModalController::get().close();
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

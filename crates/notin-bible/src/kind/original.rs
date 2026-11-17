//! Citing the original Hebrew and Greek: a word in its verse, a term of the
//! dictionary, or verses word by word. Quotes, their options, chips and
//! the search that finds them, all through the plugin contract.

use std::sync::Arc;

use notin_i18n::{Lang, tr};
use notin_sdk::kind::KindAction;
use notin_sdk::prelude::*;
use notin_sdk::{GlossedWord, Interlinear};

use super::{BibleKind, KIND};
use crate::anchor::{BibleAnchor, BibleRef, OriginalRef};
use crate::originals::describe::{self, Also};
use crate::originals::{self, LemmaView, Store, Testament, WordView, morph};

/// Quote options of a word in its verse.
const PRONUNCIATION: &str = "pronunciation";
const GRAMMAR: &str = "grammar";
const HERE: &str = "here";
const ALSO: &str = "also";
/// Quote options of a dictionary term.
const SENSES: &str = "senses";
const RENDERINGS: &str = "renderings";
const OCCURRENCES: &str = "occurrences";
const FAMILY: &str = "family";
/// Quote options of verses word by word.
const READING: &str = "reading";
const GLOSS: &str = "gloss";
const TRANSLATION: &str = "translation";

/// Action of an unavailable quote: download the original-language packs.
const DOWNLOAD_ORIGINALS: &str = "download-originals";

/// Places listed under «where it appears» in a term's quote.
const LISTED_OCCURRENCES: usize = 6;
/// Renderings listed in a quote.
const LISTED_RENDERINGS: usize = 4;
/// Occurrences of the best term offered as words of their verse.
const OFFERED_OCCURRENCES: usize = 3;
/// Terms found per search.
const FOUND_TERMS: usize = 3;

/// One word of a verse with everything the quotes say about it.
struct WordAt {
    store: Arc<Store>,
    word: WordView,
    lemma: Option<LemmaView>,
    language: morph::Language,
    /// What the reader's translation makes of it in this verse.
    here: Option<String>,
    bible_id: String,
    bible_name: String,
}

impl BibleKind {
    /// The word at `position` of the start verse of `passage`, read with
    /// the translation the reader uses now.
    fn word_at(&self, passage: &BibleAnchor, position: u16) -> Option<WordAt> {
        let store = self.originals().store()?;
        let testament = originals::testament_of(&passage.book_id)?;
        let lang = Lang::current();
        let words = store.words(
            &passage.book_id,
            passage.start_chapter,
            passage.start_verse,
            lang.tag(),
        )?;
        let word = words.into_iter().find(|w| w.position == position)?;
        let bible_id = self
            .default_bible()
            .unwrap_or_else(|| passage.bible_id.clone());
        let links = self.originals().links(
            &bible_id,
            &passage.book_id,
            passage.start_chapter,
            passage.start_verse,
        );
        let verse_text = self
            .db()
            .verse_text(
                &bible_id,
                &passage.book_id,
                passage.start_chapter,
                passage.start_verse,
            )
            .ok()
            .flatten()
            .unwrap_or_default();
        Some(WordAt {
            lemma: store.lemma(word.lemma),
            here: describe::here_span(&links, &verse_text, position),
            word,
            store,
            language: describe::language_of(testament),
            bible_name: self.translation(&bible_id),
            bible_id,
        })
    }

    /// Options the reader may toggle on a quote of `anchor`.
    pub(super) fn original_options(anchor: &BibleRef) -> Vec<QuoteOption> {
        let option = |id: &'static str, label: String| QuoteOption::new(id, label);
        let options = match anchor.original {
            None => return Vec::new(),
            Some(OriginalRef::Word { .. }) => vec![
                option(PRONUNCIATION, tr!("cite-option-pronunciation")),
                option(GRAMMAR, tr!("cite-option-grammar")),
                option(HERE, tr!("cite-option-here")),
                option(ALSO, tr!("cite-option-also")),
            ],
            Some(OriginalRef::Term { .. }) => vec![
                option(PRONUNCIATION, tr!("cite-option-pronunciation")),
                option(SENSES, tr!("cite-option-senses")),
                option(RENDERINGS, tr!("cite-option-renderings")),
                option(OCCURRENCES, tr!("cite-option-occurrences")),
                option(FAMILY, tr!("cite-option-family")).map(QuoteOption::hidden_by_default),
            ],
            Some(OriginalRef::Interlinear) => vec![
                option(READING, tr!("cite-option-reading")),
                option(GLOSS, tr!("cite-option-gloss")),
                option(TRANSLATION, tr!("cite-option-translation")),
            ],
        };
        options.into_iter().flatten().collect()
    }

    /// A quote of the original text needs the packs: without them, say so
    /// and offer to download them.
    pub(super) fn original_unavailable(&self, anchor: &BibleRef) -> Option<QuoteUnavailable> {
        anchor.original?;
        if self.originals().store().is_some() {
            return None;
        }
        Some(QuoteUnavailable {
            reason: tr!("cite-needs-originals").into(),
            action: cfg!(feature = "importer").then(|| KindAction {
                id: DOWNLOAD_ORIGINALS.into(),
                label: tr!("originals-card-download").into(),
                icon: freya::icons::lucide::download(),
            }),
        })
    }

    /// Run what [`Self::original_unavailable`] offered.
    pub(super) fn run_original_action(&self, id: &str, anchor: &BibleRef) {
        if id != DOWNLOAD_ORIGINALS || anchor.original.is_none() {
            return;
        }
        #[cfg(feature = "importer")]
        self.originals().spawn_download();
    }

    /// The chip label of a place in the original text.
    pub(crate) fn original_label(&self, passage: &BibleAnchor, original: OriginalRef) -> String {
        let place = self.label(passage);
        match original {
            OriginalRef::Word { position } => match self.word_at(passage, position) {
                Some(at) => tr!("cite-word-chip", "word" => at.word.surface, "place" => place),
                None => place,
            },
            OriginalRef::Term { position } => self
                .word_at(passage, position)
                .and_then(|at| at.lemma)
                .map_or(place, |lemma| lemma.lemma),
            OriginalRef::Interlinear => {
                tr!("cite-original-chip", "place" => place, "language" => language_name(&passage.book_id))
            }
        }
    }

    /// The quote of a place in the original text, showing `parts`.
    pub(super) fn original_quote(
        &self,
        doc: DocMeta<'_>,
        passage: &BibleAnchor,
        original: OriginalRef,
        parts: &QuoteParts,
    ) -> Option<Quote> {
        let shows = |id: &str| parts.shows_id(id, id != FAMILY);
        match original {
            OriginalRef::Word { position } => self.word_quote(passage, position, &shows),
            OriginalRef::Term { position } => self.term_quote(doc, passage, position, &shows),
            OriginalRef::Interlinear => self.interlinear_quote(doc, passage, &shows),
        }
    }

    fn word_quote(
        &self,
        passage: &BibleAnchor,
        position: u16,
        shows: &dyn Fn(&str) -> bool,
    ) -> Option<Quote> {
        let at = self.word_at(passage, position)?;
        let lang = Lang::current();
        let family = crate::fonts::original_family(at.language == morph::Language::Hebrew);
        let mut lines = Vec::new();
        let mut title = format!("### {}", original(&at.word.surface, family));
        if shows(PRONUNCIATION) {
            title.push_str(" *");
            title.push_str(&at.word.translit.replace('.', ""));
            title.push('*');
        }
        lines.push(title);
        let mut about = Vec::new();
        if let Some(lemma) = &at.lemma {
            let mut from = tr!("cite-from-lemma", "lemma" => original(&lemma.lemma, family));
            let respelled = describe::respelling(lemma, at.language, lang);
            if shows(PRONUNCIATION) && !respelled.is_empty() {
                from.push_str(", ");
                from.push_str(&tr!("originals-pronounced", "respelling" => respelled));
            }
            about.push(from);
        }
        if shows(GRAMMAR) {
            let grammar = describe::grammar_line(&at.word, at.language);
            if !grammar.is_empty() {
                about.push(capitalized(&grammar));
            }
        }
        if !about.is_empty() {
            lines.push(format!("{}.", about.join(". ")));
        }
        let meaning = describe::meaning(
            self.originals(),
            &at.store,
            &at.bible_id,
            &at.bible_name,
            &at.word,
            at.here.as_deref(),
            at.language,
            lang,
        );
        if shows(HERE) {
            lines.push(format!("**{}:** {}", tr!("originals-here"), meaning.here));
        }
        if shows(ALSO) && !morph::is_function_word(&at.word.morph, at.language) {
            if let Some(also) = also_line(&meaning.also) {
                lines.push(format!("**{}:** {also}", tr!("cite-heading-also")));
            }
        }
        let source = tr!(
            "cite-word-source",
            "place" => self.label(passage),
            "language" => language_name(&passage.book_id)
        );
        Some(Quote::new(lines.join("\n\n"), source))
    }

    fn term_quote(
        &self,
        doc: DocMeta<'_>,
        passage: &BibleAnchor,
        position: u16,
        shows: &dyn Fn(&str) -> bool,
    ) -> Option<Quote> {
        let at = self.word_at(passage, position)?;
        let lemma = at.lemma.clone()?;
        let lang = Lang::current();
        let family = crate::fonts::original_family(at.language == morph::Language::Hebrew);
        let mut lines = Vec::new();
        let mut title = format!("### {}", original(&lemma.lemma, family));
        let respelled = describe::respelling(&lemma, at.language, lang);
        if shows(PRONUNCIATION) {
            title.push_str(" *");
            title.push_str(&lemma.translit);
            title.push('*');
        }
        lines.push(title);
        if shows(PRONUNCIATION) && !respelled.is_empty() {
            lines.push(capitalized(&tr!("originals-pronounced", "respelling" => respelled)) + ".");
        }
        if shows(SENSES)
            && let Also::Senses { items, note } = describe::senses(&at.store, Some(&lemma), lang)
            && !items.is_empty()
        {
            let heading = note.map_or_else(
                || tr!("originals-senses"),
                |note| tr!("originals-senses-noted", "note" => note),
            );
            let list = items
                .iter()
                .enumerate()
                .map(|(i, sense)| format!("{}. {sense}", i + 1))
                .collect::<Vec<_>>()
                .join("\n");
            lines.push(format!("**{heading}**\n\n{list}"));
        }
        let lemma_ref = at.word.lemma;
        if shows(RENDERINGS) {
            let renderings = self.originals().renderings(&at.bible_id, lemma_ref);
            if !renderings.is_empty() {
                let listed = renderings
                    .iter()
                    .take(LISTED_RENDERINGS)
                    .map(|r| format!("{} ({})", r.token, r.count))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!(
                    "**{}:** {listed}",
                    tr!("cite-renders-in", "bible" => at.bible_name.clone())
                ));
            }
        }
        if shows(OCCURRENCES) {
            let occurrences = at.store.occurrences(lemma_ref);
            let chips = occurrences
                .iter()
                .take(LISTED_OCCURRENCES)
                .map(|o| {
                    let place =
                        BibleAnchor::single_verse(&at.bible_id, &o.book_id, o.chapter, o.verse);
                    let label = self.label(&place);
                    let target = word_target(doc.id, place, o.position);
                    format!("[{}]({})", label.replace(['[', ']'], ""), target.to_url())
                })
                .collect::<Vec<_>>()
                .join(" ");
            let testament = match lemma_ref.testament {
                Testament::Old => tr!("originals-testament-ot"),
                Testament::New => tr!("originals-testament-nt"),
            };
            lines.push(format!(
                "**{}:** {chips}",
                tr!("originals-count", "count" => occurrences.len(), "testament" => testament)
            ));
        }
        if shows(FAMILY) {
            let derivation = describe::derivation_text(&at.store, &lemma.derivation_en, lang);
            if !derivation.is_empty() {
                lines.push(format!("**{}:** {derivation}", tr!("cite-family")));
            }
        }
        let source = tr!(
            "cite-term-source",
            "lemma" => lemma.lemma,
            "language" => language_name(&passage.book_id)
        );
        Some(Quote::new(lines.join("\n\n"), source))
    }

    fn interlinear_quote(
        &self,
        doc: DocMeta<'_>,
        passage: &BibleAnchor,
        shows: &dyn Fn(&str) -> bool,
    ) -> Option<Quote> {
        let store = self.originals().store()?;
        let testament = originals::testament_of(&passage.book_id)?;
        let hebrew = testament == Testament::Old;
        let lang = Lang::current();
        let bible_id = self
            .default_bible()
            .unwrap_or_else(|| passage.bible_id.clone());
        let mut words = Vec::new();
        for (chapter, verse) in self.verse_numbers(passage) {
            let found = store.words(&passage.book_id, chapter, verse, lang.tag())?;
            let place = BibleAnchor::single_verse(&bible_id, &passage.book_id, chapter, verse);
            let links = self
                .originals()
                .links(&bible_id, &passage.book_id, chapter, verse);
            let verse_text = self
                .db()
                .verse_text(&bible_id, &passage.book_id, chapter, verse)
                .ok()
                .flatten()
                .unwrap_or_default();
            for word in found {
                let mut glossed = GlossedWord::new(word.surface.clone())
                    .link(word_target(doc.id, place.clone(), word.position).to_url());
                if shows(READING) {
                    glossed = glossed.reading(word.translit.replace('.', ""));
                }
                if shows(GLOSS) {
                    let here =
                        (!morph::is_function_word(&word.morph, describe::language_of(testament)))
                            .then(|| describe::here_span(&links, &verse_text, word.position))
                            .flatten();
                    let gloss = here
                        .or_else(|| word.gloss.clone())
                        .unwrap_or_else(|| word.gloss_en.clone());
                    glossed = glossed.gloss(gloss);
                }
                words.push(glossed);
            }
        }
        if words.is_empty() {
            return None;
        }
        let body = if shows(TRANSLATION) {
            let mut moved = passage.clone();
            moved.bible_id.clone_from(&bible_id);
            self.passage_quote(std::slice::from_ref(&moved))
                .map(|quote| format!("*{}* ({})", quote.body.trim(), self.translation(&bible_id)))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let source = tr!(
            "cite-interlinear-source",
            "place" => self.label(passage),
            "language" => language_name(&passage.book_id)
        );
        let interlinear = Interlinear::new(words)
            .right_to_left(hebrew)
            .font_family(crate::fonts::original_family(hebrew));
        Some(Quote::new(body, source).with_interlinear(interlinear))
    }

    /// `(chapter, verse)` of every verse `passage` covers.
    fn verse_numbers(&self, passage: &BibleAnchor) -> Vec<(u16, u16)> {
        (passage.start_chapter..=passage.end_chapter)
            .flat_map(|chapter| {
                let last = if chapter == passage.end_chapter {
                    passage.end_verse
                } else {
                    self.db()
                        .load_chapter(&passage.bible_id, &passage.book_id, chapter)
                        .map_or(0, |c| c.verses.last().map_or(0, |v| v.verse))
                };
                let first = if chapter == passage.start_chapter {
                    passage.start_verse
                } else {
                    1
                };
                (first.max(1)..=last).map(move |verse| (chapter, verse))
            })
            .collect()
    }

    /// Hits in the original text for `query`: verses word by word when it
    /// names a passage followed by «original», «griego» or «hebreo»; the
    /// dictionary terms it spells (in Greek, Hebrew or their
    /// transliteration), and where the best one appears, nearest to
    /// `near` first. A passage after the term («makar lc 6») narrows
    /// where.
    pub(super) fn original_hits(&self, query: &SearchQuery) -> Vec<Hit<BibleRef>> {
        let Some(store) = self.originals().store() else {
            return Vec::new();
        };
        let Some(bible) = self.default_bible() else {
            return Vec::new();
        };
        if let Some(hit) = self.interlinear_hit(&bible, &query.text) {
            return vec![hit];
        }
        let mut words = query.text.split_whitespace();
        let Some(term) = words.next() else {
            return Vec::new();
        };
        let within = {
            let rest = words.collect::<Vec<_>>().join(" ");
            (!rest.is_empty())
                .then(|| crate::resolver::resolve(self.db(), &bible, &rest))
                .flatten()
        };
        let near = near_verses(&query.near);
        let mut hits = Vec::new();
        for (rank, lemma_ref) in store.find_lemmas(term, FOUND_TERMS).into_iter().enumerate() {
            let Some(lemma) = store.lemma(lemma_ref) else {
                continue;
            };
            let mut occurrences = store.occurrences(lemma_ref);
            if let Some(within) = &within {
                occurrences.retain(|o| in_passage(within, &o.book_id, o.chapter, o.verse));
            }
            occurrences.sort_by_key(|o| {
                !near
                    .iter()
                    .any(|n| in_passage(n, &o.book_id, o.chapter, o.verse))
            });
            let Some(first) = occurrences.first() else {
                continue;
            };
            let first_place =
                BibleAnchor::single_verse(&bible, &first.book_id, first.chapter, first.verse);
            let gloss = lemma
                .senses_en
                .first()
                .cloned()
                .unwrap_or_else(|| lemma.gloss_en.clone());
            hits.push(Hit {
                doc: None,
                title: tr!("cite-term-hit", "lemma" => lemma.lemma.clone(), "translit" => lemma.translit.clone()),
                snippet: tr!("cite-term-hit-snippet", "gloss" => gloss, "count" => lemma.count),
                anchor: BibleRef::original(first_place, OriginalRef::Term { position: first.position }),
                score: 0.0,
            });
            if rank > 0 {
                continue;
            }
            for occurrence in occurrences.iter().take(OFFERED_OCCURRENCES) {
                let place = BibleAnchor::single_verse(
                    &bible,
                    &occurrence.book_id,
                    occurrence.chapter,
                    occurrence.verse,
                );
                let Some(at) = self.word_at(&place, occurrence.position) else {
                    continue;
                };
                let here = at
                    .here
                    .clone()
                    .or_else(|| at.word.gloss.clone())
                    .unwrap_or_else(|| at.word.gloss_en.clone());
                hits.push(Hit {
                    doc: None,
                    title: tr!("cite-word-chip", "word" => at.word.surface.clone(), "place" => self.label(&place)),
                    snippet: here,
                    anchor: BibleRef::original(
                        place,
                        OriginalRef::Word {
                            position: occurrence.position,
                        },
                    ),
                    score: 0.0,
                });
            }
        }
        hits
    }

    /// The passage `text` names followed by one of the words that ask for
    /// the original, as verses word by word.
    fn interlinear_hit(&self, bible: &str, text: &str) -> Option<Hit<BibleRef>> {
        let keywords = tr!("cite-search-keywords");
        let folded = notin_sdk::fold(text);
        let rest = keywords
            .split(',')
            .map(|k| notin_sdk::fold(k.trim()))
            .filter(|k| !k.is_empty())
            .find_map(|k| folded.strip_suffix(&k).map(str::trim))?;
        let passage = crate::resolver::resolve(self.db(), bible, rest)?;
        originals::testament_of(&passage.book_id)?;
        let label = self.label(&passage);
        Some(Hit {
            doc: None,
            title: tr!("cite-original-chip", "place" => label, "language" => language_name(&passage.book_id)),
            snippet: tr!("cite-interlinear-snippet"),
            anchor: BibleRef::original(passage, OriginalRef::Interlinear),
            score: f32::MAX,
        })
    }
}

/// The deep link to the word at `position` of `place`, in document `doc`.
fn word_target(doc: DocId, place: BibleAnchor, position: u16) -> SourceRef {
    SourceRef::Plugin {
        plugin_id: KIND.to_owned(),
        item: doc.0,
        anchor: BibleRef::original(place, OriginalRef::Word { position }).to_bytes(),
        selection: None,
    }
}

/// `text` in the original font `family`, as markdown.
fn original(text: &str, family: &str) -> String {
    format!("<font face=\"{family}\">{text}</font>")
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// «Griego» or «hebreo» for the book `book_id`.
fn language_name(book_id: &str) -> String {
    match originals::testament_of(book_id) {
        Some(Testament::Old) => tr!("cite-language-hebrew"),
        _ => tr!("cite-language-greek"),
    }
}

/// The «also» block of a word as one line; `None` when there is nothing.
fn also_line(also: &Also) -> Option<String> {
    let line = match also {
        Also::Renderings { items, .. } => items
            .iter()
            .take(LISTED_RENDERINGS)
            .map(|(token, count)| format!("{token} ({count})"))
            .collect::<Vec<_>>()
            .join(", "),
        Also::Senses { items, .. } => items.join(", "),
    };
    (!line.is_empty()).then_some(line)
}

/// The verses of the Bible references among `near`, in order.
fn near_verses(near: &[SourceRef]) -> Vec<BibleAnchor> {
    near.iter()
        .filter_map(|place| match place {
            SourceRef::Plugin {
                plugin_id, anchor, ..
            } if plugin_id == KIND => BibleRef::from_bytes(anchor).ok().map(|r| r.passage),
            _ => None,
        })
        .collect()
}

/// Whether verse `chapter:verse` of `book_id` is inside `passage`. A
/// passage of verse 0 covers its whole chapter.
fn in_passage(passage: &BibleAnchor, book_id: &str, chapter: u16, verse: u16) -> bool {
    if !passage.book_id.eq_ignore_ascii_case(book_id) {
        return false;
    }
    let after_start = (chapter, verse) >= (passage.start_chapter, passage.start_verse);
    let end_verse = if passage.end_verse == 0 {
        u16::MAX
    } else {
        passage.end_verse
    };
    let before_end = (chapter, verse) <= (passage.end_chapter, end_verse);
    after_start && before_end
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::*;
    use crate::db::Db;

    /// Numbers the sample folders, one per test running at once.
    static NEXT_SAMPLE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// A kind with Mateo 5:3 in one translation and the sample New
    /// Testament pack.
    fn matthew() -> BibleKind {
        let db = Db::in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO bibles (id, abbrev, name_local, language)
                 VALUES ('spa_rv1960', 'RVR1960', 'Reina Valera 1960', 'spa')",
                [],
            )?;
            conn.execute(
                "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
                 VALUES ('spa_rv1960', 'MAT', 'Mateo', 'Mt', 40, 'NT')",
                [],
            )?;
            conn.execute(
                "INSERT INTO verses (bible_id, book_id, chapter, verse, text)
                 VALUES ('spa_rv1960', 'MAT', 5, 3, ?1)",
                params!["Bienaventurados los pobres en espíritu."],
            )?;
            Ok(())
        })
        .unwrap();
        let n = NEXT_SAMPLE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("notein-cite-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::originals::write_sample(&dir);
        BibleKind::with_originals_at(db, dir)
    }

    fn meta() -> DocMeta<'static> {
        DocMeta {
            id: DocId([7; 16]),
            name: "Biblia",
            updated_at: 0,
            tint: None,
            paper: notin_sdk::create::Paper::default(),
        }
    }

    fn verse() -> BibleAnchor {
        BibleAnchor::single_verse("spa_rv1960", "MAT", 5, 3)
    }

    #[test]
    fn a_word_quotes_itself_in_its_font_and_hides_what_the_reader_hides() {
        let kind = matthew();
        let word = OriginalRef::Word { position: 0 };
        let quote = kind
            .original_quote(meta(), &verse(), word, &QuoteParts::default())
            .expect("quote");
        assert!(
            quote
                .body
                .contains("<font face=\"Noto Serif\">Μακάριοι</font>"),
            "{}",
            quote.body
        );
        assert!(quote.body.contains("μακάριος"), "{}", quote.body);
        assert!(quote.body.contains("Makarioi"), "{}", quote.body);
        assert!(quote.source.contains("5:3"), "{}", quote.source);

        let options = BibleKind::original_options(&BibleRef::original(verse(), word));
        let pronunciation = options
            .iter()
            .find(|o| o.id == PRONUNCIATION)
            .expect("option");
        let mut parts = QuoteParts::default();
        parts.set(pronunciation, false);
        let hidden = kind
            .original_quote(meta(), &verse(), word, &parts)
            .expect("quote");
        assert!(!hidden.body.contains("Makarioi"), "{}", hidden.body);
        assert!(kind.original_label(&verse(), word).contains("Μακάριοι"));
    }

    #[test]
    fn a_term_lists_where_it_appears_as_links_to_its_words() {
        let kind = matthew();
        let term = OriginalRef::Term { position: 0 };
        let quote = kind
            .original_quote(meta(), &verse(), term, &QuoteParts::default())
            .expect("quote");
        assert!(quote.body.contains("μακάριος"), "{}", quote.body);
        assert!(quote.body.contains("notein://"), "{}", quote.body);
        assert!(quote.source.contains("STEPBible"), "{}", quote.source);
        assert_eq!(kind.original_label(&verse(), term), "μακάριος");
    }

    #[test]
    fn verses_word_by_word_carry_each_word_with_its_link() {
        let kind = matthew();
        let quote = kind
            .original_quote(
                meta(),
                &verse(),
                OriginalRef::Interlinear,
                &QuoteParts::default(),
            )
            .expect("quote");
        let interlinear = quote.interlinear.expect("interlinear");
        assert_eq!(interlinear.words.len(), 2);
        assert_eq!(interlinear.words[0].text, "Μακάριοι");
        assert!(interlinear.words[0].link.is_some());
        assert!(!interlinear.right_to_left);
        assert_eq!(interlinear.font_family.as_deref(), Some("Noto Serif"));
        assert!(quote.body.contains("Bienaventurados"), "{}", quote.body);
    }

    #[test]
    fn a_search_finds_the_term_then_where_it_appears() {
        let kind = matthew();
        let hits = kind.original_hits(&SearchQuery::new("makar", 5));
        assert!(
            matches!(hits[0].anchor.original, Some(OriginalRef::Term { .. })),
            "{hits:?}"
        );
        assert!(
            hits.iter()
                .any(|h| matches!(h.anchor.original, Some(OriginalRef::Word { position: 0 })))
        );
        let original = kind.original_hits(&SearchQuery::new("Mateo 5:3 griego", 5));
        assert!(
            matches!(original[0].anchor.original, Some(OriginalRef::Interlinear)),
            "{original:?}"
        );
    }

    #[test]
    fn verses_fall_inside_their_passage() {
        let mut passage = BibleAnchor::single_verse("x", "MAT", 5, 3);
        passage.end_verse = 10;
        assert!(in_passage(&passage, "mat", 5, 3));
        assert!(in_passage(&passage, "MAT", 5, 10));
        assert!(!in_passage(&passage, "MAT", 5, 11));
        assert!(!in_passage(&passage, "LUK", 5, 4));
        let chapter = BibleAnchor::single_verse("x", "LUK", 6, 0);
        assert!(in_passage(&chapter, "LUK", 6, 20));
    }

    #[test]
    fn the_first_letter_goes_up() {
        assert_eq!(capitalized("adjetivo, plural"), "Adjetivo, plural");
        assert_eq!(capitalized(""), "");
    }
}

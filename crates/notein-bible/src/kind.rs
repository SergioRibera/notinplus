//! The Biblia kind.

use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

use freya_icons::lucide;
use notein_i18n::{Lang, tr};
use notein_sdk::create::{
    Created, CreationChoice, CreationEntry, CreationError, CreationFlow, CreationRequest,
};
use notein_sdk::doc::DocMeta;
use notein_sdk::kind::{
    ActionEffect, ChipView, DocumentKind, Hit, KindAction, KindChoice, KindColor, KindInfo, Mark,
    OutlineIcon, OutlineNode, Quote, SearchQuery, SelectionSupport,
};

use crate::anchor::BibleAnchor;
use crate::db::{BibleRow, BookRow, Db};
use crate::plugin_config::{self, SharedConfig};
use crate::state::{BibleSnapshot, u32_to_rgba};

/// Kind id stored with the Bible document.
pub const KIND: &str = "io.notein.bible";
/// Create entry that opens (or creates) the Bible.
pub const ENTRY_BIBLE: &str = "bible";
/// What the create sheet's last row launches: the catalog of
/// translations.
pub const ENTRY_CATALOG: &str = "catalog";

/// ISO 639-3 code of the interface language, the prefix translation ids
/// carry (`spa_rvr1960`): pickers offer those translations first.
#[must_use]
pub fn interface_language() -> &'static str {
    match Lang::current() {
        Lang::En => "eng",
        Lang::Es => "spa",
    }
}

/// ISO 639-3 language of translation `bible_id`, its id prefix.
#[must_use]
pub fn language_of(bible_id: &str) -> &str {
    bible_id.split('_').next().unwrap_or_default()
}

/// What the shell stores for the Bible document: reading position and
/// highlights. Highlights are per verse, so they survive a change of
/// translation.
pub type BibleContent = BibleSnapshot;

/// The Bible kind, backed by the translations database.
#[derive(Debug)]
pub struct BibleKind {
    db: Db,
    config: SharedConfig,
    books: RwLock<HashMap<String, Vec<BookRow>>>,
    #[cfg(feature = "importer")]
    installer: std::sync::OnceLock<crate::installer::Installer>,
}

impl BibleKind {
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_new"))]
    pub fn new(db: Db, config: SharedConfig) -> Self {
        Self {
            db,
            config,
            books: RwLock::new(HashMap::new()),
            #[cfg(feature = "importer")]
            installer: std::sync::OnceLock::new(),
        }
    }

    /// The downloads of translations into this kind's database.
    #[cfg(feature = "importer")]
    pub fn installer(&self) -> &crate::installer::Installer {
        self.installer
            .get_or_init(|| crate::installer::Installer::new(self.db.clone(), self.config.clone()))
    }

    #[must_use]
    pub const fn db(&self) -> &Db {
        &self.db
    }

    #[must_use]
    pub const fn config(&self) -> &SharedConfig {
        &self.config
    }

    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_books"))]
    fn books_of(&self, bible: &str) -> Vec<BookRow> {
        if let Some(books) = self
            .books
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(bible)
        {
            return books.clone();
        }
        let books = self.db.list_books(bible).unwrap_or_default();
        self.books
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(bible.to_owned(), books.clone());
        books
    }

    /// Drop cached book lists (after installing a translation).
    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_refresh"))]
    pub fn refresh(&self) {
        self.books
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    fn book_name(&self, bible: &str, book: &str) -> String {
        self.books_of(bible)
            .into_iter()
            .find(|b| b.book_id == book)
            .map_or_else(|| book.to_owned(), |b| b.name)
    }

    /// "Romanos 8:16–17" for an anchor.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_label"))]
    pub fn label(&self, anchor: &BibleAnchor) -> String {
        let book = self.book_name(&anchor.bible_id, &anchor.book_id);
        if anchor.start_chapter != anchor.end_chapter {
            format!(
                "{book} {}:{}–{}:{}",
                anchor.start_chapter, anchor.start_verse, anchor.end_chapter, anchor.end_verse
            )
        } else if anchor.start_verse == anchor.end_verse {
            format!("{book} {}:{}", anchor.start_chapter, anchor.start_verse)
        } else {
            format!(
                "{book} {}:{}–{}",
                anchor.start_chapter, anchor.start_verse, anchor.end_verse
            )
        }
    }

    /// Text of every verse `anchor` covers, `(verse, text)` in order.
    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_verses"))]
    fn verses(&self, anchor: &BibleAnchor) -> Vec<(u16, String)> {
        (anchor.start_chapter..=anchor.end_chapter)
            .filter_map(|chapter| {
                let row = self
                    .db
                    .load_chapter(&anchor.bible_id, &anchor.book_id, chapter)
                    .ok()?;
                Some((chapter, row.verses))
            })
            .flat_map(|(chapter, verses)| {
                verses.into_iter().filter(move |v| {
                    (chapter > anchor.start_chapter || v.verse >= anchor.start_verse)
                        && (chapter < anchor.end_chapter || v.verse <= anchor.end_verse)
                })
            })
            .map(|v| (v.verse, v.text))
            .collect()
    }

    /// Short name of translation `bible`, as a citation ends with it: its
    /// abbreviation, or its name when the abbreviation is only the id.
    fn translation(&self, bible: &str) -> String {
        self.db
            .list_bibles()
            .unwrap_or_default()
            .into_iter()
            .find(|b| b.id == bible)
            .map_or_else(
                || bible.to_owned(),
                |b| {
                    if b.abbrev.is_empty() || b.abbrev.eq_ignore_ascii_case(&b.id) {
                        b.name_local
                    } else {
                        b.abbrev
                    }
                },
            )
    }

    /// The verses `anchors` cover and where they come from.
    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_passage_quote"))]
    fn passage_quote(&self, anchors: &[BibleAnchor]) -> Option<Quote> {
        let first = anchors.first()?;
        let verses: Vec<(u16, String)> = anchors.iter().flat_map(|a| self.verses(a)).collect();
        let numbered = verses.len() > 1;
        let body = verses
            .iter()
            .map(|(verse, text)| {
                if numbered {
                    format!("{} {}", superscript(*verse), text.trim())
                } else {
                    text.trim().to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        (!body.is_empty()).then(|| Quote {
            body,
            source: format!(
                "{} ({})",
                self.label_of_passages(anchors),
                self.translation(&first.bible_id)
            ),
        })
    }

    /// "Génesis 1:1, 3–4" for passages of one chapter, each label joined
    /// by "; " otherwise.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_label_passages"))]
    pub fn label_of_passages(&self, anchors: &[BibleAnchor]) -> String {
        let one_chapter = anchors.first().filter(|first| {
            anchors.iter().all(|a| {
                a.book_id == first.book_id
                    && a.start_chapter == first.start_chapter
                    && a.start_chapter == a.end_chapter
            })
        });
        match (anchors, one_chapter) {
            ([only], _) => self.label(only),
            (_, Some(first)) => {
                let verses = anchors
                    .iter()
                    .map(|a| {
                        if a.start_verse == a.end_verse {
                            a.start_verse.to_string()
                        } else {
                            format!("{}–{}", a.start_verse, a.end_verse)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{} {}:{verses}",
                    self.book_name(&first.bible_id, &first.book_id),
                    first.start_chapter
                )
            }
            _ => anchors
                .iter()
                .map(|a| self.label(a))
                .collect::<Vec<_>>()
                .join("; "),
        }
    }

    /// The verses `anchors` cover as plain text for other apps, closed by
    /// their reference.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_shared_text"))]
    pub fn shared_text(&self, anchors: &[BibleAnchor]) -> Option<String> {
        self.passage_quote(anchors)
            .map(|quote| format!("{}\n\n— {}", quote.body, quote.source))
    }

    /// The passage `text` names, when it reads as a reference that
    /// exists in translation `bible`.
    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_passage"))]
    fn passage(&self, bible: &str, text: &str) -> Option<Hit<BibleAnchor>> {
        let anchor = crate::resolver::resolve(&self.db, bible, text)?;
        let verses = self.verses(&anchor);
        let (_, first) = verses.first()?;
        Some(Hit {
            doc: None,
            title: self.label(&anchor),
            snippet: first.clone(),
            anchor,
            score: f32::MAX,
        })
    }

    /// Translation the user reads by default.
    #[must_use]
    pub fn default_bible(&self) -> Option<String> {
        Some(plugin_config::effective_default_bible(
            &self.config,
            &self.db,
        ))
        .filter(|id| !id.is_empty())
    }

    /// Installed translations, those in the interface language first.
    fn installed_for_picking(&self) -> Vec<BibleRow> {
        let mut bibles = self.db.list_bibles().unwrap_or_default();
        let interface = interface_language();
        bibles.sort_by_key(|b| language_of(&b.id) != interface);
        bibles
    }

    /// Name of a new Bible left unnamed: "Bible KJV".
    fn default_name(&self, bible_id: &str) -> String {
        self.db
            .list_bibles()
            .unwrap_or_default()
            .into_iter()
            .find(|b| b.id == bible_id)
            .map_or_else(
                || tr!("kind-default-name"),
                |b| tr!("kind-default-name-of", "abbrev" => b.abbrev),
            )
    }
}

impl DocumentKind for BibleKind {
    type Anchor = BibleAnchor;
    type Content = BibleContent;
    type Hits<'a> = std::vec::IntoIter<Hit<BibleAnchor>>;

    fn info(&self) -> KindInfo {
        KindInfo {
            id: KIND.into(),
            label: tr!("kind-label").into(),
            plural: tr!("kind-plural").into(),
            search_group: tr!("kind-search-group").into(),
            icon: lucide::book_open(),
            color: KindColor::Bible,
            supports_ink: false,
            has_paper: false,
            attachment: None,
            shared_refs: true,
            tint: false,
            notebook: false,
            body: false,
            records: true,
        }
    }

    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_search"))]
    fn search<'a>(&'a self, query: &'a SearchQuery) -> Self::Hits<'a> {
        let (open, close) = crate::db::snippet_markers();
        let bible = self.default_bible();
        let passage = bible
            .as_deref()
            .and_then(|bible| self.passage(bible, &query.text));
        let found = self
            .db
            .search(bible.as_deref(), &query.text, query.limit)
            .unwrap_or_default()
            .into_iter()
            .map(|hit| {
                let anchor =
                    BibleAnchor::single_verse(&hit.bible_id, &hit.book_id, hit.chapter, hit.verse);
                Hit {
                    doc: None,
                    title: self.label(&anchor),
                    snippet: hit.snippet.replace(open, "").replace(close, ""),
                    anchor,
                    #[allow(clippy::cast_possible_truncation)] // bm25 scores are small
                    score: -(hit.score as f32),
                }
            });
        passage
            .into_iter()
            .chain(found)
            .take(query.limit)
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_outline"))]
    fn outline(&self, content: &BibleContent) -> Vec<OutlineNode<BibleAnchor>> {
        let bible = &content.bible_id;
        if bible.is_empty() {
            return Vec::new();
        }
        self.books_of(bible)
            .into_iter()
            .map(|book| {
                let chapters = self.db.chapter_count(bible, &book.book_id).unwrap_or(0);
                let children = (1..=chapters)
                    .map(|chapter| {
                        OutlineNode::new(
                            format!("{} {chapter}", book.name),
                            Some(BibleAnchor::single_verse(bible, &book.book_id, chapter, 0)),
                        )
                        .icon(OutlineIcon::Page)
                    })
                    .collect();
                OutlineNode::new(book.name, None)
                    .icon(OutlineIcon::Book)
                    .children(children)
            })
            .collect()
    }

    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_marks"))]
    fn marks(&self, content: &BibleContent) -> Vec<Mark<BibleAnchor>> {
        content
            .highlights
            .iter()
            .map(|h| {
                let mut anchor = BibleAnchor::single_verse(
                    &content.bible_id,
                    &h.book_id,
                    h.chapter,
                    h.start_verse,
                );
                anchor.end_verse = h.end_verse;
                Mark {
                    title: self.label(&anchor),
                    snippet: h.note.clone(),
                    color: h.color.map(|packed| {
                        let [r, g, b, _] = u32_to_rgba(packed);
                        [r, g, b]
                    }),
                    updated_at: 0,
                    anchor,
                }
            })
            .collect()
    }

    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_summary"))]
    fn summary(&self, content: &BibleContent) -> Option<String> {
        let bible = self
            .db
            .list_bibles()
            .ok()?
            .into_iter()
            .find(|b| b.id == content.bible_id)?;
        let name = if bible.name_local.trim().is_empty() {
            self.translation(&bible.id)
        } else {
            bible.name_local
        };
        Some(match content.highlights.len() {
            0 => name,
            n => tr!("kind-meta-highlights", "name" => name, "count" => n),
        })
    }

    fn chip(&self, _doc: DocMeta<'_>, anchor: &BibleAnchor) -> ChipView {
        ChipView::new(self.label(anchor))
    }

    fn quotes(&self) -> bool {
        true
    }

    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_quote"))]
    fn quote(&self, _doc: DocMeta<'_>, anchor: &BibleAnchor) -> Option<Quote> {
        self.passage_quote(std::slice::from_ref(anchor))
    }

    fn selection(&self) -> SelectionSupport {
        SelectionSupport::TEXT
    }

    fn actions(&self) -> Vec<KindAction> {
        vec![KindAction {
            id: "change-translation".into(),
            label: tr!("kind-action-change-translation").into(),
            icon: lucide::languages(),
        }]
    }

    fn run_action(&self, _id: &str, _doc: DocMeta<'_>, _content: &BibleContent) -> ActionEffect {
        ActionEffect::Edit
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        let installed = self.installed_for_picking();
        let entry = CreationEntry::new(
            ENTRY_BIBLE,
            tr!("kind-entry-label"),
            lucide::book_open(),
            KindColor::Bible,
        )
        .subtitle(tr!("kind-entry-subtitle"))
        .starter(3)
        .sheet(
            tr!("kind-sheet-title"),
            Some(tr!("kind-sheet-name-hint")),
            true,
        );
        let entry = if installed.len() > 1 {
            entry.flow(CreationFlow::Choose {
                title: tr!("kind-choose-title").into(),
                options: installed
                    .into_iter()
                    .map(|b| CreationChoice {
                        subtitle: b.language.into(),
                        label: format!("{} · {}", b.abbrev, b.name_local).into(),
                        id: b.id.into(),
                    })
                    .collect(),
                more: Some(CreationChoice {
                    id: ENTRY_CATALOG.into(),
                    label: tr!("kind-choose-add-translation").into(),
                    subtitle: "".into(),
                }),
                named: true,
            })
        } else if installed.is_empty() {
            // The face installs the first translation; there is nothing to
            // name yet.
            entry.flow(CreationFlow::Direct)
        } else {
            entry.flow(CreationFlow::Form)
        };
        vec![entry]
    }

    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_create"))]
    fn create(
        &self,
        entry: &str,
        request: CreationRequest<'_>,
    ) -> Result<Created<BibleContent>, CreationError> {
        if entry != ENTRY_BIBLE {
            return Err(CreationError::UnknownEntry(entry.to_owned()));
        }
        let bible_id = request
            .choice
            .map(str::to_owned)
            .or_else(|| self.default_bible())
            .unwrap_or_default();
        let first_book = self
            .books_of(&bible_id)
            .into_iter()
            .next()
            .map(|b| b.book_id);
        let name = if request.name.trim().is_empty() {
            self.default_name(&bible_id)
        } else {
            request.name.trim().to_owned()
        };
        Ok(Created {
            name,
            content: BibleSnapshot {
                version: BibleSnapshot::VERSION,
                current_book: first_book.unwrap_or_default(),
                current_chapter: 1,
                highlights: Vec::new(),
                bible_id,
            },
            attachment: None,
        })
    }

    #[cfg_attr(feature = "sismo", sismo::function("bible::kind_launch"))]
    fn launch(&self, entry: &str) {
        #[cfg(feature = "ui")]
        if entry == ENTRY_CATALOG {
            crate::ui::catalog::open();
        }
        #[cfg(not(feature = "ui"))]
        let _ = entry;
    }

    fn choice(&self, content: &BibleContent) -> Option<KindChoice> {
        Some(KindChoice {
            label: tr!("kind-choice-label").into(),
            options: self
                .installed_for_picking()
                .into_iter()
                .map(|b| CreationChoice {
                    label: format!("{} · {}", b.abbrev, b.name_local).into(),
                    subtitle: b.language.into(),
                    id: b.id.into(),
                })
                .collect(),
            current: Some(content.bible_id.clone()),
        })
    }

    fn choose(&self, content: &mut BibleContent, id: &str) -> bool {
        if content.bible_id == id {
            return false;
        }
        id.clone_into(&mut content.bible_id);
        true
    }

    fn without_marks(&self, content: &mut BibleContent) {
        content.highlights.clear();
    }
}

/// `n` written in superscript digits, as verse numbers are printed.
fn superscript(n: u16) -> String {
    const DIGITS: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    n.to_string()
        .bytes()
        .map(|digit| DIGITS[usize::from(digit - b'0')])
        .collect()
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::*;

    fn genesis() -> BibleKind {
        let db = Db::in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO bibles (id, abbrev, name_local, language)
                 VALUES ('spa_rv1960', 'RVR1960', 'Reina Valera 1960', 'spa')",
                [],
            )?;
            conn.execute(
                "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
                 VALUES ('spa_rv1960', 'GEN', 'Génesis', 'Gn', 1, 'AT')",
                [],
            )?;
            for (verse, text) in [
                (1, "En el principio creó Dios los cielos y la tierra."),
                (2, "Y la tierra estaba desordenada y vacía."),
                (3, "Y dijo Dios: Sea la luz; y fue la luz."),
            ] {
                conn.execute(
                    "INSERT INTO verses (bible_id, book_id, chapter, verse, text)
                     VALUES ('spa_rv1960', 'GEN', 1, ?1, ?2)",
                    params![verse, text],
                )?;
            }
            Ok(())
        })
        .unwrap();
        BibleKind::new(db, SharedConfig::default())
    }

    fn meta() -> DocMeta<'static> {
        DocMeta {
            id: notein_sdk::DocId([0; 16]),
            name: "Biblia",
            updated_at: 0,
            tint: None,
        }
    }

    #[test]
    fn a_typed_reference_comes_first_however_it_is_spelled() {
        let kind = genesis();
        for typed in ["genesis 1:3", "Génesis 1:3", "gn 1:3", "GEN 1.3"] {
            let hits: Vec<_> = kind.search(&SearchQuery::new(typed, 6)).collect();
            assert_eq!(hits[0].title, "Génesis 1:3", "{typed}");
        }
    }

    #[test]
    fn a_range_quotes_every_verse_with_its_number() {
        let kind = genesis();
        let hits: Vec<_> = kind.search(&SearchQuery::new("gen 1:2-3", 6)).collect();
        let quote = kind.quote(meta(), &hits[0].anchor).unwrap();
        assert_eq!(
            quote.body,
            "² Y la tierra estaba desordenada y vacía. ³ Y dijo Dios: Sea la luz; y fue la luz."
        );
        assert_eq!(quote.source, "Génesis 1:2–3 (RVR1960)");
    }

    #[test]
    fn one_verse_is_quoted_without_a_number() {
        let kind = genesis();
        let anchor = BibleAnchor::single_verse("spa_rv1960", "GEN", 1, 1);
        let quote = kind.quote(meta(), &anchor).unwrap();
        assert_eq!(
            quote.body,
            "En el principio creó Dios los cielos y la tierra."
        );
        assert_eq!(quote.source, "Génesis 1:1 (RVR1960)");
    }

    #[test]
    fn shared_verses_close_with_their_reference() {
        let kind = genesis();
        let mut anchor = BibleAnchor::single_verse("spa_rv1960", "GEN", 1, 1);
        anchor.end_verse = 2;
        assert_eq!(
            kind.shared_text(std::slice::from_ref(&anchor)).as_deref(),
            Some(
                "¹ En el principio creó Dios los cielos y la tierra. ² Y la tierra estaba desordenada y vacía.\n\n— Génesis 1:1–2 (RVR1960)"
            )
        );
    }

    #[test]
    fn a_translation_without_its_own_abbreviation_goes_by_its_name() {
        let kind = genesis();
        kind.db
            .with_conn(|conn| {
                conn.execute("UPDATE bibles SET abbrev = id WHERE id = 'spa_rv1960'", [])?;
                Ok(())
            })
            .unwrap();
        let anchor = BibleAnchor::single_verse("spa_rv1960", "GEN", 1, 1);
        assert_eq!(
            kind.quote(meta(), &anchor).unwrap().source,
            "Génesis 1:1 (Reina Valera 1960)"
        );
        let content = BibleContent {
            bible_id: "spa_rv1960".to_owned(),
            ..BibleContent::default()
        };
        assert_eq!(kind.summary(&content).as_deref(), Some("Reina Valera 1960"));
    }

    #[test]
    fn a_missing_passage_quotes_nothing() {
        let kind = genesis();
        let anchor = BibleAnchor::single_verse("spa_rv1960", "GEN", 9, 9);
        assert!(kind.quote(meta(), &anchor).is_none());
    }
}

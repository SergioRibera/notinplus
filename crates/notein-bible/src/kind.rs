//! The Biblia kind.

use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

use freya_icons::lucide;
use notein_sdk::create::{
    Created, CreationChoice, CreationEntry, CreationError, CreationFlow, CreationRequest,
};
use notein_sdk::doc::DocMeta;
use notein_sdk::kind::{
    ActionEffect, ChipView, DocumentKind, Hit, KindAction, KindChoice, KindColor, KindInfo, Mark,
    OutlineIcon, OutlineNode, Quote, SearchQuery, SelectionSupport,
};

use crate::anchor::BibleAnchor;
use crate::db::{BookRow, Db};
use crate::plugin_config::{self, SharedConfig};
use crate::state::{BibleSnapshot, u32_to_rgba};

/// Kind id stored with the Bible document.
pub const KIND: &str = "io.notein.bible";
/// Create entry that opens (or creates) the Bible.
pub const ENTRY_BIBLE: &str = "bible";
/// What the create sheet's last row launches: the catalog of
/// translations.
pub const ENTRY_CATALOG: &str = "catalog";

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

    /// The verses `anchor` covers and where they come from.
    fn passage_quote(&self, anchor: &BibleAnchor) -> Option<Quote> {
        let verses = self.verses(anchor);
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
                self.label(anchor),
                self.translation(&anchor.bible_id)
            ),
        })
    }

    /// The passage `text` names, when it reads as a reference that
    /// exists in translation `bible`.
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
}

impl DocumentKind for BibleKind {
    type Anchor = BibleAnchor;
    type Content = BibleContent;
    type Hits<'a> = std::vec::IntoIter<Hit<BibleAnchor>>;

    fn info(&self) -> KindInfo {
        KindInfo {
            id: KIND.into(),
            label: "Biblia".into(),
            plural: "Biblia".into(),
            search_group: "En la Biblia".into(),
            icon: lucide::book_open(),
            color: KindColor::Bible,
            supports_ink: false,
            has_paper: false,
            attachment: None,
            singleton: true,
            tint: false,
            notebook: false,
            body: false,
        }
    }

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
            1 => format!("{name} · 1 resaltado"),
            n => format!("{name} · {n} resaltados"),
        })
    }

    fn chip(&self, _doc: DocMeta<'_>, anchor: &BibleAnchor) -> ChipView {
        ChipView::new(self.label(anchor))
    }

    fn quotes(&self) -> bool {
        true
    }

    fn quote(&self, _doc: DocMeta<'_>, anchor: &BibleAnchor) -> Option<Quote> {
        self.passage_quote(anchor)
    }

    fn selection(&self) -> SelectionSupport {
        SelectionSupport::TEXT
    }

    fn actions(&self) -> Vec<KindAction> {
        vec![KindAction {
            id: "change-translation".into(),
            label: "Cambiar traducción".into(),
            icon: lucide::languages(),
        }]
    }

    fn run_action(&self, _id: &str, _doc: DocMeta<'_>, _content: &BibleContent) -> ActionEffect {
        ActionEffect::Edit
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        let installed = self.db.list_bibles().unwrap_or_default();
        let entry =
            CreationEntry::new(ENTRY_BIBLE, "Biblia", lucide::book_open(), KindColor::Bible)
                .subtitle("Leer y resaltar")
                .starter(3);
        let entry = if installed.len() > 1 {
            entry.flow(CreationFlow::Choose {
                title: "Abrir Biblia".into(),
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
                    label: "Agregar traducción".into(),
                    subtitle: "".into(),
                }),
            })
        } else {
            entry.flow(CreationFlow::Direct)
        };
        vec![entry]
    }

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
        Ok(Created {
            name: "Biblia".to_owned(),
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
            label: "Traducción".into(),
            options: self
                .db
                .list_bibles()
                .unwrap_or_default()
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

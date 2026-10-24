//! The Biblia kind.

use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

use freya_icons::lucide;
use notein_sdk::create::{
    Created, CreationChoice, CreationEntry, CreationError, CreationFlow, CreationRequest,
};
use notein_sdk::doc::DocMeta;
use notein_sdk::kind::{
    ActionEffect, ChipView, DocumentKind, Hit, KindAction, KindColor, KindInfo, Mark, OutlineIcon,
    OutlineNode, SearchQuery, SelectionSupport,
};

use crate::anchor::BibleAnchor;
use crate::db::{BookRow, Db};
use crate::plugin_config::{self, SharedConfig};
use crate::state::{BibleSnapshot, u32_to_rgba};

/// Kind id stored with the Bible document.
pub const KIND: &str = "io.notein.bible";
/// Create entry that opens (or creates) the Bible.
pub const ENTRY_BIBLE: &str = "bible";

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
}

impl BibleKind {
    #[must_use]
    pub fn new(db: Db, config: SharedConfig) -> Self {
        Self {
            db,
            config,
            books: RwLock::new(HashMap::new()),
        }
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
        }
    }

    fn search<'a>(&'a self, query: &'a SearchQuery) -> Self::Hits<'a> {
        let (open, close) = crate::db::snippet_markers();
        let bible = self.default_bible();
        self.db
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
            })
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
        let abbrev = self
            .db
            .list_bibles()
            .ok()?
            .into_iter()
            .find(|b| b.id == content.bible_id)
            .map(|b| b.abbrev)?;
        Some(match content.highlights.len() {
            0 => abbrev,
            1 => format!("{abbrev} · 1 resaltado"),
            n => format!("{abbrev} · {n} resaltados"),
        })
    }

    fn chip(&self, _doc: DocMeta<'_>, anchor: &BibleAnchor) -> ChipView {
        ChipView::new(self.label(anchor))
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
}

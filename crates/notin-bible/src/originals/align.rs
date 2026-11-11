//! Pairing the original words with an installed translation, so the
//! reader can say how this Bible renders each one here and elsewhere.
//!
//! Runs once per translation and pack build, in the background, and
//! keeps its result in the database (`orig_links`, `orig_renderings`).

use std::collections::HashMap;

use notin_align::{AlignConfig, SourceVerse, TargetVerse, VerseKey};
use originals_format::Testament;

use super::Store;
use crate::db::{Db, DbError, LinkRow, PlacedLink, RenderingRow};

/// `meta` key prefix: `<prefix><bible_id>` holds the pack build the
/// alignment was made against.
const META_PREFIX: &str = "originals_align:";

/// 0 for the Old Testament, 1 for the New, as the tables store it.
#[must_use]
pub const fn testament_code(testament: Testament) -> u8 {
    match testament {
        Testament::Old => 0,
        Testament::New => 1,
    }
}

/// Whether `bible_id` is aligned against the packs `store` holds.
#[must_use]
pub fn is_done(db: &Db, store: &Store, bible_id: &str) -> bool {
    db.meta(&format!("{META_PREFIX}{bible_id}"))
        .ok()
        .flatten()
        .is_some_and(|built| built == store.built_unix().to_string())
}

/// Align `bible_id` with every testament `store` has and keep the
/// result. Blocking: seconds on a desktop, longer on a phone.
#[cfg_attr(feature = "sismo", sismo::function("bible::originals_align"))]
pub fn run(db: &Db, store: &Store, bible_id: &str) -> Result<(), DbError> {
    let verses = db.verses_of_bible(bible_id)?;
    for testament in [Testament::Old, Testament::New] {
        if !store.has(testament) {
            continue;
        }
        let source: Vec<SourceVerse> = store
            .lemma_verses(testament)
            .into_iter()
            .map(|(book, chapter, verse, lemmas)| SourceVerse {
                key: VerseKey {
                    book,
                    chapter,
                    verse,
                },
                lemmas,
            })
            .collect();
        let mut book_index: HashMap<&str, u8> = HashMap::new();
        let target: Vec<TargetVerse> = verses
            .iter()
            .filter_map(|row| {
                if super::testament_of(&row.book_id) != Some(testament) {
                    return None;
                }
                let book = if let Some(&b) = book_index.get(row.book_id.as_str()) {
                    b
                } else {
                    let b = store.book_index(&row.book_id)?;
                    book_index.insert(row.book_id.as_str(), b);
                    b
                };
                Some(TargetVerse {
                    key: VerseKey {
                        book,
                        chapter: row.chapter,
                        verse: row.verse,
                    },
                    tokens: notin_align::tokenize(&row.text),
                })
            })
            .collect();
        if target.is_empty() {
            continue;
        }
        let alignment = notin_align::align(&source, &target, &AlignConfig::default());
        let links: Vec<PlacedLink> = alignment
            .verses
            .iter()
            .flat_map(|verse| {
                verse.links.iter().map(move |link| PlacedLink {
                    book: verse.key.book,
                    chapter: verse.key.chapter,
                    verse: verse.key.verse,
                    link: LinkRow {
                        position: link.source_position,
                        start: link.target_start,
                        end: link.target_end,
                        prob: link.prob,
                    },
                })
            })
            .collect();
        let renderings: Vec<(u32, RenderingRow)> = alignment
            .renderings
            .into_iter()
            .flat_map(|(lemma, list)| {
                list.into_iter().map(move |r| {
                    (
                        lemma,
                        RenderingRow {
                            token: r.token,
                            count: r.count,
                            prob: r.prob,
                        },
                    )
                })
            })
            .collect();
        log::info!(
            "originals: {bible_id} {testament:?}: {} links, {} renderings",
            links.len(),
            renderings.len()
        );
        db.replace_alignment(bible_id, testament_code(testament), &links, &renderings)?;
    }
    db.set_meta(
        &format!("{META_PREFIX}{bible_id}"),
        &store.built_unix().to_string(),
    )
}

/// The translation tokens a link covers, in the verse's own spelling.
#[must_use]
pub fn span_text(verse_text: &str, link: &LinkRow) -> String {
    let tokens = notin_align::tokenize_surface(verse_text);
    tokens
        .get(usize::from(link.start)..usize::from(link.end))
        .map(|t| t.join(" "))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_text_uses_the_verse_spelling() {
        let link = LinkRow {
            position: 3,
            start: 4,
            end: 6,
            prob: 0.9,
        };
        assert_eq!(
            span_text(
                "16 Porque el mismo Espíritu da testimonio a nuestro espíritu",
                &link
            ),
            "da testimonio"
        );
        assert_eq!(span_text("corto", &link), "");
    }

    #[test]
    fn aligns_a_sample_pack_with_a_translation() {
        let dir =
            std::env::temp_dir().join(format!("notein-originals-align-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        super::super::store::tests::write_sample(&dir);
        let store = Store::open(&dir).unwrap();
        let db = Db::in_memory().unwrap();
        crate::db::test_support::install_sample_bible(
            &db,
            "spa_test",
            &[("mat", 5, 3, "Dichosos los pobres")],
        );
        assert!(!is_done(&db, &store, "spa_test"));
        run(&db, &store, "spa_test").unwrap();
        assert!(is_done(&db, &store, "spa_test"));
        let links = db.alignment_links("spa_test", 1, 0, 5, 3).unwrap();
        assert!(!links.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Against the real packs and a real translation, for timing and a
/// look at the output; needs the files, so it is ignored by default:
///
/// ```sh
/// NOTEIN_ORIGINALS_TEST_DIR=…/packs NOTEIN_RV1960_BOOKS=…/bibles/spa_rv1960/books \
///   cargo test -p notin-bible --features importer real_packs -- --ignored --nocapture
/// ```
#[cfg(all(test, feature = "importer"))]
mod real_packs {
    use super::*;

    #[derive(serde::Deserialize)]
    struct BookFile {
        book: String,
        contents: Vec<Vec<Vec<String>>>,
    }

    #[test]
    #[ignore = "needs the built packs and a translation on disk"]
    #[allow(clippy::too_many_lines)] // a printed walk through two verses
    fn real_packs_align_with_rv1960() {
        let (Ok(packs), Ok(books)) = (
            std::env::var("NOTEIN_ORIGINALS_TEST_DIR"),
            std::env::var("NOTEIN_RV1960_BOOKS"),
        ) else {
            return;
        };
        let started = std::time::Instant::now();
        let store = Store::open(std::path::Path::new(&packs)).expect("open packs");
        println!("packs opened in {:?}", started.elapsed());
        let mut verses: Vec<(String, u16, u16, String)> = Vec::new();
        for entry in std::fs::read_dir(&books).expect("books dir") {
            let path = entry.expect("entry").path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let file: BookFile =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            for (chapter, chapter_verses) in file.contents.iter().enumerate() {
                for (verse, parts) in chapter_verses.iter().enumerate() {
                    if parts.is_empty() {
                        continue;
                    }
                    verses.push((
                        file.book.clone(),
                        u16::try_from(chapter).unwrap(),
                        u16::try_from(verse).unwrap(),
                        parts.join(" "),
                    ));
                }
            }
        }
        println!("{} verses read", verses.len());
        let db = Db::in_memory().unwrap();
        let rows: Vec<(&str, u16, u16, &str)> = verses
            .iter()
            .map(|(b, c, v, t)| (b.as_str(), *c, *v, t.as_str()))
            .collect();
        crate::db::test_support::install_sample_bible(&db, "spa_rv1960", &rows);
        let started = std::time::Instant::now();
        run(&db, &store, "spa_rv1960").expect("align");
        println!("aligned in {:?}", started.elapsed());
        let words = store.words("mat", 5, 3, "es").expect("mat 5:3");
        let links = db
            .alignment_links("spa_rv1960", 1, store.book_index("mat").unwrap(), 5, 3)
            .unwrap();
        let text = verses
            .iter()
            .find(|(b, c, v, _)| b == "mat" && *c == 5 && *v == 3)
            .map(|(_, _, _, t)| t.clone())
            .unwrap();
        println!("Mat 5:3: {text}");
        for word in &words {
            let here: Vec<String> = links
                .iter()
                .filter(|l| l.position == word.position)
                .map(|l| format!("{} ({:.2})", span_text(&text, l), l.prob))
                .collect();
            println!(
                "  {} [{}] → {}",
                word.surface,
                word.gloss.as_deref().unwrap_or(""),
                here.join(" | ")
            );
        }
        let makarios = store.lemma(words[0].lemma).unwrap();
        println!(
            "{} renderings: {:?}",
            makarios.lemma,
            db.renderings("spa_rv1960", 1, words[0].lemma.index)
                .unwrap()
        );
        let genesis = store.words("gen", 1, 1, "es").expect("gen 1:1");
        let links = db
            .alignment_links("spa_rv1960", 0, store.book_index("gen").unwrap(), 1, 1)
            .unwrap();
        let text = verses
            .iter()
            .find(|(b, c, v, _)| b == "gen" && *c == 1 && *v == 1)
            .map(|(_, _, _, t)| t.clone())
            .unwrap();
        println!("Gen 1:1: {text}");
        for word in &genesis {
            let here: Vec<String> = links
                .iter()
                .filter(|l| l.position == word.position)
                .map(|l| format!("{} ({:.2})", span_text(&text, l), l.prob))
                .collect();
            let lemma = store.lemma(word.lemma).unwrap();
            println!(
                "  {} [{}] {} → {} · {:?}",
                word.surface,
                word.gloss_en,
                crate::originals::respell::hebrew(
                    notin_i18n::Lang::Es,
                    &lemma.translit,
                    &lemma.pronunciation_strong
                ),
                here.join(" | "),
                db.renderings("spa_rv1960", 0, word.lemma.index)
                    .unwrap()
                    .iter()
                    .map(|r| format!("{} {}", r.token, r.count))
                    .collect::<Vec<_>>()
            );
        }
    }
}

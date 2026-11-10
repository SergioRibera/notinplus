//! The packs on disk, read into memory and validated once.

use std::collections::HashMap;
use std::path::Path;

use originals_format::{ArchivedPack, ArchivedSenses, FORMAT_VERSION, Origin, Testament};
use rkyv::rancor;
use rkyv::util::AlignedVec;

type PackRef<'a> = &'a ArchivedPack;
type SensesRef<'a> = &'a ArchivedSenses;

self_cell::self_cell!(
    struct PackCell {
        owner: AlignedVec,
        #[covariant]
        dependent: PackRef,
    }
);

self_cell::self_cell!(
    struct SensesCell {
        owner: AlignedVec,
        #[covariant]
        dependent: SensesRef,
    }
);

impl std::fmt::Debug for PackCell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackCell")
            .field("bytes", &self.borrow_owner().len())
            .finish()
    }
}

impl std::fmt::Debug for SensesCell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SensesCell")
            .field("bytes", &self.borrow_owner().len())
            .finish()
    }
}

#[derive(Debug)]
pub enum StoreError {
    /// No pack on disk at all.
    Missing,
    Io(std::io::Error),
    /// The file is not a valid archive of this format.
    Invalid {
        file: String,
        reason: String,
    },
    /// Built by another format version; the reader downloads again.
    Version {
        file: String,
        found: u16,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("no original-language pack on disk"),
            Self::Io(err) => write!(f, "originals: {err}"),
            Self::Invalid { file, reason } => write!(f, "originals: {file} is not valid: {reason}"),
            Self::Version { file, found } => write!(
                f,
                "originals: {file} is format {found}, this reader wants {FORMAT_VERSION}"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// The packs and senses files found in a directory.
#[derive(Debug)]
pub struct Store {
    old: Option<PackCell>,
    new: Option<PackCell>,
    senses: HashMap<String, SensesCell>,
}

/// Which lemma, in which testament's pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LemmaRef {
    pub testament: Testament,
    pub index: u32,
}

/// One word of a verse, owned, as the reader shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordView {
    pub position: u16,
    pub surface: String,
    pub translit: String,
    /// Morphology code of the main part (`A-NPM`, `Ncfsa`).
    pub morph: String,
    pub gloss_en: String,
    /// The contextual gloss in `language`, when the pack has one.
    pub gloss: Option<String>,
    pub lemma: LemmaRef,
    pub affixes: Vec<AffixView>,
}

/// A prefix or suffix of a Hebrew word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffixView {
    pub strong: String,
    pub lemma: String,
    pub gloss_en: String,
    pub morph: String,
}

/// A dictionary form, owned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LemmaView {
    pub strong: String,
    pub lemma: String,
    pub translit: String,
    pub pos: String,
    pub gloss_en: String,
    pub senses_en: Vec<String>,
    pub definition_en: String,
    pub derivation_en: String,
    pub pronunciation_strong: String,
    /// How many times it occurs as a main lemma.
    pub count: usize,
}

/// Where a lemma occurs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub book_id: String,
    pub chapter: u16,
    pub verse: u16,
    pub position: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenseView {
    pub text: String,
    pub origin: Origin,
}

impl Store {
    /// Read every pack and senses file in `dir`. A missing testament is
    /// fine (the reader shows the other); no pack at all is
    /// [`StoreError::Missing`]. A corrupt or outdated file is an error so
    /// the caller downloads again.
    ///
    /// # Errors
    /// [`StoreError::Missing`] without any pack; otherwise a file that
    /// cannot be read, is not a valid archive or has another format
    /// version.
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        let old = read_pack(dir, Testament::Old)?;
        let new = read_pack(dir, Testament::New)?;
        if old.is_none() && new.is_none() {
            return Err(StoreError::Missing);
        }
        let mut senses = HashMap::new();
        for lang in notein_i18n::Lang::ALL {
            if let Some(cell) = read_senses(dir, lang.tag())? {
                senses.insert(lang.tag().to_owned(), cell);
            }
        }
        Ok(Self { old, new, senses })
    }

    fn pack(&self, testament: Testament) -> Option<&ArchivedPack> {
        match testament {
            Testament::Old => self.old.as_ref(),
            Testament::New => self.new.as_ref(),
        }
        .map(PackCell::borrow_dependent)
        .copied()
    }

    /// Whether this testament's pack is on hand.
    #[must_use]
    pub fn has(&self, testament: Testament) -> bool {
        self.pack(testament).is_some()
    }

    /// Languages with a senses file.
    #[must_use]
    pub fn sense_languages(&self) -> Vec<String> {
        let mut langs: Vec<String> = self.senses.keys().cloned().collect();
        langs.sort();
        langs
    }

    /// Build time of the packs (the newer of the two).
    #[must_use]
    pub fn built_unix(&self) -> u64 {
        [Testament::Old, Testament::New]
            .into_iter()
            .filter_map(|t| self.pack(t).map(|p| p.header.built_unix.to_native()))
            .max()
            .unwrap_or(0)
    }

    /// The words of `chapter:verse` in the book `book_id`, in text
    /// order; `None` when the pack lacks the verse.
    #[must_use]
    pub fn words(
        &self,
        book_id: &str,
        chapter: u16,
        verse: u16,
        language: &str,
    ) -> Option<Vec<WordView>> {
        let testament = super::testament_of(book_id)?;
        let pack = self.pack(testament)?;
        let book = pack.book_index(&book_id.to_ascii_lowercase())?;
        let archived = pack.verse(book, chapter, verse)?;
        let gloss_column = pack
            .header
            .gloss_languages
            .iter()
            .position(|l| l.as_str() == language);
        let words = archived
            .words
            .iter()
            .enumerate()
            .map(|(position, word)| {
                let gloss = gloss_column
                    .and_then(|c| word.glosses.get(c))
                    .and_then(|g| g.as_ref())
                    .map(|g| pack.str(g.to_native()).to_owned())
                    .filter(|g| !g.is_empty());
                let affixes = word
                    .affixes
                    .iter()
                    .map(|a| {
                        let lemma = pack.lemma(a.lemma.to_native());
                        AffixView {
                            strong: lemma
                                .map(|l| l.strong.as_str().to_owned())
                                .unwrap_or_default(),
                            lemma: lemma
                                .map(|l| l.lemma.as_str().to_owned())
                                .unwrap_or_default(),
                            gloss_en: lemma
                                .map(|l| l.gloss_en.as_str().to_owned())
                                .unwrap_or_default(),
                            morph: pack.str(a.morph.to_native()).to_owned(),
                        }
                    })
                    .collect();
                WordView {
                    position: u16::try_from(position).unwrap_or(u16::MAX),
                    surface: pack.str(word.surface.to_native()).to_owned(),
                    translit: pack.str(word.translit.to_native()).to_owned(),
                    morph: pack.str(word.morph.to_native()).to_owned(),
                    gloss_en: word
                        .glosses
                        .first()
                        .and_then(|g| g.as_ref())
                        .map(|g| pack.str(g.to_native()).to_owned())
                        .unwrap_or_default(),
                    gloss,
                    lemma: LemmaRef {
                        testament,
                        index: word.lemma.to_native(),
                    },
                    affixes,
                }
            })
            .collect();
        Some(words)
    }

    /// The lemma ids of every verse of `testament`, for the aligner:
    /// `(book index, chapter, verse, lemmas)`.
    #[must_use]
    pub fn lemma_verses(&self, testament: Testament) -> Vec<(u8, u16, u16, Vec<u32>)> {
        let Some(pack) = self.pack(testament) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (book, archived_book) in pack.books.iter().enumerate() {
            let book = u8::try_from(book).unwrap_or(u8::MAX);
            for chapter in archived_book.chapters.iter() {
                for verse in chapter.verses.iter() {
                    let lemmas = verse.words.iter().map(|w| w.lemma.to_native()).collect();
                    out.push((
                        book,
                        chapter.number.to_native(),
                        verse.number.to_native(),
                        lemmas,
                    ));
                }
            }
        }
        out
    }

    /// Index of `book_id` in its testament's pack.
    #[must_use]
    pub fn book_index(&self, book_id: &str) -> Option<u8> {
        let testament = super::testament_of(book_id)?;
        let pack = self.pack(testament)?;
        pack.book_index(&book_id.to_ascii_lowercase())
            .and_then(|i| u8::try_from(i).ok())
    }

    /// The dictionary form `strong` names (`H433`, `H0433`, `G3107`,
    /// `H2617A`), in the pack of its testament.
    #[must_use]
    pub fn lemma_by_strong(&self, strong: &str) -> Option<LemmaView> {
        let (testament, key) = strong_key(strong)?;
        let index = self.pack(testament)?.lemma_index(&key)?;
        self.lemma(LemmaRef { testament, index })
    }

    #[must_use]
    pub fn lemma(&self, at: LemmaRef) -> Option<LemmaView> {
        let pack = self.pack(at.testament)?;
        let lemma = pack.lemma(at.index)?;
        Some(LemmaView {
            strong: lemma.strong.as_str().to_owned(),
            lemma: lemma.lemma.as_str().to_owned(),
            translit: lemma.translit.as_str().to_owned(),
            pos: lemma.pos.as_str().to_owned(),
            gloss_en: lemma.gloss_en.as_str().to_owned(),
            senses_en: lemma
                .senses_en
                .iter()
                .map(|s| s.as_str().to_owned())
                .collect(),
            definition_en: lemma.definition_en.as_str().to_owned(),
            derivation_en: lemma.derivation_en.as_str().to_owned(),
            pronunciation_strong: lemma.pronunciation_strong.as_str().to_owned(),
            count: lemma.occurrences.len(),
        })
    }

    /// Every occurrence of the lemma, in canon order.
    #[must_use]
    pub fn occurrences(&self, at: LemmaRef) -> Vec<Occurrence> {
        let Some(pack) = self.pack(at.testament) else {
            return Vec::new();
        };
        let Some(lemma) = pack.lemma(at.index) else {
            return Vec::new();
        };
        lemma
            .occurrences
            .iter()
            .filter_map(|o| {
                let book = pack.books.get(usize::from(o.book))?;
                Some(Occurrence {
                    book_id: book.id.as_str().to_owned(),
                    chapter: o.chapter.to_native(),
                    verse: o.verse.to_native(),
                    position: o.position.to_native(),
                })
            })
            .collect()
    }

    /// The senses of the lemma keyed `strong` in `language`; empty when
    /// that language has no file or no entry.
    #[must_use]
    pub fn senses(&self, strong: &str, language: &str) -> Vec<SenseView> {
        let Some(cell) = self.senses.get(language) else {
            return Vec::new();
        };
        let senses: &ArchivedSenses = cell.borrow_dependent();
        senses.entry(strong).map_or_else(Vec::new, |entry| {
            entry
                .senses
                .iter()
                .map(|s| SenseView {
                    text: s.text.as_str().to_owned(),
                    origin: match s.origin {
                        originals_format::ArchivedOrigin::Human => Origin::Human,
                        originals_format::ArchivedOrigin::Wiktionary => Origin::Wiktionary,
                        originals_format::ArchivedOrigin::Auto => Origin::Auto,
                    },
                })
                .collect()
        })
    }
}

fn read_bytes(path: &Path) -> Result<Option<AlignedVec>, StoreError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(StoreError::Io(err)),
    };
    let mut aligned = AlignedVec::<16>::with_capacity(bytes.len());
    aligned.extend_from_slice(&bytes);
    Ok(Some(aligned))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read_pack(dir: &Path, testament: Testament) -> Result<Option<PackCell>, StoreError> {
    let path = dir.join(super::pack_file(testament));
    let Some(bytes) = read_bytes(&path)? else {
        return Ok(None);
    };
    let cell = PackCell::try_new(bytes, |b| rkyv::access::<ArchivedPack, rancor::Error>(b))
        .map_err(|err: rancor::Error| StoreError::Invalid {
            file: file_name(&path),
            reason: err.to_string(),
        })?;
    let found = cell.borrow_dependent().header.format_version.to_native();
    if found != FORMAT_VERSION {
        return Err(StoreError::Version {
            file: file_name(&path),
            found,
        });
    }
    Ok(Some(cell))
}

fn read_senses(dir: &Path, language: &str) -> Result<Option<SensesCell>, StoreError> {
    let path = dir.join(super::senses_file(language));
    let Some(bytes) = read_bytes(&path)? else {
        return Ok(None);
    };
    let cell = SensesCell::try_new(bytes, |b| rkyv::access::<ArchivedSenses, rancor::Error>(b))
        .map_err(|err: rancor::Error| StoreError::Invalid {
            file: file_name(&path),
            reason: err.to_string(),
        })?;
    let found = cell.borrow_dependent().header.format_version.to_native();
    if found != FORMAT_VERSION {
        return Err(StoreError::Version {
            file: file_name(&path),
            found,
        });
    }
    Ok(Some(cell))
}

/// The testament and the key the packs use for `strong`: the number
/// zero-padded to four digits, the disambiguating letter kept.
fn strong_key(strong: &str) -> Option<(Testament, String)> {
    let mut chars = strong.trim().chars();
    let testament = match chars.next()? {
        'H' => Testament::Old,
        'G' => Testament::New,
        _ => return None,
    };
    let rest = chars.as_str();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let suffix = &rest[digits.len()..];
    if digits.is_empty() || !suffix.chars().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    let prefix = if testament == Testament::Old {
        'H'
    } else {
        'G'
    };
    Some((testament, format!("{prefix}{digits:0>4}{suffix}")))
}

#[cfg(test)]
mod strong_key_tests {
    use super::{Testament, strong_key};

    #[test]
    fn strong_numbers_become_pack_keys() {
        assert_eq!(strong_key("H433"), Some((Testament::Old, "H0433".into())));
        assert_eq!(strong_key("H0433"), Some((Testament::Old, "H0433".into())));
        assert_eq!(
            strong_key("H2617A"),
            Some((Testament::Old, "H2617A".into()))
        );
        assert_eq!(strong_key("G3107"), Some((Testament::New, "G3107".into())));
        assert_eq!(strong_key("H"), None);
        assert_eq!(strong_key("Hx"), None);
        assert_eq!(strong_key("S123"), None);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use originals_format::{
        Affix, Book, Chapter, Header, Lemma, Pack, Sense, SenseEntry, Senses, SensesHeader, Verse,
        Word, WordRef,
    };

    use super::*;

    /// A New Testament pack with Mat 5:3 (two words) and a senses file
    /// in Spanish, written to `dir`.
    #[allow(clippy::too_many_lines)] // one literal pack
    pub(crate) fn write_sample(dir: &Path) {
        let pack = Pack {
            header: Header {
                format_version: FORMAT_VERSION,
                testament: Testament::New,
                gloss_languages: vec!["en".into(), "es".into()],
                sources: "test".into(),
                built_unix: 7,
            },
            strings: vec![
                "Μακάριοι".into(),
                "Makarioi".into(),
                "A-NPM".into(),
                "Blessed [are]".into(),
                "Dichosos".into(),
                "οἱ".into(),
                "hoi".into(),
                "T-NPM".into(),
                "the".into(),
                "los".into(),
                "R".into(),
            ],
            books: vec![Book {
                id: "mat".into(),
                chapters: vec![Chapter {
                    number: 5,
                    verses: vec![Verse {
                        number: 3,
                        words: vec![
                            Word {
                                surface: 0,
                                translit: 1,
                                morph: 2,
                                glosses: vec![Some(3), Some(4)],
                                lemma: 0,
                                affixes: vec![],
                            },
                            Word {
                                surface: 5,
                                translit: 6,
                                morph: 7,
                                glosses: vec![Some(8), Some(9)],
                                lemma: 1,
                                affixes: vec![Affix {
                                    lemma: 2,
                                    morph: 10,
                                }],
                            },
                        ],
                    }],
                }],
            }],
            lemmas: vec![
                Lemma {
                    strong: "G3107".into(),
                    lemma: "μακάριος".into(),
                    translit: "makarios".into(),
                    pos: "A".into(),
                    gloss_en: "blessed".into(),
                    senses_en: vec!["blessed".into(), "happy".into()],
                    definition_en: "supremely blest".into(),
                    derivation_en: String::new(),
                    pronunciation_strong: "mak-ar'-ee-os".into(),
                    occurrences: vec![WordRef {
                        book: 0,
                        chapter: 5,
                        verse: 3,
                        position: 0,
                    }],
                },
                Lemma {
                    strong: "G3588".into(),
                    lemma: "ὁ".into(),
                    translit: "ho".into(),
                    pos: "T".into(),
                    gloss_en: "the".into(),
                    senses_en: vec!["the".into()],
                    definition_en: String::new(),
                    derivation_en: String::new(),
                    pronunciation_strong: "ho".into(),
                    occurrences: vec![WordRef {
                        book: 0,
                        chapter: 5,
                        verse: 3,
                        position: 1,
                    }],
                },
                Lemma {
                    strong: "H9003".into(),
                    lemma: "ב".into(),
                    translit: "b".into(),
                    pos: "Prefix".into(),
                    gloss_en: "in".into(),
                    senses_en: vec![],
                    definition_en: String::new(),
                    derivation_en: String::new(),
                    pronunciation_strong: String::new(),
                    occurrences: vec![],
                },
            ],
        };
        let senses = Senses {
            header: SensesHeader {
                format_version: FORMAT_VERSION,
                language: "es".into(),
                built_unix: 7,
            },
            entries: vec![SenseEntry {
                strong: "G3107".into(),
                senses: vec![Sense {
                    text: "bienaventurado".into(),
                    origin: Origin::Auto,
                }],
            }],
        };
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(super::super::pack_file(Testament::New)),
            rkyv::to_bytes::<rancor::Error>(&pack).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join(super::super::senses_file("es")),
            rkyv::to_bytes::<rancor::Error>(&senses).unwrap(),
        )
        .unwrap();
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("notein-originals-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn reads_words_lemmas_and_senses() {
        let dir = temp_dir("store");
        write_sample(&dir);
        let store = Store::open(&dir).expect("open");
        assert!(store.has(Testament::New));
        assert!(!store.has(Testament::Old));
        assert_eq!(store.sense_languages(), vec!["es"]);
        let words = store.words("MAT", 5, 3, "es").expect("verse");
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].surface, "Μακάριοι");
        assert_eq!(words[0].gloss.as_deref(), Some("Dichosos"));
        assert_eq!(words[1].affixes[0].gloss_en, "in");
        let lemma = store.lemma(words[0].lemma).expect("lemma");
        assert_eq!(lemma.strong, "G3107");
        assert_eq!(lemma.count, 1);
        assert_eq!(store.occurrences(words[0].lemma)[0].book_id, "mat");
        assert_eq!(store.senses("G3107", "es")[0].text, "bienaventurado");
        assert!(store.senses("G3107", "en").is_empty());
        assert!(store.words("gen", 1, 1, "es").is_none());
        assert_eq!(
            store.lemma_verses(Testament::New),
            vec![(0, 5, 3, vec![0, 1])]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_and_corrupt_packs_are_told_apart() {
        let dir = temp_dir("empty");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(Store::open(&dir), Err(StoreError::Missing)));
        std::fs::write(
            dir.join(super::super::pack_file(Testament::Old)),
            b"not a pack",
        )
        .unwrap();
        assert!(matches!(Store::open(&dir), Err(StoreError::Invalid { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

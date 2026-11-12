//! The packs on disk, read into memory once and borrowed from there:
//! every table is a slice of the file's bytes, checked when opened.

use std::collections::HashMap;
use std::path::Path;

use originals_format::{self as format, FORMAT_VERSION, FormatError, Origin, Testament};

self_cell::self_cell!(
    struct PackCell {
        owner: Vec<u8>,
        #[covariant]
        dependent: PackView,
    }
);

self_cell::self_cell!(
    struct SensesCell {
        owner: Vec<u8>,
        #[covariant]
        dependent: SensesView,
    }
);

type PackView<'a> = format::PackView<'a>;
type SensesView<'a> = format::SensesView<'a>;

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
    /// cannot be read, is not a valid pack or has another format
    /// version.
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        let old = read_pack(dir, Testament::Old)?;
        let new = read_pack(dir, Testament::New)?;
        if old.is_none() && new.is_none() {
            return Err(StoreError::Missing);
        }
        let mut senses = HashMap::new();
        for lang in notin_i18n::Lang::ALL {
            if let Some(cell) = read_senses(dir, lang.tag())? {
                senses.insert(lang.tag().to_owned(), cell);
            }
        }
        Ok(Self { old, new, senses })
    }

    fn pack(&self, testament: Testament) -> Option<&PackView<'_>> {
        match testament {
            Testament::Old => self.old.as_ref(),
            Testament::New => self.new.as_ref(),
        }
        .map(PackCell::borrow_dependent)
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
            .filter_map(|t| self.pack(t).map(PackView::built_unix))
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
        let found = pack.verse(book, chapter, verse)?;
        let gloss_column = pack.gloss_languages().iter().position(|l| *l == language);
        let words = found
            .words()
            .enumerate()
            .map(|(position, word)| {
                let affixes = word
                    .affixes()
                    .map(|a| {
                        let lemma = pack.lemma(a.lemma());
                        AffixView {
                            strong: lemma.map(|l| l.strong().to_owned()).unwrap_or_default(),
                            lemma: lemma.map(|l| l.lemma().to_owned()).unwrap_or_default(),
                            gloss_en: lemma.map(|l| l.gloss_en().to_owned()).unwrap_or_default(),
                            morph: a.morph().to_owned(),
                        }
                    })
                    .collect();
                WordView {
                    position: u16::try_from(position).unwrap_or(u16::MAX),
                    surface: word.surface().to_owned(),
                    translit: word.translit().to_owned(),
                    morph: word.morph().to_owned(),
                    gloss_en: word.gloss(0).unwrap_or_default().to_owned(),
                    gloss: gloss_column
                        .and_then(|c| word.gloss(c))
                        .filter(|g| !g.is_empty())
                        .map(str::to_owned),
                    lemma: LemmaRef {
                        testament,
                        index: word.lemma(),
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
        for (index, book) in pack.books().enumerate() {
            let index = u8::try_from(index).unwrap_or(u8::MAX);
            for chapter in book.chapters() {
                for verse in chapter.verses() {
                    let lemmas = verse.words().map(|w| w.lemma()).collect();
                    out.push((index, chapter.number(), verse.number(), lemmas));
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
        let lemma = self.pack(at.testament)?.lemma(at.index)?;
        Some(LemmaView {
            strong: lemma.strong().to_owned(),
            lemma: lemma.lemma().to_owned(),
            translit: lemma.translit().to_owned(),
            pos: lemma.pos().to_owned(),
            gloss_en: lemma.gloss_en().to_owned(),
            senses_en: lemma.senses_en().map(str::to_owned).collect(),
            definition_en: lemma.definition_en().to_owned(),
            derivation_en: lemma.derivation_en().to_owned(),
            pronunciation_strong: lemma.pronunciation_strong().to_owned(),
            count: lemma.occurrences().len(),
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
            .occurrences()
            .filter_map(|o| {
                let book = pack.book(usize::from(o.book))?;
                Some(Occurrence {
                    book_id: book.id().to_owned(),
                    chapter: o.chapter,
                    verse: o.verse,
                    position: o.position,
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
        cell.borrow_dependent()
            .entry(strong)
            .map_or_else(Vec::new, |entry| {
                entry
                    .senses()
                    .map(|(text, origin)| SenseView {
                        text: text.to_owned(),
                        origin,
                    })
                    .collect()
            })
    }
}

/// The file at `path`, or `None` when there is none. Read straight into
/// the buffer the views borrow from: one copy of the file in memory.
fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, StoreError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(StoreError::Io(err)),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What a failed parse of `path` means for the reader.
fn store_error(path: &Path, err: FormatError) -> StoreError {
    match err {
        FormatError::Version { found } => StoreError::Version {
            file: file_name(path),
            found,
        },
        other => StoreError::Invalid {
            file: file_name(path),
            reason: other.to_string(),
        },
    }
}

fn read_pack(dir: &Path, testament: Testament) -> Result<Option<PackCell>, StoreError> {
    let path = dir.join(super::pack_file(testament));
    let Some(bytes) = read_bytes(&path)? else {
        return Ok(None);
    };
    PackCell::try_new(bytes, |b| PackView::parse(b))
        .map(Some)
        .map_err(|err| store_error(&path, err))
}

fn read_senses(dir: &Path, language: &str) -> Result<Option<SensesCell>, StoreError> {
    let path = dir.join(super::senses_file(language));
    let Some(bytes) = read_bytes(&path)? else {
        return Ok(None);
    };
    SensesCell::try_new(bytes, |b| SensesView::parse(b))
        .map(Some)
        .map_err(|err| store_error(&path, err))
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
            pack.encode().unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join(super::super::senses_file("es")),
            senses.encode().unwrap(),
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
        let pack = dir.join(super::super::pack_file(Testament::Old));
        // Our version, then nothing a pack is made of.
        let mut damaged = FORMAT_VERSION.to_le_bytes().to_vec();
        damaged.extend_from_slice(b"not a pack");
        std::fs::write(&pack, damaged).unwrap();
        assert!(matches!(Store::open(&dir), Err(StoreError::Invalid { .. })));
        // A pack of another layout is told by its version.
        let mut older = (FORMAT_VERSION - 1).to_le_bytes().to_vec();
        older.extend_from_slice(b"whatever it held");
        std::fs::write(&pack, older).unwrap();
        assert!(matches!(
            Store::open(&dir),
            Err(StoreError::Version { found, .. }) if found == FORMAT_VERSION - 1
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

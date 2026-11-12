//! Bible original-language packs off the network: parsing any bytes, or
//! a valid pack damaged anywhere, must fail cleanly or yield views that
//! never panic and never read out of range.
#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use originals_format::{
    Affix, Book, Chapter, FORMAT_VERSION, Header, Lemma, Pack, PackView, Sense, SenseEntry,
    Senses, SensesHeader, SensesView, Testament, Verse, Word, WordRef,
};

fn lemma(strong: &str, senses: &[&str], verse: u16) -> Lemma {
    Lemma {
        strong: strong.into(),
        lemma: "μακάριος".into(),
        translit: "makarios".into(),
        pos: "A".into(),
        gloss_en: "blessed".into(),
        senses_en: senses.iter().map(|s| (*s).into()).collect(),
        definition_en: "supremely blest".into(),
        derivation_en: String::new(),
        pronunciation_strong: "mak-ar'-ee-os".into(),
        occurrences: vec![WordRef {
            book: 0,
            chapter: 5,
            verse,
            position: 0,
        }],
    }
}

fn word(lemma: u32, gloss: Option<u32>, affixes: Vec<Affix>) -> Word {
    Word {
        surface: 0,
        translit: 1,
        morph: 2,
        glosses: vec![Some(3), gloss],
        lemma,
        affixes,
    }
}

/// A small valid pack and senses file, encoded once.
fn samples() -> &'static (Vec<u8>, Vec<u8>) {
    static SAMPLES: OnceLock<(Vec<u8>, Vec<u8>)> = OnceLock::new();
    SAMPLES.get_or_init(|| {
        let pack = Pack {
            header: Header {
                format_version: FORMAT_VERSION,
                testament: Testament::New,
                gloss_languages: vec!["en".into(), "es".into()],
                sources: "fuzz".into(),
                built_unix: 1,
            },
            strings: ["Μακάριοι", "Makarioi", "A-NPM", "Blessed", "Dichosos"]
                .map(String::from)
                .to_vec(),
            books: vec![Book {
                id: "mat".into(),
                chapters: vec![Chapter {
                    number: 5,
                    verses: vec![
                        Verse {
                            number: 3,
                            words: vec![word(0, Some(4), vec![Affix { lemma: 1, morph: 2 }])],
                        },
                        Verse {
                            number: 4,
                            words: vec![word(1, None, vec![]), word(0, Some(4), vec![])],
                        },
                    ],
                }],
            }],
            lemmas: vec![lemma("G3107", &["blessed", "happy"], 3), lemma("G3588", &[], 4)],
        };
        let senses = Senses {
            header: SensesHeader {
                format_version: FORMAT_VERSION,
                language: "es".into(),
                built_unix: 1,
            },
            entries: vec![SenseEntry {
                strong: "G3107".into(),
                senses: vec![Sense {
                    text: "bienaventurado".into(),
                    origin: originals_format::Origin::Auto,
                }],
            }],
        };
        (
            pack.encode().expect("sample pack"),
            senses.encode().expect("sample senses"),
        )
    })
}

/// Every view a reader can reach.
fn walk(pack: &PackView<'_>) -> usize {
    let mut seen = pack.sources().len() + pack.lemma_count();
    for book in pack.books() {
        seen += book.id().len();
        for chapter in book.chapters() {
            for verse in chapter.verses() {
                for word in verse.words() {
                    seen += word.surface().len() + word.translit().len() + word.morph().len();
                    for column in 0..=pack.gloss_languages().len() {
                        seen += word.gloss(column).map_or(0, str::len);
                    }
                    for affix in word.affixes() {
                        seen += affix.morph().len();
                        seen += pack.lemma(affix.lemma()).map_or(0, |l| l.lemma().len());
                    }
                    if let Some(lemma) = pack.lemma(word.lemma()) {
                        seen += lemma.senses_en().map(str::len).sum::<usize>();
                        seen += lemma.occurrences().count() + lemma.strong().len();
                        seen += usize::from(pack.lemma_index(lemma.strong()).is_some());
                    }
                }
            }
        }
    }
    seen
}

fn senses(bytes: &[u8]) {
    if let Ok(senses) = SensesView::parse(bytes) {
        for key in ["G3107", "G0001", ""] {
            if let Some(entry) = senses.entry(key) {
                std::hint::black_box(entry.senses().count());
            }
        }
    }
}

/// `data` laid over `sample`, so most inputs stay close to a valid file.
fn damaged(sample: &[u8], data: &[u8]) -> Vec<u8> {
    let mut bytes = sample.to_vec();
    if bytes.is_empty() {
        return bytes;
    }
    for (i, b) in data.iter().enumerate() {
        let at = i % bytes.len();
        bytes[at] ^= b;
    }
    // Some inputs cut the file short too.
    if let Some(&cut) = data.first()
        && cut & 1 == 1
    {
        bytes.truncate(bytes.len() * usize::from(cut) / 256);
    }
    bytes
}

fuzz_target!(|data: &[u8]| {
    let (pack, senses_file) = samples();
    if let Ok(view) = PackView::parse(data) {
        std::hint::black_box(walk(&view));
    }
    if let Ok(view) = PackView::parse(&damaged(pack, data)) {
        std::hint::black_box(walk(&view));
    }
    senses(data);
    senses(&damaged(senses_file, data));
});

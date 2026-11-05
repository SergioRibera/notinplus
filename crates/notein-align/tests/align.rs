use std::time::Instant;

use notein_align::{AlignConfig, Alignment, SourceVerse, TargetVerse, VerseKey, align, tokenize};

const fn key(verse: u16) -> VerseKey {
    VerseKey {
        book: 1,
        chapter: 1,
        verse,
    }
}

/// Distractor vocabulary large enough that co-occurrence stays sparse.
const FILLER_TYPES: u16 = 40;

fn filler(idx: u16) -> String {
    format!("f{idx}")
}

/// Lemma 1 is always "misericordia", lemma 2 always "dios", lemma 3 always
/// the two-word "da testimonio". Each appears in a different subset of the
/// verses, mixed with distractors that vary, so the statistics can tell
/// them apart the way real text does.
fn corpus(verses: u16, with_two_word: bool) -> (Vec<SourceVerse>, Vec<TargetVerse>) {
    let mut source = Vec::new();
    let mut target = Vec::new();
    for v in 0..verses {
        // Each item is one original word with its rendering; the order is
        // shuffled per verse so no two items are always adjacent.
        let mut items: Vec<(u32, Vec<String>)> = Vec::new();
        for k in 0..3u16 {
            let idx = (v.wrapping_mul(7).wrapping_add(k * 11)) % FILLER_TYPES;
            items.push((10 + u32::from(idx), vec![filler(idx)]));
        }
        if v % 3 != 0 {
            items.push((1, vec!["misericordia".into()]));
        }
        if v % 2 == 0 {
            items.push((2, vec!["dios".into()]));
        }
        if with_two_word && v % 5 != 2 {
            items.push((3, vec!["da".into(), "testimonio".into()]));
        }
        items.sort_by_key(|(lemma, _)| (u32::from(v) * 31 + lemma * 17) % 23);
        let lemmas = items.iter().map(|(lemma, _)| *lemma).collect();
        let tokens = items.into_iter().flat_map(|(_, words)| words).collect();
        source.push(SourceVerse {
            key: key(v),
            lemmas,
        });
        target.push(TargetVerse {
            key: key(v),
            tokens,
        });
    }
    (source, target)
}

fn renderings_of(alignment: &Alignment, lemma: u32) -> Vec<(&str, u32)> {
    alignment
        .renderings
        .iter()
        .find(|(l, _)| *l == lemma)
        .map(|(_, list)| list.iter().map(|r| (r.token.as_str(), r.count)).collect())
        .unwrap_or_default()
}

/// Position of `lemma` in the verse, if present.
fn position(verse: &SourceVerse, lemma: u32) -> Option<u16> {
    verse
        .lemmas
        .iter()
        .position(|&l| l == lemma)
        .and_then(|p| u16::try_from(p).ok())
}

#[test]
fn recovers_consistent_renderings() {
    let (source, target) = corpus(60, false);
    let alignment = align(&source, &target, &AlignConfig::default());
    assert_eq!(alignment.verses.len(), 60);
    assert_eq!(renderings_of(&alignment, 1), [("misericordia", 40)]);
    assert_eq!(renderings_of(&alignment, 2), [("dios", 30)]);
    for (verse, src) in alignment.verses.iter().zip(&source) {
        let Some(pos) = position(src, 1) else {
            continue;
        };
        let link = verse
            .links
            .iter()
            .find(|l| l.source_position == pos)
            .expect("lemma 1 linked");
        assert_eq!(link.target_end, link.target_start + 1);
        assert_eq!(
            target[usize::from(src.key.verse)].tokens[usize::from(link.target_start)],
            "misericordia"
        );
        assert!(link.prob > 0.8, "prob {}", link.prob);
    }
}

#[test]
fn two_word_rendering_becomes_one_span() {
    let (source, target) = corpus(60, true);
    let alignment = align(&source, &target, &AlignConfig::default());
    assert_eq!(renderings_of(&alignment, 3), [("da testimonio", 48)]);
    let src = &source[0];
    let pos = position(src, 3).expect("verse 0 has lemma 3");
    let link = alignment.verses[0]
        .links
        .iter()
        .find(|l| l.source_position == pos)
        .expect("lemma 3 linked");
    let tokens = &target[0].tokens[usize::from(link.target_start)..usize::from(link.target_end)];
    assert_eq!(tokens, ["da", "testimonio"]);
}

#[test]
fn unpaired_and_empty_verses_are_skipped() {
    let (mut source, mut target) = corpus(10, false);
    source.push(SourceVerse {
        key: key(99),
        lemmas: vec![1, 2],
    });
    target.push(TargetVerse {
        key: key(98),
        tokens: vec!["dios".into()],
    });
    source.push(SourceVerse {
        key: key(97),
        lemmas: vec![],
    });
    target.push(TargetVerse {
        key: key(97),
        tokens: vec!["dios".into()],
    });
    let alignment = align(&source, &target, &AlignConfig::default());
    assert_eq!(alignment.verses.len(), 10);
    assert!(alignment.verses.iter().all(|v| v.key.verse < 10));
    assert!(align(&[], &[], &AlignConfig::default()).verses.is_empty());
}

#[test]
fn rendering_filters_apply() {
    let (source, target) = corpus(60, false);
    let config = AlignConfig {
        min_rendering_count: 61,
        ..AlignConfig::default()
    };
    let alignment = align(&source, &target, &config);
    assert!(alignment.renderings.is_empty());
}

#[test]
fn tokenizer_matches_expectations() {
    assert_eq!(
        tokenize("3 Bienaventurados los pobres en espíritu,"),
        ["bienaventurados", "los", "pobres", "en", "espíritu"]
    );
    assert_eq!(tokenize("¿Qué? —dijo— «sí»."), ["qué", "dijo", "sí"]);
}

#[test]
fn aligns_two_thousand_verses_quickly() {
    let fillers: Vec<String> = (0..400).map(|i| format!("w{i}")).collect();
    let mut source = Vec::new();
    let mut target = Vec::new();
    for v in 0..2000u16 {
        let lemmas: Vec<u32> = (0..15)
            .map(|k| u32::from(v.wrapping_mul(7).wrapping_add(k * 13) % 400))
            .collect();
        let tokens: Vec<String> = lemmas
            .iter()
            .map(|&l| fillers[l as usize].clone())
            .collect();
        let key = VerseKey {
            book: 1,
            chapter: v / 50,
            verse: v % 50,
        };
        source.push(SourceVerse { key, lemmas });
        target.push(TargetVerse { key, tokens });
    }
    let start = Instant::now();
    let alignment = align(&source, &target, &AlignConfig::default());
    let elapsed = start.elapsed();
    assert_eq!(alignment.verses.len(), 2000);
    assert!(elapsed.as_secs() < 30, "took {elapsed:?}");
    let (_, list) = alignment
        .renderings
        .iter()
        .find(|(l, _)| *l == 5)
        .expect("lemma 5 seen");
    assert_eq!(list[0].token, "w5");
}

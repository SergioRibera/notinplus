//! What a word of the original text means, put in words.
//!
//! How it reads, what it means in this verse and elsewhere, its grammar
//! and where it comes from. The «Original» tab and the quotes of original
//! text both read it from here, so they always say the same.

use notin_i18n::{Lang, tr};

use super::{LemmaView, Originals, Store, Testament, WordView, align, morph, respell};
use crate::db::LinkRow;

/// The language of the original text of `testament`.
#[must_use]
pub const fn language_of(testament: Testament) -> morph::Language {
    match testament {
        Testament::Old => morph::Language::Hebrew,
        Testament::New => morph::Language::Greek,
    }
}

/// The text of the translation `links` mark for the word at `position`,
/// pieces joined; `None` when it is not aligned.
#[must_use]
pub fn here_span(links: &[LinkRow], verse_text: &str, position: u16) -> Option<String> {
    let here = links
        .iter()
        .filter(|l| l.position == position)
        .map(|l| align::span_text(verse_text, l))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" … ");
    (!here.is_empty()).then_some(here)
}

/// How `lemma` sounds, spelled for a reader of `lang`; empty when the
/// dictionary has no pronunciation.
#[must_use]
pub fn respelling(lemma: &LemmaView, language: morph::Language, lang: Lang) -> String {
    match language {
        morph::Language::Hebrew => {
            respell::hebrew(lang, &lemma.translit, &lemma.pronunciation_strong)
        }
        morph::Language::Greek => respell::greek(lang, &lemma.lemma, &lemma.pronunciation_strong),
    }
}

/// What a word means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meaning {
    /// How it reads in this verse.
    pub here: String,
    /// `here` fell back to the English gloss.
    pub here_english: bool,
    /// What else it means.
    pub also: Also,
}

/// The word's meaning beyond this verse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Also {
    /// How the open translation renders the lemma elsewhere, with how
    /// many times each.
    Renderings {
        bible: String,
        items: Vec<(String, u32)>,
    },
    /// The dictionary senses, with a note when they are English or
    /// machine translated.
    Senses {
        items: Vec<String>,
        note: Option<String>,
    },
}

impl Also {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Renderings { items, .. } => items.is_empty(),
            Self::Senses { items, .. } => items.is_empty(),
        }
    }
}

/// Here: the aligned span `span`, else the pack's contextual gloss. Also:
/// how the translation `bible_id` (`bible_name`) renders the lemma
/// elsewhere, else the senses in `lang`, else the English ones.
#[must_use]
#[allow(
    clippy::too_many_arguments,
    reason = "every input is a different source"
)]
pub fn meaning(
    originals: &Originals,
    store: &Store,
    bible_id: &str,
    bible_name: &str,
    word: &WordView,
    span: Option<&str>,
    language: morph::Language,
    lang: Lang,
) -> Meaning {
    let lemma = store.lemma(word.lemma);
    // An article or a particle aligns noisily; its own gloss says more.
    let span = (!morph::is_function_word(&word.morph, language))
        .then_some(span)
        .flatten();
    let (here, here_english) = match (span, &word.gloss) {
        (Some(span), _) => (span.to_owned(), false),
        (None, Some(gloss)) => (gloss.clone(), false),
        (None, None) => (word.gloss_en.clone(), lang != Lang::En),
    };
    let renderings = originals.renderings(bible_id, word.lemma);
    if !renderings.is_empty() {
        return Meaning {
            here,
            here_english,
            also: Also::Renderings {
                bible: bible_name.to_owned(),
                items: renderings
                    .iter()
                    .map(|r| (r.token.clone(), r.count))
                    .collect(),
            },
        };
    }
    Meaning {
        here,
        here_english,
        also: senses(store, lemma.as_ref(), lang),
    }
}

/// The dictionary senses of `lemma` in `lang`, else in English with a
/// note saying so.
#[must_use]
pub fn senses(store: &Store, lemma: Option<&LemmaView>, lang: Lang) -> Also {
    let found = lemma
        .map(|l| store.senses(&l.strong, lang.tag()))
        .unwrap_or_default();
    if found.is_empty() {
        return Also::Senses {
            items: lemma.map(|l| l.senses_en.clone()).unwrap_or_default(),
            note: (lang != Lang::En).then(|| tr!("originals-meaning-english")),
        };
    }
    let automatic = found
        .iter()
        .all(|s| s.origin == originals_format::Origin::Auto);
    Also::Senses {
        items: found.into_iter().map(|s| s.text).collect(),
        note: automatic.then(|| tr!("originals-automatic")),
    }
}

/// The grammar of a word in words, its prefixes and suffixes after it.
#[must_use]
pub fn grammar_line(word: &WordView, language: morph::Language) -> String {
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

/// The dictionary's derivation ("plural of H433;") with every Strong
/// number replaced by the word it names and its pronunciation, so the
/// line reads without the numbering.
#[must_use]
pub fn derivation_text(store: &Store, derivation: &str, lang: Lang) -> String {
    let mut out = String::with_capacity(derivation.len());
    let mut rest = derivation.trim().trim_end_matches(';').trim_end();
    while let Some((before, number, after)) = next_strong_number(rest) {
        out.push_str(before);
        match store.lemma_by_strong(number) {
            Some(lemma) => {
                let language = if number.starts_with('H') {
                    morph::Language::Hebrew
                } else {
                    morph::Language::Greek
                };
                let respelled = respelling(&lemma, language, lang);
                out.push_str(&lemma.lemma);
                out.push_str(" (");
                out.push_str(if respelled.is_empty() {
                    &lemma.translit
                } else {
                    &respelled
                });
                out.push(')');
            }
            None => out.push_str(number),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// The text before the first Strong number in `text`, the number and
/// the text after it.
fn next_strong_number(text: &str) -> Option<(&str, &str, &str)> {
    let bytes = text.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        let at_word_start = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        if at_word_start && matches!(bytes[start], b'H' | b'G') {
            let mut end = start + 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start + 1 {
                if end < bytes.len() && bytes[end].is_ascii_uppercase() {
                    end += 1;
                }
                let bounded = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
                if bounded {
                    return Some((&text[..start], &text[start..end], &text[end..]));
                }
            }
        }
        start += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::next_strong_number;

    #[test]
    fn strong_numbers_are_found_whole_words_only() {
        assert_eq!(
            next_strong_number("plural of H433;"),
            Some(("plural of ", "H433", ";"))
        );
        assert_eq!(
            next_strong_number("from H2616A and (G1)"),
            Some(("from ", "H2616A", " and (G1)"))
        );
        assert_eq!(next_strong_number("HAG12 is not one"), None);
        assert_eq!(next_strong_number("a primitive root"), None);
    }
}

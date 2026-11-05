//! Morphology codes as words: `V-PAI-3S` → "verb, present, indicative,
//! 3rd person singular"; `Ncfsa` → "noun, feminine singular".
//!
//! Greek codes follow the Robinson scheme `STEPBible` uses in TAGNT;
//! Hebrew ones the `OpenScriptures` scheme TAHOT uses, without the leading
//! language letter. The output is a list of interface strings; unknown
//! pieces are left out rather than guessed.

/// Which scheme a code is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Hebrew,
    Greek,
}

/// The grammar of a code, as interface strings in reading order
/// (part of speech first). Empty when nothing in it is known.
#[must_use]
pub fn describe(code: &str, language: Language) -> Vec<String> {
    match language {
        Language::Greek => greek(code),
        Language::Hebrew => hebrew(code),
    }
    .into_iter()
    .map(|key| crate::LOCALES.text(key))
    .collect()
}

/// Whether a code is a function word (article, preposition,
/// conjunction, particle, pronominal suffix): the reader shows its
/// grammar but no meaning, since it has none worth a line.
#[must_use]
pub fn is_function_word(code: &str, language: Language) -> bool {
    match language {
        Language::Greek => {
            let pos = code.split('-').next().unwrap_or(code);
            matches!(
                pos,
                "T" | "CONJ"
                    | "COND"
                    | "PRT"
                    | "PREP"
                    | "INJ"
                    | "ADV"
                    | "P"
                    | "R"
                    | "D"
                    | "K"
                    | "I"
                    | "X"
                    | "Q"
                    | "F"
                    | "S"
                    | "C"
            )
        }
        Language::Hebrew => matches!(code.chars().next(), Some('R' | 'C' | 'T' | 'S' | 'P' | 'D')),
    }
}

/// `V-PAI-3S`, `N-NSF`, `P-1NS`, `A-NPM-C`, `V-AAP-NSM`.
fn greek(code: &str) -> Vec<&'static str> {
    let mut parts = code.split('-');
    let Some(pos) = parts.next() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let pos_key = match pos {
        "N" => "morph-noun",
        "A" => "morph-adjective",
        "T" => "morph-article",
        "V" => "morph-verb",
        "P" => "morph-pronoun-personal",
        "R" => "morph-pronoun-relative",
        "C" => "morph-pronoun-reciprocal",
        "D" => "morph-pronoun-demonstrative",
        "K" | "Q" => "morph-pronoun-correlative",
        "I" => "morph-pronoun-interrogative",
        "X" => "morph-pronoun-indefinite",
        "F" => "morph-pronoun-reflexive",
        "S" => "morph-pronoun-possessive",
        "ADV" => "morph-adverb",
        "CONJ" => "morph-conjunction",
        "COND" => "morph-conditional",
        "PRT" => "morph-particle",
        "PREP" => "morph-preposition",
        "INJ" => "morph-interjection",
        "ARAM" => "morph-aramaic",
        "HEB" => "morph-hebrew-word",
        _ => return Vec::new(),
    };
    out.push(pos_key);
    let Some(features) = parts.next() else {
        return out;
    };
    if pos == "V" {
        greek_verb(features, parts.next(), &mut out);
    } else {
        greek_nominal(features, &mut out);
    }
    for extra in parts {
        match extra {
            "C" => out.push("morph-comparative"),
            "S" => out.push("morph-superlative"),
            "P" => out.push("morph-person-name"),
            "L" => out.push("morph-place-name"),
            "T" => out.push("morph-title"),
            "G" => out.push("morph-gentilic"),
            "NUI" | "PRI" | "LI" | "OI" => out.push("morph-indeclinable"),
            _ => {}
        }
    }
    out
}

/// `PAI` + `3S`, `AAP` + `NSM`, `PAN`.
fn greek_verb(tvm: &str, rest: Option<&str>, out: &mut Vec<&'static str>) {
    let mut chars = tvm.chars();
    let tense = match chars.next() {
        Some('2') => match chars.next() {
            Some('A') => Some("morph-tense-aorist"),
            Some('F') => Some("morph-tense-future"),
            Some('X') => Some("morph-tense-perfect"),
            Some('Y') => Some("morph-tense-pluperfect"),
            _ => None,
        },
        Some('P') => Some("morph-tense-present"),
        Some('I') => Some("morph-tense-imperfect"),
        Some('F') => Some("morph-tense-future"),
        Some('A') => Some("morph-tense-aorist"),
        Some('X') => Some("morph-tense-perfect"),
        Some('Y') => Some("morph-tense-pluperfect"),
        _ => None,
    };
    let voice = match chars.next() {
        Some('A') => Some("morph-voice-active"),
        Some('M') => Some("morph-voice-middle"),
        Some('P') => Some("morph-voice-passive"),
        Some('E' | 'N') => Some("morph-voice-middle-or-passive"),
        Some('D' | 'O' | 'Q' | 'X') => Some("morph-voice-deponent"),
        _ => None,
    };
    let mood = match chars.next() {
        Some('I') => Some("morph-mood-indicative"),
        Some('S') => Some("morph-mood-subjunctive"),
        Some('O') => Some("morph-mood-optative"),
        Some('M') => Some("morph-mood-imperative"),
        Some('N') => Some("morph-mood-infinitive"),
        Some('P' | 'R') => Some("morph-mood-participle"),
        _ => None,
    };
    out.extend([tense, voice, mood].into_iter().flatten());
    let Some(rest) = rest else {
        return;
    };
    if rest.starts_with(|c: char| c.is_ascii_digit()) {
        greek_person_number(rest, out);
    } else {
        greek_nominal(rest, out);
    }
}

/// `3S`, `1P`.
fn greek_person_number(code: &str, out: &mut Vec<&'static str>) {
    let mut chars = code.chars();
    let person = match chars.next() {
        Some('1') => Some("morph-person-1"),
        Some('2') => Some("morph-person-2"),
        Some('3') => Some("morph-person-3"),
        _ => None,
    };
    let number = match chars.next() {
        Some('S') => Some("morph-singular"),
        Some('P') => Some("morph-plural"),
        _ => None,
    };
    out.extend([person, number].into_iter().flatten());
}

/// `NSF`, `1NS` (pronouns carry the person first), `GPM`.
fn greek_nominal(code: &str, out: &mut Vec<&'static str>) {
    let mut chars = code.chars().peekable();
    if let Some(person) = chars.peek().copied().filter(char::is_ascii_digit) {
        chars.next();
        out.extend(match person {
            '1' => Some("morph-person-1"),
            '2' => Some("morph-person-2"),
            '3' => Some("morph-person-3"),
            _ => None,
        });
    }
    let case = match chars.next() {
        Some('N') => Some("morph-case-nominative"),
        Some('G') => Some("morph-case-genitive"),
        Some('D') => Some("morph-case-dative"),
        Some('A') => Some("morph-case-accusative"),
        Some('V') => Some("morph-case-vocative"),
        _ => None,
    };
    let number = match chars.next() {
        Some('S') => Some("morph-singular"),
        Some('P') => Some("morph-plural"),
        _ => None,
    };
    let gender = match chars.next() {
        Some('M') => Some("morph-masculine"),
        Some('F') => Some("morph-feminine"),
        Some('N') => Some("morph-neuter"),
        _ => None,
    };
    out.extend([case, number, gender].into_iter().flatten());
}

/// `Ncfsa`, `Vqp3ms`, `Td`, `R`, `Sp3ms`, `Aamsa`, `Pp3ms`.
fn hebrew(code: &str) -> Vec<&'static str> {
    let mut chars = code.chars();
    let mut out = Vec::new();
    match chars.next() {
        Some('N') => {
            out.push("morph-noun");
            if chars.next() == Some('p') {
                out.push("morph-proper-name");
            }
            hebrew_gender_number(&mut chars, &mut out);
            match chars.next() {
                Some('c') => out.push("morph-state-construct"),
                Some('d') => out.push("morph-state-determined"),
                _ => {}
            }
        }
        Some('V') => {
            out.push("morph-verb");
            out.extend(match chars.next() {
                Some('q') => Some("morph-stem-qal"),
                Some('N') => Some("morph-stem-niphal"),
                Some('p') => Some("morph-stem-piel"),
                Some('P') => Some("morph-stem-pual"),
                Some('h') => Some("morph-stem-hiphil"),
                Some('H') => Some("morph-stem-hophal"),
                Some('t') => Some("morph-stem-hithpael"),
                Some(_) => Some("morph-stem-other"),
                None => None,
            });
            out.extend(match chars.next() {
                Some('p') => Some("morph-tense-perfect"),
                Some('q') => Some("morph-tense-sequential-perfect"),
                Some('i') => Some("morph-tense-imperfect"),
                Some('w') => Some("morph-tense-sequential-imperfect"),
                Some('h') => Some("morph-mood-cohortative"),
                Some('j') => Some("morph-mood-jussive"),
                Some('v') => Some("morph-mood-imperative"),
                Some('r') => Some("morph-participle-active"),
                Some('s') => Some("morph-participle-passive"),
                Some('a') => Some("morph-infinitive-absolute"),
                Some('c') => Some("morph-infinitive-construct"),
                _ => None,
            });
            hebrew_person(&mut chars, &mut out);
            hebrew_gender_number(&mut chars, &mut out);
        }
        Some('A') => {
            out.push(match chars.next() {
                Some('c') => "morph-number-cardinal",
                Some('o') => "morph-number-ordinal",
                Some('g') => "morph-gentilic",
                _ => "morph-adjective",
            });
            hebrew_gender_number(&mut chars, &mut out);
        }
        Some('R') => {
            out.push("morph-preposition");
            if chars.next() == Some('d') {
                out.push("morph-article");
            }
        }
        Some('C') => out.push("morph-conjunction"),
        Some('D') => out.push("morph-adverb"),
        Some('T') => out.push(match chars.next() {
            Some('d') => "morph-article",
            Some('o') => "morph-object-marker",
            Some('n') => "morph-negative",
            Some('i') => "morph-interrogative",
            Some('j') => "morph-interjection",
            Some('r') => "morph-relative",
            Some('m') => "morph-pronoun-demonstrative",
            Some('a') => "morph-affirmation",
            Some('e') => "morph-exhortation",
            _ => "morph-particle",
        }),
        Some('P') => {
            out.push(match chars.next() {
                Some('d') => "morph-pronoun-demonstrative",
                Some('f') => "morph-pronoun-indefinite",
                Some('i') => "morph-pronoun-interrogative",
                Some('r') => "morph-pronoun-relative",
                _ => "morph-pronoun-personal",
            });
            hebrew_person(&mut chars, &mut out);
            hebrew_gender_number(&mut chars, &mut out);
        }
        Some('S') => {
            out.push(match chars.next() {
                Some('d') => "morph-suffix-directional",
                Some('h' | 'n') => "morph-suffix-paragogic",
                _ => "morph-suffix-pronominal",
            });
            hebrew_person(&mut chars, &mut out);
            hebrew_gender_number(&mut chars, &mut out);
        }
        _ => {}
    }
    out
}

fn hebrew_person(chars: &mut std::str::Chars<'_>, out: &mut Vec<&'static str>) {
    let mut peek = chars.clone();
    let person = match peek.next() {
        Some('1') => Some("morph-person-1"),
        Some('2') => Some("morph-person-2"),
        Some('3') => Some("morph-person-3"),
        _ => None,
    };
    if let Some(person) = person {
        chars.next();
        out.push(person);
    }
}

fn hebrew_gender_number(chars: &mut std::str::Chars<'_>, out: &mut Vec<&'static str>) {
    let mut peek = chars.clone();
    let gender = match peek.next() {
        Some('m') => Some("morph-masculine"),
        Some('f') => Some("morph-feminine"),
        Some('b') => Some("morph-both-genders"),
        _ => None,
    };
    if matches!(peek_first(chars), Some('m' | 'f' | 'b' | 'c')) {
        chars.next();
    }
    let number = match peek_first(chars) {
        Some('s') => Some("morph-singular"),
        Some('p') => Some("morph-plural"),
        Some('d') => Some("morph-dual"),
        _ => None,
    };
    if number.is_some() {
        chars.next();
    }
    out.extend([gender, number].into_iter().flatten());
}

fn peek_first(chars: &std::str::Chars<'_>) -> Option<char> {
    chars.clone().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greek_verbs_and_nouns() {
        assert_eq!(
            greek("V-PAI-3S"),
            vec![
                "morph-verb",
                "morph-tense-present",
                "morph-voice-active",
                "morph-mood-indicative",
                "morph-person-3",
                "morph-singular"
            ]
        );
        assert_eq!(
            greek("V-2AAP-NSM"),
            vec![
                "morph-verb",
                "morph-tense-aorist",
                "morph-voice-active",
                "morph-mood-participle",
                "morph-case-nominative",
                "morph-singular",
                "morph-masculine"
            ]
        );
        assert_eq!(
            greek("N-GSF"),
            vec![
                "morph-noun",
                "morph-case-genitive",
                "morph-singular",
                "morph-feminine"
            ]
        );
        assert_eq!(
            greek("P-1NS"),
            vec![
                "morph-pronoun-personal",
                "morph-person-1",
                "morph-case-nominative",
                "morph-singular"
            ]
        );
        assert_eq!(
            greek("A-NPM-C"),
            vec![
                "morph-adjective",
                "morph-case-nominative",
                "morph-plural",
                "morph-masculine",
                "morph-comparative"
            ]
        );
        assert_eq!(
            greek("N-GSM-P"),
            vec![
                "morph-noun",
                "morph-case-genitive",
                "morph-singular",
                "morph-masculine",
                "morph-person-name"
            ]
        );
        assert_eq!(greek("CONJ"), vec!["morph-conjunction"]);
        assert!(greek("ZZZ").is_empty());
    }

    #[test]
    fn hebrew_verbs_nouns_and_affixes() {
        assert_eq!(
            hebrew("Ncfsa"),
            vec!["morph-noun", "morph-feminine", "morph-singular"]
        );
        assert_eq!(
            hebrew("Ncmpc"),
            vec![
                "morph-noun",
                "morph-masculine",
                "morph-plural",
                "morph-state-construct"
            ]
        );
        assert_eq!(
            hebrew("Vqp3ms"),
            vec![
                "morph-verb",
                "morph-stem-qal",
                "morph-tense-perfect",
                "morph-person-3",
                "morph-masculine",
                "morph-singular"
            ]
        );
        assert_eq!(
            hebrew("Vqw3ms"),
            vec![
                "morph-verb",
                "morph-stem-qal",
                "morph-tense-sequential-imperfect",
                "morph-person-3",
                "morph-masculine",
                "morph-singular"
            ]
        );
        assert_eq!(hebrew("Td"), vec!["morph-article"]);
        assert_eq!(hebrew("R"), vec!["morph-preposition"]);
        assert_eq!(
            hebrew("Sp3ms"),
            vec![
                "morph-suffix-pronominal",
                "morph-person-3",
                "morph-masculine",
                "morph-singular"
            ]
        );
        assert_eq!(
            hebrew("Npm"),
            vec!["morph-noun", "morph-proper-name", "morph-masculine"]
        );
        assert_eq!(
            hebrew("Aamsa"),
            vec!["morph-adjective", "morph-masculine", "morph-singular"]
        );
    }

    #[test]
    fn function_words() {
        assert!(is_function_word("T-NSM", Language::Greek));
        assert!(is_function_word("CONJ", Language::Greek));
        assert!(!is_function_word("N-NSM", Language::Greek));
        assert!(!is_function_word("V-PAI-3S", Language::Greek));
        assert!(is_function_word("Td", Language::Hebrew));
        assert!(is_function_word("R", Language::Hebrew));
        assert!(!is_function_word("Ncfsa", Language::Hebrew));
    }

    #[test]
    fn described_in_spanish() {
        notein_i18n::Lang::Es.make_current();
        let words = describe("V-PAI-3S", Language::Greek);
        assert_eq!(words[0], "verbo");
        assert_eq!(words.len(), 6);
    }
}

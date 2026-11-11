//! Matching that ignores case and accents, with excerpts, for plugin
//! search.

/// Text folded to lower case without accents, remembering where each folded byte came
/// from so matches map back to the original.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folded<'a> {
    original: &'a str,
    lower: String,
    /// `origin[i]` = byte offset in `original` of the char that produced
    /// folded byte `i`; one extra entry for the end.
    origin: Vec<usize>,
}

impl<'a> Folded<'a> {
    #[must_use]
    pub fn new(original: &'a str) -> Self {
        let mut lower = String::with_capacity(original.len());
        let mut origin = Vec::with_capacity(original.len() + 1);
        for (at, ch) in original.char_indices() {
            for ch_lower in ch.to_lowercase() {
                let before = lower.len();
                lower.push(plain(ch_lower));
                origin.extend(std::iter::repeat_n(at, lower.len() - before));
            }
        }
        origin.push(original.len());
        Self {
            original,
            lower,
            origin,
        }
    }

    /// Byte range in the original text of the first match of an
    /// already [`fold`]ed `needle`.
    #[must_use]
    pub fn find(&self, needle: &str) -> Option<(usize, usize)> {
        if needle.is_empty() {
            return None;
        }
        let at = self.lower.find(needle)?;
        let end = at + needle.len();
        Some((self.origin[at], self.origin[end]))
    }

    /// The match with up to `context` characters on each side, white
    /// space collapsed and ellipses where it was cut.
    #[must_use]
    pub fn excerpt(&self, needle: &str, context: usize) -> Option<String> {
        let (start, end) = self.find(needle)?;
        let before = &self.original[..start];
        let after = &self.original[end..];
        let from = before
            .char_indices()
            .rev()
            .take(context)
            .last()
            .map_or(start, |(i, _)| i);
        let to = after
            .char_indices()
            .nth(context)
            .map_or(self.original.len(), |(i, _)| end + i);
        let piece = self.original[from..to]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        Some(format!(
            "{}{piece}{}",
            if from > 0 { "…" } else { "" },
            if to < self.original.len() { "…" } else { "" }
        ))
    }
}

/// `text` in lower case without accents, the form [`Folded::find`] expects.
#[must_use]
pub fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(plain)
        .collect()
}

/// A lower-case letter without its accent.
const fn plain(ch: char) -> char {
    match ch {
        'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' => 'a',
        'é' | 'è' | 'ë' | 'ê' => 'e',
        'í' | 'ì' | 'ï' | 'î' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
        'ú' | 'ù' | 'ü' | 'û' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{Folded, fold};

    #[test]
    fn matches_map_back_to_the_original() {
        let text = "Ninguna CONDENACIÓN hay";
        let folded = Folded::new(text);
        let (start, end) = folded.find(&fold("condenación")).expect("match");
        assert_eq!(&text[start..end], "CONDENACIÓN");
    }

    #[test]
    fn accents_do_not_matter_either_way() {
        let text = "Génesis y éxodo";
        let folded = Folded::new(text);
        let (start, end) = folded.find(&fold("GENESIS")).expect("match");
        assert_eq!(&text[start..end], "Génesis");
        assert!(folded.find(&fold("exodo")).is_some());
    }

    #[test]
    fn excerpt_cuts_with_ellipses() {
        let text = "uno dos tres cuatro cinco seis siete";
        let folded = Folded::new(text);
        assert_eq!(
            folded.excerpt("cuatro", 4).as_deref(),
            Some("…res cuatro cin…")
        );
        assert_eq!(folded.excerpt("uno", 100).as_deref(), Some(text));
        assert_eq!(folded.excerpt("ocho", 4), None);
    }
}

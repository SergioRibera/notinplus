//! Minimal Unicode-aware tokenizer for translation text.

/// Splits `text` into lowercase word tokens.
///
/// Letters (with their diacritics, precomposed or combining) are kept;
/// digits, punctuation and symbols act as separators, so verse numbers
/// and quotation marks never become tokens. No stemming is applied.
#[must_use]
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c.is_alphabetic() || is_combining_mark(c) {
            current.extend(c.to_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Combining diacritics that must stay attached to the letter before them
/// (`is_alphabetic` reports them as non-letters).
const fn is_combining_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'
            | '\u{0591}'..='\u{05C7}'
            | '\u{0610}'..='\u{061A}'
            | '\u{064B}'..='\u{065F}'
            | '\u{0670}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

#[cfg(test)]
mod tests {
    use super::tokenize;

    #[test]
    fn splits_on_punctuation_and_digits() {
        assert_eq!(
            tokenize("16 Porque el mismo Espíritu da testimonio; ¡«hijos» de Dios!"),
            [
                "porque",
                "el",
                "mismo",
                "espíritu",
                "da",
                "testimonio",
                "hijos",
                "de",
                "dios"
            ]
        );
    }

    #[test]
    fn keeps_combining_marks() {
        assert_eq!(tokenize("pneu\u{0302}ma"), ["pneu\u{0302}ma"]);
    }

    #[test]
    fn empty_and_whitespace() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("  12 — ").is_empty());
    }
}

/// Splits `text` exactly like [`tokenize`] but keeps the letters as
/// written, so a span of token indices from an alignment can be shown
/// in the translation's own spelling.
#[must_use]
pub fn tokenize_surface(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c.is_alphabetic() || is_combining_mark(c) {
            current.push(c);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod surface_tests {
    use super::{tokenize, tokenize_surface};

    #[test]
    fn surface_tokens_line_up_with_lowercase_ones() {
        let text = "16 Porque el mismo Espíritu; ¡«hijos» de Dios!";
        let surface = tokenize_surface(text);
        assert_eq!(surface[3], "Espíritu");
        assert_eq!(surface.len(), tokenize(text).len());
    }
}

//! Verse text and its line layout out of the pieces of a fetch.bible /
//! biblionlabs verse.
//!
//! Those sources mark poetry lines with `\n` and the end of a paragraph or
//! stanza with `\n\n`, inside or at the edges of the verse's strings. The
//! stored text keeps one space where each break was, so it reads, searches
//! and counts words the same as a flat text; the breaks travel beside it
//! as byte offsets of those spaces ([`LineBreak`]).

/// What ends a line of text: a new line, or a new paragraph or stanza.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Break {
    Line,
    Paragraph,
}

impl Break {
    const fn from_run(newlines: usize) -> Option<Self> {
        match newlines {
            0 => None,
            1 => Some(Self::Line),
            _ => Some(Self::Paragraph),
        }
    }
}

/// A break inside a verse: the space at byte `at` of its text starts a
/// new line (or paragraph).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineBreak {
    pub at: usize,
    pub kind: Break,
}

/// One verse laid out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaidVerse {
    /// Whitespace runs folded to one space, trimmed.
    pub text: String,
    /// Breaks inside the text, in order.
    pub breaks: Vec<LineBreak>,
    /// Break before the first word (the verse opens a line or paragraph).
    pub starts: Option<Break>,
    /// Break after the last word (the next verse opens a line or paragraph).
    pub ends: Option<Break>,
}

/// Lay out the text pieces of one verse, in order.
pub fn lay_out<'a>(pieces: impl IntoIterator<Item = &'a str>) -> LaidVerse {
    let mut verse = LaidVerse::default();
    // Newlines since the last word, and whether plain whitespace (or a
    // piece boundary) came since.
    let mut newlines = 0usize;
    let mut space;
    for piece in pieces {
        // Pieces meet with a space, as importers always joined them.
        space = true;
        for ch in piece.chars() {
            if ch == '\n' {
                newlines += 1;
                continue;
            }
            if ch.is_whitespace() {
                space = true;
                continue;
            }
            if verse.text.is_empty() {
                verse.starts = Break::from_run(newlines);
            } else if let Some(kind) = Break::from_run(newlines) {
                verse.breaks.push(LineBreak {
                    at: verse.text.len(),
                    kind,
                });
                verse.text.push(' ');
            } else if space {
                verse.text.push(' ');
            }
            verse.text.push(ch);
            newlines = 0;
            space = false;
        }
    }
    verse.ends = Break::from_run(newlines);
    verse
}

/// `breaks` as stored: `12,40p` (byte offsets, `p` for a paragraph).
#[must_use]
pub fn encode_breaks(breaks: &[LineBreak]) -> String {
    let mut out = String::new();
    for (index, line) in breaks.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&line.at.to_string());
        if line.kind == Break::Paragraph {
            out.push('p');
        }
    }
    out
}

/// Read what [`encode_breaks`] wrote; unreadable entries are skipped.
#[must_use]
pub fn decode_breaks(stored: &str) -> Vec<LineBreak> {
    stored
        .split(',')
        .filter_map(|entry| {
            let (digits, kind) = entry
                .strip_suffix('p')
                .map_or((entry, Break::Line), |digits| (digits, Break::Paragraph));
            Some(LineBreak {
                at: digits.trim().parse().ok()?,
                kind,
            })
        })
        .collect()
}

/// A footnote as sources write it, `1.21 Esto es, Salvador.` or
/// `23:1 See Revelation 7:17.`, without the leading reference.
#[must_use]
pub fn note_text(note: &str) -> &str {
    let note = note.trim();
    let mut rest = note;
    let digits = |s: &str| s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let lead = digits(rest);
    if lead == 0 {
        return note;
    }
    rest = &rest[lead..];
    let Some(after) = rest.strip_prefix(['.', ':']) else {
        return note;
    };
    let verse = digits(after);
    if verse == 0 {
        return note;
    }
    let mut tail = &after[verse..];
    // A range: `5.3-4`.
    if let Some(range) = tail.strip_prefix(['-', '–']) {
        tail = &range[digits(range)..];
    }
    tail.strip_prefix(char::is_whitespace)
        .map_or(note, str::trim_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poetry_lines_keep_spaces_and_offsets() {
        let laid = lay_out([
            "En lugares de delicados pastos me hará descansar;\nJunto a aguas de reposo me pastoreará.\n",
        ]);
        assert_eq!(
            laid.text,
            "En lugares de delicados pastos me hará descansar; Junto a aguas de reposo me pastoreará."
        );
        assert_eq!(laid.breaks.len(), 1);
        assert_eq!(&laid.text[laid.breaks[0].at..=laid.breaks[0].at], " ");
        assert_eq!(&laid.text[laid.breaks[0].at + 1..][..5], "Junto");
        assert_eq!(laid.ends, Some(Break::Line));
        assert_eq!(laid.starts, None);
    }

    #[test]
    fn pieces_around_a_note_join_like_before() {
        let laid = lay_out(["The LORD is my shepherd;", "\nI shall not want.\n\n"]);
        assert_eq!(laid.text, "The LORD is my shepherd; I shall not want.");
        assert_eq!(laid.breaks[0].kind, Break::Line);
        assert_eq!(laid.ends, Some(Break::Paragraph));

        let prose = lay_out([
            "Bienaventurados los que lloran,",
            " porque ellos recibirán consolación.",
        ]);
        assert_eq!(
            prose.text,
            "Bienaventurados los que lloran, porque ellos recibirán consolación."
        );
        assert!(prose.breaks.is_empty());
        assert_eq!(prose.ends, None);
    }

    #[test]
    fn leading_breaks_and_indentation() {
        let laid = lay_out(["\n    En lugares de delicados pastos me hará yacer:\nJunto á aguas"]);
        assert_eq!(laid.starts, Some(Break::Line));
        assert_eq!(
            laid.text,
            "En lugares de delicados pastos me hará yacer: Junto á aguas"
        );
    }

    #[test]
    fn breaks_round_trip() {
        let breaks = vec![
            LineBreak {
                at: 12,
                kind: Break::Line,
            },
            LineBreak {
                at: 40,
                kind: Break::Paragraph,
            },
        ];
        assert_eq!(encode_breaks(&breaks), "12,40p");
        assert_eq!(decode_breaks("12,40p"), breaks);
        assert!(decode_breaks("").is_empty());
    }

    #[test]
    fn notes_lose_their_reference() {
        assert_eq!(note_text("1.21 Esto es, Salvador."), "Esto es, Salvador.");
        assert_eq!(
            note_text("23:1 See Revelation 7:17."),
            "See Revelation 7:17."
        );
        assert_eq!(note_text("5.3-4 O, dichosos."), "O, dichosos.");
        assert_eq!(note_text("Or the valley"), "Or the valley");
    }
}

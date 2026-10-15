//! `[[kind:value]]` chip parser + serializer for bookmark bodies.
//!
//! Phase 6.1. The body is a markdown-lite string where references to
//! other content live in a bracketed syntax:
//!
//! - `[[canvas:<uuid>]]` — another bookmark in the library.
//! - `[[plugin:<plugin_id>/<item_hex>/<anchor_hex>]]` — anchor inside a
//!   source plugin instance. `item_hex` is a 32-char UUID hex; `anchor`
//!   is lower-case hex of the plugin's opaque anchor bytes.
//! - `[[url:<url>]]` — arbitrary url. Everything up to the closing `]]`
//!   is the url payload; `]]` cannot appear inside.
//!
//! Unrecognised kinds are passed through as plain text so a user never
//! loses characters. Malformed brackets (missing colon, truncated anchor
//! hex, bad uuid) also fall back to plain text — the parser never
//! fails.
//!
//! The in-memory editor model is [`Vec<Segment>`]. The editor compiles
//! this to a rope where every chip is a single `\u{FFFC}` (object
//! replacement) char + a side map of chip refs keyed by char index.
//! Round-tripping goes [`parse`] → edit → [`serialize`] with no silent
//! drift: `serialize(parse(s))` reproduces every chip in its canonical
//! form (even if the input had a case-insensitive kind, since all
//! kinds are already lowercase).

pub use notein_plugin::Completion;
use notein_plugin::{BookmarkId, SourceRef};

/// Object replacement character — Unicode "placeholder for inline
/// object". Reserved for this exact use case; Skia's paragraph builder
/// pairs this char with a shaper placeholder.
pub const CHIP_SENTINEL: char = '\u{FFFC}';

/// A piece of the compiled body: either literal text or a chip ref.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Chip(SourceRef),
}

/// Parse a body string into interleaved text + chip segments.
///
/// Recognised escapes:
///
/// - `\[[` — literal `[[` (prevents the next two chars being read as a
///   chip opener). Needed so a user can type `[[` by hand without
///   accidentally conjuring a chip on reload.
///
/// Never fails. Garbage input survives as plain text.
#[must_use]
pub fn parse(body: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut pending = String::new();
    let bytes = body.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        // `\[[` → literal `[[`. All three chars are ASCII so byte
        // indexing is safe.
        if bytes[i] == b'\\'
            && i + 2 < bytes.len()
            && bytes[i + 1] == b'['
            && bytes[i + 2] == b'['
        {
            pending.push_str("[[");
            i += 3;
            continue;
        }

        // Chip opener `[[`.
        if i + 1 < bytes.len() && bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let after = &body[i + 2..];
            if let Some(close) = after.find("]]")
                && let Some(chip) = decode_payload(&after[..close])
            {
                if !pending.is_empty() {
                    segments.push(Segment::Text(std::mem::take(&mut pending)));
                }
                segments.push(Segment::Chip(chip));
                i += 2 + close + 2;
                continue;
            }
            // No recognisable chip — keep `[[` as literal text and
            // keep scanning after it.
            pending.push_str("[[");
            i += 2;
            continue;
        }

        // Normal char. Use char-aware step so multi-byte UTF-8 keeps
        // its boundaries.
        let ch = body[i..].chars().next().unwrap();
        pending.push(ch);
        i += ch.len_utf8();
    }

    if !pending.is_empty() {
        segments.push(Segment::Text(pending));
    }
    segments
}

/// Serialize segments back to a `[[...]]` body string. Inverse of
/// [`parse`] for well-formed segment lists.
///
/// Text segments have any literal `[[` sequences escaped to `\[[` so
/// a reload doesn't mis-parse them as chip openers.
#[must_use]
pub fn serialize(segments: &[Segment]) -> String {
    let mut out = String::new();
    for seg in segments {
        match seg {
            Segment::Text(t) => out.push_str(&escape_text(t)),
            Segment::Chip(c) => encode_chip(c, &mut out),
        }
    }
    out
}

fn escape_text(s: &str) -> String {
    s.replace("[[", "\\[[")
}

/// Canonical `[[kind:value]]` form for a single ref. Used by the
/// serializer and by the autocomplete picker.
#[must_use]
pub fn encode_chip_str(chip: &SourceRef) -> String {
    let mut out = String::new();
    encode_chip(chip, &mut out);
    out
}

fn encode_chip(chip: &SourceRef, out: &mut String) {
    match chip {
        SourceRef::Canvas { bookmark } => {
            out.push_str("[[canvas:");
            out.push_str(&uuid_hex(bookmark.as_bytes()));
            out.push_str("]]");
        }
        SourceRef::Plugin {
            plugin_id,
            item,
            anchor,
            ..
        } => {
            out.push_str("[[plugin:");
            out.push_str(plugin_id);
            out.push('/');
            out.push_str(&uuid_hex(item));
            out.push('/');
            out.push_str(&hex(anchor));
            out.push_str("]]");
        }
        SourceRef::External { url } => {
            out.push_str("[[url:");
            // Percent-encode `]` so a URL that happens to contain
            // a right bracket (RFC says only inside `[v6]` host
            // literals) can't terminate the chip early.
            out.push_str(&url.replace(']', "%5D"));
            out.push_str("]]");
        }
    }
}

fn decode_payload(payload: &str) -> Option<SourceRef> {
    let (kind, value) = payload.split_once(':')?;
    match kind {
        "canvas" => decode_uuid(value).map(|b| SourceRef::Canvas {
            bookmark: BookmarkId::from_bytes(b),
        }),
        "plugin" => decode_plugin(value),
        "url" => (!value.is_empty()).then(|| SourceRef::External {
            url: value.replace("%5D", "]"),
        }),
        _ => None,
    }
}

fn decode_plugin(value: &str) -> Option<SourceRef> {
    let mut parts = value.splitn(3, '/');
    let plugin_id = parts.next()?;
    let item_hex = parts.next()?;
    let anchor_hex = parts.next()?;
    if plugin_id.is_empty() {
        return None;
    }
    let item = decode_uuid(item_hex)?;
    let anchor = decode_hex(anchor_hex)?;
    Some(SourceRef::Plugin {
        plugin_id: plugin_id.to_owned(),
        item,
        anchor,
        selection: None,
    })
}

fn decode_uuid(s: &str) -> Option<[u8; 16]> {
    uuid::Uuid::parse_str(s).ok().map(|u| *u.as_bytes())
}

fn uuid_hex(bytes: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes(*bytes).to_string()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Flatten segments into (rope string with `\u{FFFC}` sentinels, chip
/// list indexed by UTF-16 code-unit position). UTF-16 is the index
/// space `freya_edit::use_editable` uses internally; keeping chip
/// indices in the same space means they travel through `insert_char` /
/// `remove(range)` / `cursor_pos` with no conversion.
#[must_use]
pub fn to_rope_with_chips(segments: &[Segment]) -> (String, Vec<(usize, SourceRef)>) {
    let mut rope = String::new();
    let mut chips = Vec::new();
    let mut utf16_cursor = 0usize;
    for seg in segments {
        match seg {
            Segment::Text(t) => {
                utf16_cursor += t.encode_utf16().count();
                rope.push_str(t);
            }
            Segment::Chip(c) => {
                chips.push((utf16_cursor, c.clone()));
                rope.push(CHIP_SENTINEL);
                utf16_cursor += 1;
            }
        }
    }
    (rope, chips)
}

/// Inverse of [`to_rope_with_chips`]. Walks the rope + chip list,
/// splitting the rope into text runs between chip sentinels. Chip
/// consumption is strictly in-order — any `chip` whose idx doesn't
/// line up with its next sentinel is still included at that
/// sentinel's position (the editor keeps `chip_map` sorted + 1:1 with
/// sentinels after every edit; defensive against upstream drift).
#[must_use]
pub fn from_rope_with_chips(rope: &str, chips: &[(usize, SourceRef)]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut buf = String::new();
    let mut chip_iter = chips.iter();
    for ch in rope.chars() {
        if ch == CHIP_SENTINEL {
            if !buf.is_empty() {
                segments.push(Segment::Text(std::mem::take(&mut buf)));
            }
            match chip_iter.next() {
                Some((_, chip)) => segments.push(Segment::Chip(chip.clone())),
                None => {
                    // Orphan sentinel with no matching entry — swallow
                    // silently. Shouldn't happen in practice.
                }
            }
        } else {
            buf.push(ch);
        }
    }
    if !buf.is_empty() {
        segments.push(Segment::Text(buf));
    }
    segments
}

/// Re-anchor a chip map to the current rope. Walks the rope in order,
/// pairing every `\u{FFFC}` with the next chip from `prev`. Indices
/// returned are UTF-16 code-unit counts, matching
/// `freya_edit`'s cursor index space.
///
/// Entries that no longer have a sentinel are dropped (user deleted
/// that chip's placeholder char). Extra sentinels at the tail are
/// ignored — shouldn't happen in practice (no key event types a
/// `\u{FFFC}`) but keeps the function total.
#[must_use]
pub fn rebase_chips(prev: &[(usize, SourceRef)], rope: &str) -> Vec<(usize, SourceRef)> {
    let mut out = Vec::with_capacity(prev.len());
    let mut prev_iter = prev.iter();
    let mut utf16_idx = 0usize;
    for ch in rope.chars() {
        if ch == CHIP_SENTINEL
            && let Some((_, chip)) = prev_iter.next()
        {
            out.push((utf16_idx, chip.clone()));
        }
        utf16_idx += ch.len_utf16();
    }
    out
}

// `Completion` is defined canonically in `notein_plugin::Completion`
// so both plugins and the shell can produce entries. Re-exported at
// the top of this module.

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid_a() -> [u8; 16] {
        [1u8; 16]
    }

    fn uuid_b() -> [u8; 16] {
        [2u8; 16]
    }

    #[test]
    fn plain_text_passes_through() {
        let got = parse("hola mundo");
        assert_eq!(got, vec![Segment::Text("hola mundo".into())]);
    }

    #[test]
    fn empty_body_is_empty_vec() {
        assert_eq!(parse(""), Vec::<Segment>::new());
    }

    #[test]
    fn canvas_chip_round_trips() {
        let bm = BookmarkId::from_bytes(uuid_a());
        let input = format!("ver {}", encode_chip_str(&SourceRef::Canvas { bookmark: bm }));
        let segs = parse(&input);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], Segment::Text("ver ".into()));
        assert_eq!(segs[1], Segment::Chip(SourceRef::Canvas { bookmark: bm }));
        assert_eq!(serialize(&segs), input);
    }

    #[test]
    fn plugin_chip_round_trips_with_anchor() {
        let r = SourceRef::Plugin {
            plugin_id: "io.notein.pdf".into(),
            item: uuid_a(),
            anchor: vec![0xde, 0xad, 0xbe, 0xef],
            selection: None,
        };
        let input = encode_chip_str(&r);
        assert!(input.contains("deadbeef"));
        let segs = parse(&input);
        assert_eq!(segs, vec![Segment::Chip(r)]);
    }

    #[test]
    fn url_chip_preserves_slashes_and_colons() {
        let r = SourceRef::External {
            url: "https://example.com/path?q=1".into(),
        };
        let segs = parse(&encode_chip_str(&r));
        assert_eq!(segs, vec![Segment::Chip(r)]);
    }

    #[test]
    fn garbage_bracket_survives_as_text() {
        assert_eq!(
            parse("mira [[not-a-ref]] eso"),
            vec![Segment::Text("mira [[not-a-ref]] eso".into())]
        );
    }

    #[test]
    fn unterminated_bracket_is_text() {
        assert_eq!(
            parse("abre [[canvas:x "),
            vec![Segment::Text("abre [[canvas:x ".into())]
        );
    }

    #[test]
    fn bad_uuid_falls_back_to_text() {
        assert_eq!(
            parse("[[canvas:not-a-uuid]]"),
            vec![Segment::Text("[[canvas:not-a-uuid]]".into())]
        );
    }

    #[test]
    fn two_chips_adjacent() {
        let a = SourceRef::Canvas {
            bookmark: BookmarkId::from_bytes(uuid_a()),
        };
        let b = SourceRef::Canvas {
            bookmark: BookmarkId::from_bytes(uuid_b()),
        };
        let input = format!("{}{}", encode_chip_str(&a), encode_chip_str(&b));
        let segs = parse(&input);
        assert_eq!(segs, vec![Segment::Chip(a), Segment::Chip(b)]);
    }

    #[test]
    fn rope_and_chips_round_trip() {
        let bm = BookmarkId::from_bytes(uuid_a());
        let segs = vec![
            Segment::Text("ver ".into()),
            Segment::Chip(SourceRef::Canvas { bookmark: bm }),
            Segment::Text(" ya".into()),
        ];
        let (rope, chips) = to_rope_with_chips(&segs);
        assert_eq!(rope, "ver \u{FFFC} ya");
        assert_eq!(chips, vec![(4, SourceRef::Canvas { bookmark: bm })]);
        assert_eq!(from_rope_with_chips(&rope, &chips), segs);
    }

    #[test]
    fn rebase_chips_shifts_after_insert() {
        let chip = SourceRef::External {
            url: "x".into(),
        };
        let prev = vec![(2, chip.clone())];
        // User typed two chars before the chip: rope grew from "ab\u{FFFC}" to "abcd\u{FFFC}".
        let rope = "abcd\u{FFFC}";
        assert_eq!(rebase_chips(&prev, rope), vec![(4, chip)]);
    }

    /// One Backspace right of a chip = chip gone. Guarantees the
    /// atomic-delete contract (Phase 6 step 4). Because the chip
    /// occupies a single `\u{FFFC}` char in the rope, `use_editable`'s
    /// one-char delete already removes it in one op — no loop needed.
    #[test]
    fn literal_bracket_text_escapes_on_roundtrip() {
        let segs = vec![Segment::Text("ver [[canvas:xyz]] fin".into())];
        let encoded = serialize(&segs);
        // The literal `[[` becomes `\[[` on the wire so a reload
        // doesn't mis-parse it as a chip opener.
        assert!(encoded.starts_with("ver \\[["));
        // Round-trip returns the literal text unchanged.
        assert_eq!(parse(&encoded), segs);
    }

    #[test]
    fn url_with_bracket_escapes_right_bracket() {
        let r = SourceRef::External {
            url: "https://ex.com/x]y".into(),
        };
        let encoded = encode_chip_str(&r);
        assert!(encoded.contains("%5D"));
        assert_eq!(parse(&encoded), vec![Segment::Chip(r)]);
    }

    #[test]
    fn one_backspace_removes_whole_chip() {
        let chip = SourceRef::Canvas {
            bookmark: BookmarkId::from_bytes([9u8; 16]),
        };
        let prev = vec![(3, chip.clone())];
        // "abc<chip>" → user Backspaces at cursor 4 → rope becomes "abc".
        let after = "abc";
        assert_eq!(rebase_chips(&prev, after), vec![]);
    }

    #[test]
    fn rebase_chips_drops_removed_chip() {
        let a = SourceRef::External {
            url: "a".into(),
        };
        let b = SourceRef::External {
            url: "b".into(),
        };
        let prev = vec![(0, a.clone()), (2, b.clone())];
        // User deleted the FFFC at position 2.
        let rope = "\u{FFFC}xx";
        assert_eq!(rebase_chips(&prev, rope), vec![(0, a)]);
    }

    #[test]
    fn serialize_parse_identity() {
        let segs = vec![
            Segment::Text("hola ".into()),
            Segment::Chip(SourceRef::External {
                url: "https://ex.com".into(),
            }),
            Segment::Text(" fin".into()),
        ];
        assert_eq!(parse(&serialize(&segs)), segs);
    }
}

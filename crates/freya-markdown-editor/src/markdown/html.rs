//! Minimal inline HTML tag reader. Only understands the tags the rich
//! dialect writes back (`<span style>`, `<u>`, `<sup>`, ...) plus their
//! common synonyms; everything else stays an opaque [`Atom::Html`].
//!
//! [`Atom::Html`]: crate::document::Atom::Html

use crate::document::{MarkKind, Rgba};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Tag {
    Open { name: String, marks: Vec<MarkKind> },
    Close { name: String },
    LineBreak,
    Unknown,
}

impl Tag {
    pub(super) fn classify(raw: &str) -> Self {
        let raw = raw.trim();
        let Some(inner) = raw
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'))
        else {
            return Self::Unknown;
        };
        if let Some(name) = inner.strip_prefix('/') {
            let name = name.trim().to_ascii_lowercase();
            return if marks_for(&name, &[]).is_some() {
                Self::Close { name }
            } else {
                Self::Unknown
            };
        }
        let inner = inner.trim_end_matches('/').trim();
        let (name, rest) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
        let name = name.to_ascii_lowercase();
        if name == "br" {
            return Self::LineBreak;
        }
        let attributes = parse_attributes(rest);
        marks_for(&name, &attributes).map_or(Self::Unknown, |marks| Self::Open { name, marks })
    }
}

fn marks_for(name: &str, attributes: &[(String, String)]) -> Option<Vec<MarkKind>> {
    let attribute = |key: &str| {
        attributes
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let mut marks = match name {
        "u" | "ins" => vec![MarkKind::Underline],
        "s" | "del" | "strike" => vec![MarkKind::Strikethrough],
        "b" | "strong" => vec![MarkKind::Strong],
        "i" | "em" => vec![MarkKind::Emphasis],
        "mark" => vec![MarkKind::Highlight],
        "sup" => vec![MarkKind::Superscript],
        "sub" => vec![MarkKind::Subscript],
        "code" | "kbd" => vec![MarkKind::Code],
        "abbr" => vec![MarkKind::Abbreviation(unescape(
            attribute("title").unwrap_or_default(),
        ))],
        "span" => Vec::new(),
        "font" => {
            let mut marks = Vec::new();
            if let Some(color) = attribute("color").and_then(|v| v.parse::<Rgba>().ok()) {
                marks.push(MarkKind::Color(color));
            }
            if let Some(face) = attribute("face") {
                marks.push(MarkKind::FontFamily(clean_family(face)));
            }
            marks
        }
        _ => return None,
    };
    if let Some(style) = attribute("style") {
        marks.extend(style_marks(style));
    }
    Some(marks)
}

/// Marks described by a CSS declaration list.
pub(super) fn style_marks(style: &str) -> Vec<MarkKind> {
    style
        .split(';')
        .filter_map(|declaration| declaration.split_once(':'))
        .filter_map(|(property, value)| {
            let value = value.trim();
            match property.trim().to_ascii_lowercase().as_str() {
                "color" => value.parse().ok().map(MarkKind::Color),
                "background" | "background-color" => value.parse().ok().map(MarkKind::Background),
                "font-family" => Some(MarkKind::FontFamily(clean_family(value))),
                "font-size" => parse_px(value).map(MarkKind::FontSize),
                "font-weight" if matches!(value, "bold" | "700" | "800" | "900") => {
                    Some(MarkKind::Strong)
                }
                "font-style" if value == "italic" => Some(MarkKind::Emphasis),
                "text-decoration" | "text-decoration-line" if value.contains("underline") => {
                    Some(MarkKind::Underline)
                }
                "text-decoration" | "text-decoration-line" if value.contains("line-through") => {
                    Some(MarkKind::Strikethrough)
                }
                _ => None,
            }
        })
        .collect()
}

/// Text of an HTML attribute value with the common entities decoded.
pub(super) fn unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn clean_family(value: &str) -> String {
    value.trim().trim_matches(['"', '\'']).trim().to_owned()
}

fn parse_px(value: &str) -> Option<u16> {
    let number = value.trim().trim_end_matches("px").trim();
    let size = number.parse::<f32>().ok()?;
    if !(1.0..=999.0).contains(&size) {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "range checked above"
    )]
    Some(size.round() as u16)
}

fn parse_attributes(source: &str) -> Vec<(String, String)> {
    let mut attributes = Vec::new();
    let mut rest = source.trim();
    while !rest.is_empty() {
        let key_end = rest
            .find(|ch: char| ch == '=' || ch.is_whitespace())
            .unwrap_or(rest.len());
        let key = rest[..key_end].to_ascii_lowercase();
        if key.is_empty() {
            break;
        }
        rest = rest[key_end..].trim_start();
        let value = rest.strip_prefix('=').map_or_else(String::new, |after| {
            let (value, remaining) = split_value(after.trim_start());
            rest = remaining.trim_start();
            value.to_owned()
        });
        attributes.push((key, value));
    }
    attributes
}

/// Split an attribute value (quoted or bare) from the rest of the tag.
fn split_value(source: &str) -> (&str, &str) {
    if let Some(quote @ ('"' | '\'')) = source.chars().next() {
        let body = &source[1..];
        let end = body.find(quote).unwrap_or(body.len());
        return (&body[..end], body.get(end + 1..).unwrap_or(""));
    }
    let end = source.find(char::is_whitespace).unwrap_or(source.len());
    source.split_at(end)
}

/// Opening tag for a group of style marks, as written by the rich dialect.
pub(super) fn span_open(marks: &[&MarkKind]) -> String {
    let declarations: Vec<String> = marks
        .iter()
        .filter_map(|mark| match mark {
            MarkKind::Color(color) => Some(format!("color: {color}")),
            MarkKind::Background(color) => Some(format!("background-color: {color}")),
            MarkKind::FontFamily(family) => Some(format!("font-family: {family}")),
            MarkKind::FontSize(size) => Some(format!("font-size: {size}px")),
            _ => None,
        })
        .collect();
    format!("<span style=\"{}\">", declarations.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_styled_span() {
        let tag = Tag::classify(r#"<span style="color: #ff0000; font-size: 18px">"#);
        assert_eq!(
            tag,
            Tag::Open {
                name: "span".into(),
                marks: vec![
                    MarkKind::Color(Rgba::rgb(255, 0, 0)),
                    MarkKind::FontSize(18)
                ],
            }
        );
    }

    #[test]
    fn reads_simple_tags() {
        assert_eq!(
            Tag::classify("<u>"),
            Tag::Open {
                name: "u".into(),
                marks: vec![MarkKind::Underline]
            }
        );
        assert_eq!(Tag::classify("</u>"), Tag::Close { name: "u".into() });
        assert_eq!(Tag::classify("<br/>"), Tag::LineBreak);
        assert_eq!(Tag::classify("<video src=x>"), Tag::Unknown);
    }

    #[test]
    fn span_round_trip() {
        let color = MarkKind::Color(Rgba::rgb(1, 2, 3));
        let family = MarkKind::FontFamily("Fira Code".into());
        let open = span_open(&[&color, &family]);
        assert_eq!(
            Tag::classify(&open),
            Tag::Open {
                name: "span".into(),
                marks: vec![color, family]
            }
        );
    }
}

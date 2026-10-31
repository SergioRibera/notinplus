//! Search hits as the reader lists them: a reference and the snippet,
//! with the words that matched marked.

use crate::db::{SearchHit as DbSearchHit, snippet_markers};

/// One rendered search hit — the UI-facing shape.
#[derive(Clone, Debug)]
pub struct SearchHitView {
    /// Human-readable location (`"Juan 3:16"`).
    pub reference: String,
    /// Alternating normal / highlighted runs. Each `bold = true` run
    /// paints in a brighter colour so the match pops in the list.
    pub snippet_runs: Vec<SnippetRun>,
    /// Original hit — preserved so the pick callback can navigate the
    /// viewer to it without reconstructing the anchor.
    pub source: DbSearchHit,
}

#[derive(Clone, Debug)]
pub struct SnippetRun {
    pub text: String,
    pub bold: bool,
}

/// Build a [`SearchHitView`] from a raw DB hit. Splits the snippet on
/// the private-use markers written by `db::search::snippet()` and
/// derives the reference label from the hit coordinates.
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("bible::search_hit_view"))]
pub fn hit_to_view(hit: DbSearchHit, book_name: Option<&str>) -> SearchHitView {
    let (open, close) = snippet_markers();
    let runs = split_snippet(&hit.snippet, open, close);
    let book = book_name.unwrap_or(&hit.book_id);
    let reference = format!("{} {}:{}", book, hit.chapter, hit.verse);
    SearchHitView {
        reference,
        snippet_runs: runs,
        source: hit,
    }
}

fn split_snippet(snippet: &str, open: &str, close: &str) -> Vec<SnippetRun> {
    let mut out: Vec<SnippetRun> = Vec::new();
    let mut buf = String::new();
    let mut bold = false;
    let mut utf8 = [0u8; 4];
    for c in snippet.chars() {
        let token: &str = c.encode_utf8(&mut utf8);
        let toggle = if token == open {
            Some(true)
        } else if token == close {
            Some(false)
        } else {
            None
        };
        match toggle {
            Some(next) => {
                if !buf.is_empty() {
                    out.push(SnippetRun {
                        text: std::mem::take(&mut buf),
                        bold,
                    });
                }
                bold = next;
            }
            None => buf.push(c),
        }
    }
    if !buf.is_empty() {
        out.push(SnippetRun { text: buf, bold });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_splits_on_private_use_markers() {
        let (open, close) = snippet_markers();
        let s = format!("plain {open}bold{close} rest");
        let runs = split_snippet(&s, open, close);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text, "plain ");
        assert!(!runs[0].bold);
        assert_eq!(runs[1].text, "bold");
        assert!(runs[1].bold);
        assert_eq!(runs[2].text, " rest");
        assert!(!runs[2].bold);
    }

    #[test]
    fn hit_view_builds_reference_with_book_name() {
        let hit = DbSearchHit {
            bible_id: "spa".into(),
            book_id: "EPH".into(),
            chapter: 6,
            verse: 1,
            snippet: "Hijos obedeced".into(),
            score: -1.0,
        };
        let v = hit_to_view(hit, Some("Efesios"));
        assert_eq!(v.reference, "Efesios 6:1");
    }

    #[test]
    fn hit_view_falls_back_to_book_id_without_name() {
        let hit = DbSearchHit {
            bible_id: "spa".into(),
            book_id: "EPH".into(),
            chapter: 6,
            verse: 1,
            snippet: String::new(),
            score: -1.0,
        };
        let v = hit_to_view(hit, None);
        assert_eq!(v.reference, "EPH 6:1");
    }
}

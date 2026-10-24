//! Full-text search sheet.
//!
//! The sheet is split into two pure builders:
//!
//! - [`search_input`] — Freya [`Input`] with the plugin's placeholder
//!   + submit handler. The host owns the `Writable<String>` so query
//!   state outlives this component.
//! - [`search_results_list`] — scrollable list of pre-parsed hits.
//!   Each row shows the reference (`BOOK c:v`) + a snippet with bold
//!   runs marked where the FTS5 engine wrapped matches.
//!
//! Converting `db::SearchHit` to [`SearchHitView`] happens on the host
//! side (usually inside the signal reducer that fires DB searches),
//! not on each render.

use freya::prelude::*;

use crate::db::{SearchHit as DbSearchHit, snippet_markers};

use notein_ui_kit::theme::palette;

/// One rendered search hit — the UI-facing shape.
#[derive(Clone, Debug)]
pub struct SearchHitView {
    /// Human-readable location (`"Juan 3:16"`).
    pub reference: String,
    /// Alternating normal / highlighted runs. Each `bold = true` run
    /// paints in a brighter colour so the match pops in the list.
    pub snippet_runs: Vec<SnippetRun>,
    /// Flat single-line summary string used as the suggestion label
    /// in an inline input (toolbar `FormInput`). Formatted as
    /// `"<reference> — <snippet>"` with the snippet markers stripped
    /// so suggestions render as plain text.
    pub suggestion_label: String,
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
pub fn hit_to_view(hit: DbSearchHit, book_name: Option<&str>) -> SearchHitView {
    let (open, close) = snippet_markers();
    let runs = split_snippet(&hit.snippet, open, close);
    let book = book_name.unwrap_or(&hit.book_id);
    let reference = format!("{} {}:{}", book, hit.chapter, hit.verse);
    // Flatten the runs for the inline suggestion label — one plain
    // line, no markup, trimmed to a readable length.
    let flat: String = runs.iter().map(|r| r.text.as_str()).collect();
    let flat = flat.replace('\n', " ");
    let trimmed = if flat.chars().count() > 80 {
        let mut s = flat.chars().take(77).collect::<String>();
        s.push('…');
        s
    } else {
        flat
    };
    let suggestion_label = format!("{reference} — {trimmed}");
    SearchHitView {
        reference,
        snippet_runs: runs,
        suggestion_label,
        source: hit,
    }
}

fn split_snippet(snippet: &str, open: &str, close: &str) -> Vec<SnippetRun> {
    let mut out: Vec<SnippetRun> = Vec::new();
    let mut buf = String::new();
    let mut bold = false;
    let mut chars = snippet.chars().peekable();
    while let Some(c) = chars.next() {
        if open.starts_with(c) && std::iter::once(c).collect::<String>() == open {
            if !buf.is_empty() {
                out.push(SnippetRun { text: std::mem::take(&mut buf), bold });
            }
            bold = true;
            continue;
        }
        if close.starts_with(c) && std::iter::once(c).collect::<String>() == close {
            if !buf.is_empty() {
                out.push(SnippetRun { text: std::mem::take(&mut buf), bold });
            }
            bold = false;
            continue;
        }
        buf.push(c);
    }
    if !buf.is_empty() {
        out.push(SnippetRun { text: buf, bold });
    }
    out
}

/// Search text field. `on_submit` fires on Enter — the caller runs the
/// FTS5 query there and updates its hits signal.
pub fn search_input(
    value: Writable<String>,
    on_submit: impl Into<EventHandler<String>>,
) -> impl IntoElement {
    rect().width(Size::fill()).padding((8., 12.)).child(
        Input::new(value)
            .placeholder("Buscar en la Biblia…")
            .on_submit(on_submit),
    )
}

/// Scrollable list of hits. `on_pick(index)` fires when the user taps
/// a row; the caller resolves the pick against the hits slice.
pub fn search_results_list(
    hits: Vec<SearchHitView>,
    on_pick: impl Into<EventHandler<usize>> + 'static,
) -> impl IntoElement {
    let on_pick = on_pick.into();
    let mut list = rect()
        .width(Size::fill())
        .padding((4., 8.))
        .spacing(6.0);
    for (idx, view) in hits.into_iter().enumerate() {
        let on_pick = on_pick.clone();
        list = list.child(hit_row(view, move || on_pick.call(idx)));
    }
    ScrollView::new().child(list)
}

fn hit_row<F>(view: SearchHitView, mut on_press: F) -> impl IntoElement
where
    F: FnMut() + 'static,
{
    let mut snippet = paragraph()
        .width(Size::fill())
        .color(palette().t2)
        .font_size(14.0);
    for run in view.snippet_runs {
        let mut span = Span::new(run.text);
        if run.bold {
            span = span.color(palette().t1);
        }
        snippet = snippet.span(span);
    }
    rect()
        .width(Size::fill())
        .padding((10., 12.))
        .with_corner_radius(10.0)
        .background(palette().s2)
        .spacing(4.0)
        .on_press(move |_: Event<PressEventData>| on_press())
        .child(
            label()
                .font_size(12.0)
                .color(palette().t3)
                .text(view.reference),
        )
        .child(snippet)
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

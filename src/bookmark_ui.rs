//! Pin UI — long-press to create, floating card to view / edit.
//!
//! Phase 1.5 of `SOURCES_PLAN`. Scope limited to body-only editing;
//! rich `[[…]]` refs chip parsing + source anchors land in Phase 1.6+.
//!
//! The gesture layer that produces long-press events lives inside
//! [`crate::canvas::drawing_surface`] because it must sit on the same
//! pointer stack the viewport gestures already own — layering an
//! overlay rect for gestures would eat drawing input. The card
//! overlay here is a sibling of the drawing surface, rendered only
//! when a bookmark is selected.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use freya::icons::lucide::{bookmark, search};
use freya::prelude::*;
use istmo::plugins::EdgeInsets;

use crate::bookmark::{SourceRef, TimestampMs};
use crate::bookmark_body::Completion;
use crate::canvas::{Board, lock};
use crate::components::{BookmarkBodyEditor, FormInput};
use crate::ids::BookmarkId;
use crate::source_ref_nav;

/// Card width in logical pixels. Narrow enough to not swamp a mobile
/// landscape canvas; wide enough for a two-line body preview.
const CARD_WIDTH: f32 = 240.0;
/// Vertical gap from the pin to the card so the pin stays visible.
const CARD_GAP: f32 = 20.0;

/// Floating card overlay anchored to the selected bookmark. Reads +
/// writes bookmark body through the shared [`Board`]; closing clears
/// `selected`.
#[must_use]
pub fn bookmark_card_overlay(
    board: &Arc<Mutex<Board>>,
    mut selected: State<Option<BookmarkId>>,
    body_buffer: State<String>,
) -> impl IntoElement {
    let Some(selected_id) = *selected.read() else {
        // Nothing selected — render a zero-area placeholder so the
        // overlay slot in `app::root` always has a child regardless
        // of state.
        return rect().width(Size::px(0.0)).height(Size::px(0.0));
    };

    let (card_x, card_y, present, refs) = {
        let guard = lock(board);
        match guard.doc().bookmark(selected_id) {
            Some(bm) => {
                let world = guard.bookmark_world_position(bm);
                let (sx, sy) = guard.viewport().world_to_screen(world.x, world.y);
                (sx, sy, true, bm.refs.clone())
            }
            None => (0.0_f32, 0.0_f32, false, Vec::new()),
        }
    };
    if !present {
        // Selected bookmark vanished (deleted elsewhere, remote
        // tombstone, etc.) — close the card next tick.
        selected.set(None);
        return rect().width(Size::px(0.0)).height(Size::px(0.0));
    }

    let save_board = Arc::clone(board);
    let save_body = body_buffer;
    let mut save_selected = selected;
    let on_save = move |_: Event<PressEventData>| {
        let Some(id) = *save_selected.peek() else {
            return;
        };
        let body = save_body.peek().clone();
        let now = now_ms();
        lock(&save_board).update_bookmark(id, body, Vec::new(), None, now);
        save_selected.set(None);
    };

    let delete_board = Arc::clone(board);
    let mut delete_selected = selected;
    let on_delete = move |_: Event<PressEventData>| {
        let Some(id) = *delete_selected.peek() else {
            return;
        };
        lock(&delete_board).delete_bookmark(id);
        delete_selected.set(None);
    };

    let mut close_selected = selected;
    let on_close = move |_: Event<PressEventData>| {
        close_selected.set(None);
    };

    // Position the card below the pin, nudged right so the top-left
    // corner lands roughly on the pin baseline. Clamping to the
    // surface is left to Phase 1.6 — the current overlay stays
    // visible under any realistic zoom.
    let left = (card_x - CARD_WIDTH * 0.5).max(8.0);
    let top = card_y + CARD_GAP;

    rect()
        .position(Position::new_global().top(top).left(left))
        .layer(Layer::Overlay)
        .width(Size::px(CARD_WIDTH))
        .padding((12.0, 12.0))
        .spacing(10.0)
        .background(Color::from_rgb(28, 28, 32))
        .with_corner_radius(10.0)
        .child(
            BookmarkBodyEditor::new(body_buffer)
                .placeholder("Nota")
                .completions(bookmark_completions_provider(board, selected_id)),
        )
        .child(ref_chip_row(&refs))
        .child(
            rect()
                .direction(Direction::Horizontal)
                .main_align(Alignment::SpaceBetween)
                .cross_align(Alignment::Center)
                .width(Size::fill())
                .spacing(8.0)
                .child(
                    rect()
                        .padding((6.0, 10.0))
                        .background(Color::from_rgb(60, 60, 68))
                        .with_corner_radius(6.0)
                        .on_press(on_close)
                        .child("Cerrar"),
                )
                .child(
                    rect()
                        .padding((6.0, 10.0))
                        .background(Color::from_rgb(140, 32, 32))
                        .with_corner_radius(6.0)
                        .on_press(on_delete)
                        .child("Borrar"),
                )
                .child(
                    rect()
                        .padding((6.0, 10.0))
                        .background(Color::from_rgb(32, 120, 72))
                        .with_corner_radius(6.0)
                        .on_press(on_save)
                        .child("Guardar"),
                ),
        )
}

/// Build the completion provider the body editor consults while the
/// `[[` autocomplete popup is open. Phase 6.2 aggregates:
///
/// - Bookmarks on the current board (minus the one being edited).
/// - A URL shortcut when the query starts with `http://` or
///   `https://`.
/// - Plugin completions — every registered `Source` plugin is given
///   the query via [`notein_plugin::Source::plugin_completions`].
///
/// Called on every keystroke while the popup is open; registry
/// snapshot is taken under the mutex and dropped before any UI work.
fn bookmark_completions_provider(
    board: &Arc<Mutex<Board>>,
    editing: BookmarkId,
) -> impl Into<freya::prelude::Callback<String, Vec<Completion>>> {
    let board = Arc::clone(board);
    move |query: String| {
        let needle = query.to_lowercase();
        let guard = lock(&board);
        let mut out: Vec<Completion> = guard
            .doc()
            .bookmarks()
            .iter()
            .filter(|bm| bm.id != editing)
            .filter(|bm| {
                needle.is_empty() || bm.body.to_lowercase().contains(&needle)
            })
            .map(|bm| Completion {
                label: completion_label_for(bm),
                subtitle: Some(format!("Bookmark {}", short_uuid(bm.id))),
                source: SourceRef::Canvas { bookmark: bm.id },
            })
            .collect();
        drop(guard);

        // External url shortcut when the user types a well-formed url.
        if query.starts_with("http://") || query.starts_with("https://") {
            out.insert(
                0,
                Completion {
                    label: query.clone(),
                    subtitle: Some("Enlace externo".to_owned()),
                    source: SourceRef::External { url: query.clone() },
                },
            );
        }

        // Plugin-driven entries. Clone the Arcs out of the registry
        // under the lock so individual plugin calls run lock-free.
        let plugins: Vec<_> = {
            let reg = crate::plugins::registry();
            let reg_guard = crate::plugins::lock(&reg);
            reg_guard.sources().cloned().collect()
        };
        for plugin in plugins {
            let entries = plugin.plugin_completions(&query);
            out.extend(entries);
        }

        out
    }
}

fn completion_label_for(bm: &notein_plugin::Bookmark) -> String {
    let trimmed = bm.body.trim();
    if trimmed.is_empty() {
        format!("Bookmark {}", short_uuid(bm.id))
    } else {
        let one_line = trimmed.replace('\n', " ");
        truncate_body(&one_line)
    }
}

fn short_uuid(id: BookmarkId) -> String {
    let s = id.to_string();
    s.chars().take(8).collect()
}

/// Horizontal chip row showing one pill per [`SourceRef`] attached to
/// the bookmark. Tapping a chip calls [`source_ref_nav::navigate`] —
/// for PDF plugin anchors that scrolls the PDF face + fires a 500 ms
/// pulse highlight.
///
/// Returns a zero-area placeholder when the bookmark has no refs so
/// the surrounding card layout stays stable.
fn ref_chip_row(refs: &[SourceRef]) -> impl IntoElement {
    if refs.is_empty() {
        return rect().width(Size::px(0.0)).height(Size::px(0.0));
    }
    let mut row = rect()
        .direction(Direction::Horizontal)
        .spacing(6.0)
        .cross_align(Alignment::Center);
    for src in refs {
        row = row.child(ref_chip(src.clone()));
    }
    row
}

/// Single ref chip. Pill-shaped, tap → navigate.
fn ref_chip(src: SourceRef) -> impl IntoElement {
    let label_text = source_ref_nav::chip_label(&src);
    let on_tap = move |_: Event<PressEventData>| {
        source_ref_nav::navigate(&src);
    };
    rect()
        .padding((4.0, 8.0))
        .background(Color::from_rgb(44, 52, 70))
        .with_corner_radius(12.0)
        .on_press(on_tap)
        .child(
            label()
                .color(Color::from_rgb(200, 215, 240))
                .font_size(11.5)
                .text(label_text),
        )
}

/// Wall-clock ms since Unix epoch. Display-only — merge order is
/// decided by the op's lamport stamp, not by this value.
pub fn now_ms() -> TimestampMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Width of the slide-in sidebar in logical pixels. Matches
/// `SOURCES_PLAN` §7 (320 px desktop/tablet target).
const SIDEBAR_WIDTH: f32 = 320.0;
/// Height limit of the preview row content inside each entry. Clips
/// multi-line bodies so the list stays scannable; the full body shows
/// up in the card once the user taps.
const PREVIEW_MAX_LEN: usize = 80;

/// How the sidebar list is ordered. Phase 1.7 ships creation + recent;
/// position-y lands with the eventual multi-canvas layout work in
/// Phase 4+ where "y-axis of what" has an unambiguous answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkSort {
    /// Insertion order (oldest `created_at` first).
    Oldest,
    /// Newest `updated_at` first.
    Recent,
}

impl BookmarkSort {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Oldest => "Antiguos",
            Self::Recent => "Recientes",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Oldest => Self::Recent,
            Self::Recent => Self::Oldest,
        }
    }
}

/// Right-anchored overlay listing every bookmark on the current canvas.
/// Reads live from the board on each render so creation / deletion
/// flows don't need their own notification path — the card overlay's
/// mutations already fire `notify` which re-runs the whole app shell.
///
/// Clicking a row loads that bookmark into the card overlay (reuses
/// `selected_bookmark` + `body_buffer` state). Search filters by body
/// substring, case-insensitive.
#[must_use]
pub fn bookmark_sidebar(
    board: &Arc<Mutex<Board>>,
    mut selected: State<Option<BookmarkId>>,
    mut body_buffer: State<String>,
    query: State<String>,
    sort: State<BookmarkSort>,
    pad: EdgeInsets,
) -> impl IntoElement {
    let entries = {
        let guard = lock(board);
        let needle = query.read().trim().to_ascii_lowercase();
        let mut v: Vec<SidebarEntry> = guard
            .bookmarks()
            .iter()
            .filter(|bm| {
                if needle.is_empty() {
                    return true;
                }
                bm.body.to_ascii_lowercase().contains(&needle)
            })
            .map(|bm| SidebarEntry {
                id: bm.id,
                body_preview: truncate_body(&bm.body),
                refs_count: bm.refs.len(),
                color: bm.color,
                created_at: bm.created_at,
                updated_at: bm.updated_at,
            })
            .collect();
        match *sort.read() {
            BookmarkSort::Oldest => v.sort_by_key(|e| e.created_at),
            BookmarkSort::Recent => v.sort_by_key(|e| std::cmp::Reverse(e.updated_at)),
        }
        v
    };

    let empty_label = if query.read().trim().is_empty() {
        "Sin marcadores. Mantén presionado en el lienzo para crear uno."
    } else {
        "Nada coincide con la búsqueda."
    };

    let sort_label = sort.read().label();
    let mut sort_toggle = sort;
    let on_sort_toggle = move |_: Event<PressEventData>| {
        let next = sort_toggle.peek().next();
        sort_toggle.set(next);
    };

    let mut header = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .main_align(Alignment::SpaceBetween)
        .width(Size::fill())
        .padding((8.0, 10.0))
        .background(Color::from_rgb(22, 22, 26))
        .child(
            rect()
                .direction(Direction::Horizontal)
                .cross_align(Alignment::Center)
                .spacing(6.0)
                .child(
                    SvgViewer::new(bookmark())
                        .fill(Color::from_rgb(220, 220, 220))
                        .width(Size::px(14.))
                        .height(Size::px(14.)),
                )
                .child("Marcadores"),
        )
        .child(
            rect()
                .padding((4.0, 8.0))
                .background(Color::from_rgb(50, 50, 56))
                .with_corner_radius(6.0)
                .on_press(on_sort_toggle)
                .child(sort_label),
        );
    header = header.spacing(8.0);

    let search_row = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .width(Size::fill())
        .padding((6.0, 8.0))
        .spacing(6.0)
        .background(Color::from_rgb(24, 24, 28))
        .child(
            SvgViewer::new(search())
                .fill(Color::from_rgb(160, 160, 170))
                .width(Size::px(14.))
                .height(Size::px(14.)),
        )
        .child(FormInput::new(query).placeholder("Buscar"));

    let mut list = rect()
        .direction(Direction::Vertical)
        .width(Size::fill())
        .padding((8.0, 8.0))
        .spacing(6.0);

    if entries.is_empty() {
        list = list.child(rect().padding((12.0, 12.0)).child(empty_label));
    } else {
        for entry in entries {
            let row_board = Arc::clone(board);
            let row_id = entry.id;
            let on_pick = move |_: Event<PressEventData>| {
                let body = lock(&row_board)
                    .doc()
                    .bookmark(row_id)
                    .map(|bm| bm.body.clone())
                    .unwrap_or_default();
                body_buffer.set(body);
                selected.set(Some(row_id));
            };
            let dot_color = entry
                .color
                .map(|[r, g, b, _]| Color::from_rgb(r, g, b))
                .unwrap_or_else(|| Color::from_rgb(245, 158, 11));
            list = list.child(
                rect()
                    .direction(Direction::Horizontal)
                    .cross_align(Alignment::Center)
                    .spacing(8.0)
                    .padding((8.0, 10.0))
                    .background(Color::from_rgb(34, 34, 40))
                    .with_corner_radius(6.0)
                    .on_press(on_pick)
                    .child(
                        rect()
                            .width(Size::px(10.0))
                            .height(Size::px(10.0))
                            .with_corner_radius(5.0)
                            .background(dot_color),
                    )
                    .child(
                        rect()
                            .direction(Direction::Vertical)
                            .width(Size::fill())
                            .spacing(2.0)
                            .child(if entry.body_preview.is_empty() {
                                "(sin texto)".to_string()
                            } else {
                                entry.body_preview.clone()
                            })
                            .child(format!("{} ref(s)", entry.refs_count)),
                    ),
            );
        }
    }

    rect()
        .position(
            Position::new_absolute()
                .top(pad.top)
                .right(pad.right)
                .bottom(pad.bottom),
        )
        .layer(Layer::OverlayLevel(5))
        .width(Size::px(SIDEBAR_WIDTH))
        .height(Size::percent(100.))
        .background(Color::from_rgb(28, 28, 32))
        .with_corner_radius(10.0)
        .child(header)
        .child(search_row)
        .child(list)
}

/// Captured snapshot of a bookmark for list rendering. Taken under
/// a single `lock(&board)` so the list stays consistent even if a
/// concurrent op land between rows (not currently possible — the
/// board is single-threaded under this UI — but the pattern survives
/// Phase 10 sync).
struct SidebarEntry {
    id: BookmarkId,
    body_preview: String,
    refs_count: usize,
    color: Option<[u8; 4]>,
    created_at: TimestampMs,
    updated_at: TimestampMs,
}

fn truncate_body(body: &str) -> String {
    let flat: String = body.lines().collect::<Vec<_>>().join(" · ");
    if flat.chars().count() <= PREVIEW_MAX_LEN {
        return flat;
    }
    let mut out: String = flat.chars().take(PREVIEW_MAX_LEN).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_body_passes_through() {
        assert_eq!(truncate_body("hola"), "hola");
    }

    #[test]
    fn truncate_long_body_appends_ellipsis() {
        let input = "a".repeat(PREVIEW_MAX_LEN + 10);
        let out = truncate_body(&input);
        assert_eq!(out.chars().count(), PREVIEW_MAX_LEN + 1);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_flattens_multi_line_bodies() {
        let out = truncate_body("linea 1\nlinea 2\nlinea 3");
        assert_eq!(out, "linea 1 · linea 2 · linea 3");
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        // 10 chars of 2-byte UTF-8 — truncate must slice by char count,
        // not byte count, or the output panics mid-codepoint.
        let input = "á".repeat(PREVIEW_MAX_LEN + 5);
        let out = truncate_body(&input);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), PREVIEW_MAX_LEN + 1);
    }

    #[test]
    fn sort_next_round_trips() {
        assert_eq!(BookmarkSort::Oldest.next(), BookmarkSort::Recent);
        assert_eq!(BookmarkSort::Recent.next(), BookmarkSort::Oldest);
    }
}

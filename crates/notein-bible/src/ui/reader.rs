//! Freya chapter reader.
//!
//! Pure builders — the host embeds [`chapter_view`] inside a reactive
//! scope and feeds a loaded [`ChapterPlan`]. The reader itself does not
//! own state; chapter navigation and selection live on
//! [`crate::state::BibleViewerState`].

use std::collections::HashMap;

use freya::elements::paragraph::ParagraphCursorExt;
use freya::prelude::*;
use freya_engine::prelude::{RectHeightStyle, RectWidthStyle};

use crate::state::{BibleBookmark, u32_to_rgba};

use super::chapter::{Block, ChapterPlan, ParagraphSpan};
use super::theme;
use super::typography;

/// UI-side projection of a publisher footnote. `caller` is the display
/// letter (publisher-assigned — "a", "b", "c" or "+" for autogen).
/// `text` is the footnote body shown in the hover popup. `target`, when
/// present, lets the click handler navigate to the referenced verse.
#[derive(Clone, Debug, PartialEq)]
pub struct FootnoteView {
    pub src_verse: u16,
    pub caller: String,
    pub text: String,
    pub target: Option<(String, u16, u16)>,
}

#[must_use]
pub fn footnotes_by_verse(notes: &[FootnoteView]) -> HashMap<u16, Vec<FootnoteView>> {
    let mut out: HashMap<u16, Vec<FootnoteView>> = HashMap::new();
    for n in notes {
        out.entry(n.src_verse).or_default().push(n.clone());
    }
    out
}

/// UI-side projection of a cross-reference. The target text is
/// pre-fetched (`db.verse_text`) so the hover popup renders without
/// a DB round-trip inside the layout pass.
#[derive(Clone, Debug, PartialEq)]
pub struct CrossRefView {
    pub src_verse: u16,
    /// Short label shown as the inline chip (e.g. `"2 Co 4:6"`).
    pub label: String,
    /// Target verse text — shown inside the hover popup.
    pub tooltip: String,
    /// Target coordinates — the chip's `on_press` hands these back so
    /// the viewer navigates to the referenced verse.
    pub tgt_book: String,
    pub tgt_chap: u16,
    pub tgt_verse: u16,
}

/// Build a map keyed by source-verse so the paragraph renderer can look
/// up chips in O(1) per verse without walking the full list.
#[must_use]
pub fn cross_refs_by_verse(refs: &[CrossRefView]) -> HashMap<u16, Vec<CrossRefView>> {
    let mut out: HashMap<u16, Vec<CrossRefView>> = HashMap::new();
    for r in refs {
        out.entry(r.src_verse).or_default().push(r.clone());
    }
    out
}

/// Styling info derived from the active highlights, keyed by verse
/// number inside a single chapter. Precomputed so the paragraph builder
/// does one hash lookup per span instead of a linear scan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VerseStyle {
    /// Non-default text colour when the verse is highlighted. The
    /// absence of a stored colour on the bookmark falls back to the
    /// theme highlight yellow.
    pub tint: Option<Color>,
    /// True when the verse carries a non-empty note. Drives the inline
    /// note icon rendered right after the verse text.
    pub has_note: bool,
}

/// Build the per-verse style map from a highlights slice. The caller
/// passes `state.highlights_on(book, chapter)` for the chapter being
/// rendered.
#[must_use]
pub fn style_map(highlights: &[&BibleBookmark]) -> HashMap<u16, VerseStyle> {
    let mut out: HashMap<u16, VerseStyle> = HashMap::new();
    for h in highlights {
        let tint = h
            .color
            .map(|c| rgba_to_freya(u32_to_rgba(c)))
            .or_else(|| Some(parse_rgb(theme::HIGHLIGHT_YELLOW)));
        let has_note = !h.note.is_empty();
        for v in h.start_verse..=h.end_verse {
            let entry = out.entry(v).or_default();
            // Later highlights win on tint — matches a layered-notes
            // mental model (the newest note shows its colour).
            entry.tint = tint;
            entry.has_note |= has_note;
        }
    }
    out
}

fn rgba_to_freya(rgba: [u8; 4]) -> Color {
    Color::from_rgb(rgba[0], rgba[1], rgba[2])
}

/// Full chapter view — scrollable column of section headers +
/// paragraph groups.
///
/// `font_size` and `line_height` override the typography defaults so
/// the shell can honour the user's plugin config without re-building
/// this crate.
/// Callback the reader fires when the user clicks a cross-reference
/// chip. Carries `(book, chapter, verse)` so the viewer can
/// `jump_to` the referenced verse without re-decoding anchor bytes.
pub type OnJump = EventHandler<(String, u16, u16)>;

/// Chapter-scoped drag-selection cursor. Provided via `use_provide_context`
/// at the `chapter_view` root so every `VerseMarker` can co-operate on
/// a single multi-marker drag without the parent threading state as a
/// prop. `start` sticks on pointer-down over a marker; `end` tracks the
/// last marker the pointer entered while held. Global pointer-up reads
/// both and emits a `HighlightTarget::Range` when they differ.
#[derive(Clone, Copy)]
struct DragSelect {
    start: State<Option<u16>>,
    end: State<Option<u16>>,
    /// Set true on the first marker's `pointer_down` and cleared on
    /// global `pointer_up`. Markers mid-paragraph only extend `end`
    /// while this is true so a stray `pointer_enter` (hover without
    /// press) never grows the selection.
    pressing: State<bool>,
}

impl DragSelect {
    fn use_context() -> Self {
        use_provide_context(|| Self {
            start: State::create_in_scope(None, ScopeId::ROOT),
            end: State::create_in_scope(None, ScopeId::ROOT),
            pressing: State::create_in_scope(false, ScopeId::ROOT),
        })
    }
}

/// Which verse(s) a bookmark gesture targets. `Verse` fires for a
/// double-tap or long-press on a single verse; `Range` fires when the
/// user drags across multiple verses (inclusive on both ends).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightKind {
    Verse(u16),
    Range(u16, u16),
}

/// Full gesture payload. `anchor` is the global (window-relative)
/// pointer location where the gesture started, so the host can float
/// the colour picker next to the touch point instead of pinning it to
/// a fixed corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HighlightTarget {
    pub kind: HighlightKind,
    pub anchor_x: f32,
    pub anchor_y: f32,
}

/// Callback the reader fires when a bookmark gesture completes.
pub type OnHighlight = EventHandler<HighlightTarget>;

/// Live-preview highlight shown while the colour picker is open. The
/// shell forwards whatever colour the user is currently hovering /
/// dragging on the wheel; the reader overrides those verses' tint so
/// the paragraph updates in real time instead of only on commit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HighlightPreview {
    pub lo: u16,
    pub hi: u16,
    pub color: Color,
}

pub fn chapter_view(
    plan: &ChapterPlan,
    highlights: &[&BibleBookmark],
    cross_refs: &HashMap<u16, Vec<CrossRefView>>,
    footnotes: &HashMap<u16, Vec<FootnoteView>>,
    on_jump: OnJump,
    on_highlight: OnHighlight,
    flash_verse: Option<u16>,
    preview: Option<HighlightPreview>,
    font_size: f32,
    line_height: f32,
) -> impl IntoElement {
    // Install the chapter-scoped drag-select cursor so every verse
    // marker rendered under this view shares one `(start, end)` state
    // for the drag-highlight gesture.
    let _drag = DragSelect::use_context();
    let drop_cap_size = font_size * 4.5;
    let mut styles = style_map(highlights);
    if let Some(v) = flash_verse {
        // Transient wash — overrides any user highlight on the flash
        // target so the eye lands immediately.
        styles.insert(
            v,
            VerseStyle {
                tint: Some(Color::from_rgb(255, 225, 110)),
                has_note: styles.get(&v).map(|s| s.has_note).unwrap_or(false),
            },
        );
    }
    if let Some(prev) = preview {
        for v in prev.lo..=prev.hi {
            styles.insert(
                v,
                VerseStyle {
                    tint: Some(prev.color),
                    has_note: styles.get(&v).map(|s| s.has_note).unwrap_or(false),
                },
            );
        }
    }

    let _ = drop_cap_size;
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(parse_rgb(theme::BG))
        .padding((24., 28.))
        .child(
            ScrollView::new().child(
                rect()
                    .width(Size::fill())
                    .padding(16.)
                    .spacing(14.)
                    .child(book_title_header(plan))
                    .child(all_blocks(
                        plan,
                        &styles,
                        cross_refs,
                        footnotes,
                        on_jump,
                        on_highlight,
                        font_size,
                        line_height,
                        preview,
                    )),
            ),
        )
}

/// Render the full book name centered above the first chapter — doubles
/// as the top-of-document ornament the reader lands on. Hidden on
/// chapters > 1 so a swipe-nav doesn't repaint the heading mid-flow.
fn book_title_header(plan: &ChapterPlan) -> impl IntoElement {
    if plan.chapter == 1 {
        rect()
            .width(Size::fill())
            .padding((28., 0., 18., 0.))
            .cross_align(Alignment::Center)
            .child(
                label()
                    .font_family(typography::FAMILY)
                    .font_size(28.0)
                    .color(parse_rgb(theme::TEXT_PRIMARY))
                    .text(plan.book_name.to_uppercase()),
            )
            .into_element()
    } else {
        spacer(28.).into_element()
    }
}

/// Walk every block in reading order. The paragraph that owns verse 1
/// gets its "1" slot replaced by the big chapter number — the rest
/// render as regular verse-number superscripts.
fn all_blocks(
    plan: &ChapterPlan,
    styles: &HashMap<u16, VerseStyle>,
    cross_refs: &HashMap<u16, Vec<CrossRefView>>,
    footnotes: &HashMap<u16, Vec<FootnoteView>>,
    on_jump: OnJump,
    on_highlight: OnHighlight,
    font_size: f32,
    line_height: f32,
    preview: Option<HighlightPreview>,
) -> impl IntoElement {
    let mut root = rect().width(Size::fill()).spacing(14.);
    for block in &plan.blocks {
        match block {
            Block::Paragraph(spans) => {
                let starts_chapter = spans.first().is_some_and(|s| s.verse == 1);
                root = root.child(verse_paragraph(
                    spans,
                    styles,
                    cross_refs,
                    footnotes,
                    on_jump.clone(),
                    on_highlight.clone(),
                    font_size,
                    line_height,
                    if starts_chapter { Some(plan.chapter) } else { None },
                    preview,
                ));
            }
            Block::Header(text) => {
                root = root.child(section_header(text));
            }
        }
    }
    root
}

/// A single paragraph with inline superscript verse numbers.
/// Highlighted verses get their text repainted in the stored tint and
/// gain an inline note icon when the bookmark has a non-empty body.
/// Cross-reference chips render as blue clickable widgets right after
/// each verse that has at least one ref.
///
/// `starts_chapter` carries the chapter number when this paragraph
/// contains verse 1 — the "1" slot is then rendered as the chapter
/// number at 2.5× body font (drop-cap style, inline).
/// Paragraph wrapper that owns the bookmark gestures. Taps and drags
/// hit the whole paragraph (not the tiny verse-number marker), map
/// element-local coordinates to a UTF-16 offset via `ParagraphHolder::
/// cursor_index_at_point`, and resolve the offset to a verse number
/// via the per-verse span ranges computed at build time. Visual-only
/// inline children (`VerseMarker`, `FootnoteLetter`, `RefHoverButton`)
/// still render as before; they no longer carry gesture handlers.
#[derive(Clone, PartialEq)]
pub(super) struct VerseParagraph {
    pub(super) spans: Vec<ParagraphSpan>,
    pub(super) styles: HashMap<u16, VerseStyle>,
    pub(super) cross_refs: HashMap<u16, Vec<CrossRefView>>,
    pub(super) footnotes: HashMap<u16, Vec<FootnoteView>>,
    pub(super) on_jump: OnJump,
    pub(super) on_highlight: OnHighlight,
    pub(super) font_size: f32,
    pub(super) line_height: f32,
    pub(super) starts_chapter: Option<u16>,
    pub(super) preview: Option<HighlightPreview>,
}

impl std::fmt::Debug for VerseParagraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerseParagraph")
            .field("spans", &self.spans.len())
            .finish()
    }
}

/// `(start_utf16, end_utf16, verse_number)` so a hit-tested offset can
/// be mapped back to the owning verse in `O(n)` for `n` = verses in the
/// paragraph (typically ≤ 10, so no indexing structure needed).
type VerseRange = (usize, usize, u16);

fn offset_to_verse(ranges: &[VerseRange], offset: usize) -> Option<u16> {
    ranges
        .iter()
        .find(|(start, end, _)| offset >= *start && offset < *end)
        .map(|(_, _, v)| *v)
}

/// Resolve `element_location` to the UTF-16 offset inside the laid-out
/// paragraph the holder references. Returns `None` before the first
/// frame has published the paragraph (holder still empty).
fn hit_offset(holder: &ParagraphHolder, elem: CursorPoint) -> Option<usize> {
    let inner = holder.0.borrow();
    let data = inner.as_ref()?;
    let vis = holder.visible_location(elem);
    Some(data.paragraph.cursor_index_at_point((vis.x, vis.y)))
}

impl Component for VerseParagraph {
    fn render(&self) -> impl IntoElement {
        let holder = use_hook(ParagraphHolder::default);
        let drag = use_consume::<DragSelect>();
        // Monotonic token so a pointer-up / pointer-leave that arrives
        // after the long-press timer has already fired is a no-op, and
        // so a second press starts a fresh timer.
        let long_press_token = use_state(|| 0u64);
        // Global pointer location captured on `pointer_down`. The
        // host anchors its colour picker here so the popup opens next
        // to the finger / cursor instead of pinned to a fixed corner.
        let anchor: State<(f32, f32)> = use_state(|| (0.0, 0.0));
        // Paragraph area captured via `on_sized`. Flips 0 → real_size
        // after first layout; subscribing to it forces a re-render once
        // the holder has been populated, so the second-pass bg-rect
        // compute reads a laid-out paragraph.
        let mut area: State<Area> = use_state(Area::default);
        // Belt-and-suspenders tick that bumps a few times right after
        // mount, so the bg-rect compute re-runs even if `on_sized`
        // misses (first layout before the handler was attached, size
        // unchanged across frames, etc.). Cheap: at most three extra
        // renders in the first ~150ms.
        let tick: State<u32> = use_state(|| 0);
        use_hook({
            let mut tick = tick;
            move || {
                spawn(async move {
                    for i in 1u32..=3 {
                        async_io::Timer::after(std::time::Duration::from_millis(40)).await;
                        tick.set(i);
                    }
                });
            }
        });
        let _ = *tick.read();

        let on_jump = self.on_jump.clone();
        let on_highlight = self.on_highlight.clone();
        let font_size = self.font_size;
        let line_height = self.line_height;

        let mut p = paragraph()
            .width(Size::fill())
            .line_height(line_height)
            .color(parse_rgb(theme::TEXT_PRIMARY))
            .font_family(typography::FAMILY)
            .font_size(font_size)
            .holder(holder.clone())
            .on_sized(move |e: Event<SizedEventData>| {
                let new_area = e.data().area;
                if *area.peek() != new_area {
                    area.set(new_area);
                }
            });

        // UTF-16 offsets drive both pointer hit-testing and the
        // per-verse bg rects we paint behind the text. Each `.child()`
        // adds one placeholder unit; each span contributes
        // `text.encode_utf16().count()` units.
        let mut utf16_offset: usize = 0;
        let mut verse_ranges: Vec<VerseRange> = Vec::new();
        // Per-verse highlight items — one entry per bookmark /
        // preview target with its own colour, so we can paint
        // per-verse bg rects via `holder.get_rects_for_range` after
        // layout. Preserves stored colours (the single-colour
        // `paragraph.highlights` paint path couldn't).
        let mut highlight_items: Vec<(usize, usize, Color)> = Vec::new();

        for span in &self.spans {
            let style = self.styles.get(&span.verse);
            let preview_color = self
                .preview
                .as_ref()
                .filter(|p| span.verse >= p.lo && span.verse <= p.hi)
                .map(|p| p.color);

            p = p.child(VerseMarker {
                verse: span.verse,
                display: if span.verse == 1 && self.starts_chapter.is_some() {
                    MarkerDisplay::Chapter(self.starts_chapter.unwrap())
                } else {
                    MarkerDisplay::Verse
                },
                font_size,
            });
            utf16_offset += 1; // placeholder for the marker child

            let text = format!("{} ", span.text);
            let text_units = text.encode_utf16().count();
            let start = utf16_offset;
            p = p.span(Span::new(text).font_size(font_size));
            utf16_offset += text_units;
            let end = utf16_offset;

            verse_ranges.push((start, end, span.verse));
            // Preview wins over the stored tint for the targeted verse
            // so the live wheel feedback stays on top; non-targeted
            // bookmarks keep their own colour.
            let paint_color = preview_color.or_else(|| style.and_then(|s| s.tint));
            if let Some(c) = paint_color {
                highlight_items.push((start, end, c));
            }

            if style.map(|s| s.has_note).unwrap_or(false) {
                p = p.child(note_icon(font_size));
                utf16_offset += 1;
            }
            if let Some(notes) = self.footnotes.get(&span.verse) {
                for (idx, note) in notes.iter().enumerate() {
                    p = p.child(FootnoteLetter {
                        letter: display_letter(&note.caller, idx),
                        text: note.text.clone(),
                        target: note.target.clone(),
                        jump: on_jump.clone(),
                        font_size,
                    });
                    utf16_offset += 1;
                }
            }
            if let Some(refs) = self.cross_refs.get(&span.verse) {
                p = p.child(RefHoverButton {
                    refs: refs.clone(),
                    jump: on_jump.clone(),
                    font_size,
                });
                utf16_offset += 1;
            }
        }

        // Subscribe to the area signal so the second pass runs after
        // the paragraph publishes its laid-out form into the holder.
        let paragraph_area = *area.read();
        let bg_rects = build_bg_rects(&holder, &highlight_items, paragraph_area);

        // Gesture wiring: paragraph-level so a tap anywhere in verse
        // text — not just the tiny superscript marker — fires.
        let holder_for_down = holder.clone();
        let ranges_for_down = verse_ranges.clone();
        let holder_for_move = holder.clone();
        let ranges_for_move = verse_ranges.clone();

        let p = p.on_pointer_down({
            let mut long_press_token = long_press_token;
            let on_highlight = on_highlight.clone();
            let mut drag = drag;
            let mut anchor = anchor;
            move |e: Event<PointerEventData>| {
                let Some(offset) = hit_offset(&holder_for_down, e.element_location()) else {
                    return;
                };
                let Some(verse) = offset_to_verse(&ranges_for_down, offset) else {
                    return;
                };

                let g = e.global_location();
                let anchor_pt = (g.x as f32, g.y as f32);
                anchor.set(anchor_pt);

                drag.start.set(Some(verse));
                drag.end.set(Some(verse));
                drag.pressing.set(true);

                let my_token = long_press_token.peek().wrapping_add(1);
                long_press_token.set(my_token);

                if EventsCombos::<()>::pressed(e.global_location()).is_double() {
                    on_highlight.call(HighlightTarget {
                        kind: HighlightKind::Verse(verse),
                        anchor_x: anchor_pt.0,
                        anchor_y: anchor_pt.1,
                    });
                    drag.start.set(None);
                    drag.end.set(None);
                    drag.pressing.set(false);
                    return;
                }

                let on_highlight = on_highlight.clone();
                let long_press_token = long_press_token;
                let mut drag_for_long = drag;
                spawn(async move {
                    async_io::Timer::after(std::time::Duration::from_millis(LONG_PRESS_MS))
                        .await;
                    if *long_press_token.peek() == my_token {
                        on_highlight.call(HighlightTarget {
                            kind: HighlightKind::Verse(verse),
                            anchor_x: anchor_pt.0,
                            anchor_y: anchor_pt.1,
                        });
                        drag_for_long.start.set(None);
                        drag_for_long.end.set(None);
                        drag_for_long.pressing.set(false);
                    }
                });
            }
        })
        .on_pointer_move({
            let mut drag = drag;
            let mut long_press_token = long_press_token;
            move |e: Event<PointerEventData>| {
                if !*drag.pressing.peek() {
                    return;
                }
                let Some(offset) = hit_offset(&holder_for_move, e.element_location()) else {
                    return;
                };
                let Some(verse) = offset_to_verse(&ranges_for_move, offset) else {
                    return;
                };
                let current = *drag.end.peek();
                if current != Some(verse) {
                    drag.end.set(Some(verse));
                    // Dragging across verses cancels the long-press
                    // timer so the user does not get both a Verse +
                    // Range emission from one gesture.
                    let next = long_press_token.peek().wrapping_add(1);
                    long_press_token.set(next);
                }
            }
        })
        .on_global_pointer_up({
            let mut long_press_token = long_press_token;
            let on_highlight = on_highlight.clone();
            let mut drag = drag;
            let anchor = anchor;
            move |_: Event<PointerEventData>| {
                let next = long_press_token.peek().wrapping_add(1);
                long_press_token.set(next);

                let start = *drag.start.peek();
                let end = *drag.end.peek();
                drag.start.set(None);
                drag.end.set(None);
                drag.pressing.set(false);
                EventsCombos::<()>::released();

                if let (Some(a), Some(b)) = (start, end)
                    && a != b
                {
                    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                    let (ax, ay) = *anchor.peek();
                    on_highlight.call(HighlightTarget {
                        kind: HighlightKind::Range(lo, hi),
                        anchor_x: ax,
                        anchor_y: ay,
                    });
                }
            }
        });

        // Wrap: bg rects first (painted behind) + paragraph with
        // gestures second. Both are children of a `rect` whose own
        // size is driven by the paragraph (bg rects are absolute, so
        // they do not contribute to layout).
        rect()
            .width(Size::fill())
            .child(bg_rects)
            .child(p)
    }
}

/// Build the per-verse background-rect overlay. Reads the laid-out
/// paragraph from `holder` and emits one absolute-positioned coloured
/// rect per text box the paragraph returned for each highlight range.
/// Multi-line verses produce one box per visual line, so the paint
/// hugs the line heights naturally.
///
/// Before the first layout publishes into the holder (`area` still at
/// its default), returns an empty rect. The `on_sized` callback on the
/// paragraph flips `area` after layout, which re-runs render and gives
/// this function a populated holder on the second pass.
fn build_bg_rects(
    holder: &ParagraphHolder,
    items: &[(usize, usize, Color)],
    paragraph_area: Area,
) -> impl IntoElement {
    // `Layer::Relative(-1)` pushes every bg rect one step behind the
    // paragraph sibling in the paint order. Hit-testing then prefers
    // the paragraph on top, so the gestures keep firing even over a
    // painted highlight.
    let mut container = rect()
        .layer(Layer::Relative(-1))
        .position(Position::new_absolute().top(0.).left(0.))
        .width(Size::fill())
        .height(Size::fill());
    // Skip empty items so unhighlighted paragraphs do not allocate.
    // Keep the area read for its subscription side-effect so a layout
    // change schedules a re-render even when the holder data hasn't.
    let _ = paragraph_area;
    if items.is_empty() {
        return container;
    }
    let inner = holder.0.borrow();
    let Some(data) = inner.as_ref() else {
        return container;
    };
    let scale = data.scale_factor.max(1e-3);
    for (start, end, color) in items {
        if start == end {
            continue;
        }
        let boxes = data.paragraph.get_rects_for_range(
            *start..*end,
            RectHeightStyle::Tight,
            RectWidthStyle::Tight,
        );
        for tb in boxes {
            let left = (tb.rect.left as f64 / scale) as f32;
            let top = (tb.rect.top as f64 / scale) as f32;
            let width = ((tb.rect.right - tb.rect.left) as f64 / scale) as f32;
            let height = ((tb.rect.bottom - tb.rect.top) as f64 / scale) as f32;
            if width <= 0.0 || height <= 0.0 {
                continue;
            }
            container = container.child(
                rect()
                    .position(Position::new_absolute().top(top).left(left))
                    .width(Size::px(width))
                    .height(Size::px(height))
                    .background(*color),
            );
        }
    }
    container
}

/// Legacy entry point. Preserved so `all_blocks` doesn't need to care
/// about construction details; just forwards to the component.
fn verse_paragraph(
    spans: &[ParagraphSpan],
    styles: &HashMap<u16, VerseStyle>,
    cross_refs: &HashMap<u16, Vec<CrossRefView>>,
    footnotes: &HashMap<u16, Vec<FootnoteView>>,
    on_jump: OnJump,
    on_highlight: OnHighlight,
    font_size: f32,
    line_height: f32,
    starts_chapter: Option<u16>,
    preview: Option<HighlightPreview>,
) -> impl IntoElement {
    VerseParagraph {
        spans: spans.to_vec(),
        styles: styles.clone(),
        cross_refs: cross_refs.clone(),
        footnotes: footnotes.clone(),
        on_jump,
        on_highlight,
        font_size,
        line_height,
        starts_chapter,
        preview,
    }
}

/// What the clickable verse marker should display — the superscript
/// verse number OR the big chapter number (when the marker opens a
/// chapter + sits on verse 1).
#[derive(Clone, Copy, PartialEq, Debug)]
enum MarkerDisplay {
    Verse,
    Chapter(u16),
}

/// Visual-only verse marker. Gestures live on the enclosing
/// `VerseParagraph` so taps hit the whole verse text, not just the
/// tiny superscript number.
#[derive(Clone, PartialEq)]
struct VerseMarker {
    verse: u16,
    display: MarkerDisplay,
    font_size: f32,
}

impl std::fmt::Debug for VerseMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerseMarker")
            .field("verse", &self.verse)
            .finish()
    }
}

/// Delay before a sustained press is treated as a long-press.
const LONG_PRESS_MS: u64 = 500;

impl Component for VerseMarker {
    fn render(&self) -> impl IntoElement {
        let verse = self.verse;
        let inner = match self.display {
            MarkerDisplay::Chapter(chap) => label()
                .font_family(typography::FAMILY)
                .font_size(self.font_size * 2.5)
                .color(parse_rgb(theme::DROP_CAP))
                .text(format!("{chap} ")),
            MarkerDisplay::Verse => label()
                .font_size(self.font_size * 0.6)
                .color(parse_rgb(theme::VERSE_NUM))
                .text(format!(" {verse} ")),
        };
        rect().padding((0., 2.)).child(inner)
    }
}

/// Normalise the publisher-supplied caller glyph. Many translations
/// ship `"+"` as an autogen placeholder — fall back to positional
/// letters a/b/c in that case so the UI stays readable.
fn display_letter(caller: &str, idx: usize) -> String {
    let trimmed = caller.trim();
    if trimmed.is_empty() || trimmed == "+" || trimmed == "*" {
        if idx < 26 {
            char::from(b'a' + idx as u8).to_string()
        } else {
            format!("{idx}")
        }
    } else {
        trimmed.to_string()
    }
}

/// Publisher footnote letter inline. Hover shows the full note text;
/// click navigates to the target verse when the footnote carries one
/// (parsed `Cited in Book Chap:Verse` style refs).
#[derive(Clone, PartialEq)]
struct FootnoteLetter {
    letter: String,
    text: String,
    target: Option<(String, u16, u16)>,
    jump: OnJump,
    font_size: f32,
}

impl std::fmt::Debug for FootnoteLetter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FootnoteLetter")
            .field("letter", &self.letter)
            .finish()
    }
}

impl Component for FootnoteLetter {
    fn render(&self) -> impl IntoElement {
        let hover = use_state(|| false);
        let version = use_state(|| 0u64);
        let letter = self.letter.clone();
        let text = self.text.clone();
        let target = self.target.clone();
        let on_jump = self.jump.clone();
        let marker_font = self.font_size * 0.55;
        let blue = Color::from_rgb(80, 150, 230);

        let mut container = rect()
            .padding((0., 1.))
            .margin((0., 1.))
            .on_pointer_enter({
                let mut hover = hover;
                let mut version = version;
                move |_| {
                    hover.set(true);
                    let next = version.peek().wrapping_add(1);
                    version.set(next);
                }
            })
            .on_pointer_leave({
                let mut hover = hover;
                let mut version = version;
                move |_| close_soon(&mut hover, &mut version)
            })
            .on_press({
                let mut hover = hover;
                let mut version = version;
                move |_: Event<PressEventData>| {
                    hover.set(false);
                    let next = version.peek().wrapping_add(1);
                    version.set(next);
                    if let Some(t) = target.clone() {
                        on_jump.call(t);
                    }
                }
            })
            .child(label().font_size(marker_font).color(blue).text(letter));

        if *hover.read() && !text.is_empty() {
            container = container.child(
                rect()
                    .layer(Layer::Overlay)
                    .position(Position::new_absolute().top(marker_font * 1.6).left(0.))
                    .width(Size::px(300.0))
                    .padding((10., 12.))
                    .with_corner_radius(8.0)
                    .background(parse_rgb(theme::CHIP_BG))
                    .on_pointer_enter({
                        let mut hover = hover;
                        let mut version = version;
                        move |_| {
                            hover.set(true);
                            let next = version.peek().wrapping_add(1);
                            version.set(next);
                        }
                    })
                    .on_pointer_leave({
                        let mut hover = hover;
                        let mut version = version;
                        move |_| close_soon(&mut hover, &mut version)
                    })
                    .child(
                        label()
                            .font_size(self.font_size * 0.65)
                            .color(parse_rgb(theme::TEXT_PRIMARY))
                            .text(text),
                    ),
            );
        }
        container
    }
}

/// Schedule a hover close that only fires if no `enter` event bumps
/// `version` in the meantime. Shared by every hover-popup widget so
/// the "move cursor from icon to popup body" gesture is reliable.
fn close_soon(hover: &mut State<bool>, version: &mut State<u64>) {
    let my_token = version.peek().wrapping_add(1);
    version.set(my_token);
    let mut hover = *hover;
    let version = *version;
    spawn(async move {
        async_io::Timer::after(std::time::Duration::from_millis(350)).await;
        if *version.peek() == my_token {
            hover.set(false);
        }
    });
}

/// Inline bookmark icon rendered when a verse has at least one
/// cross-ref. Hovering opens a floating list of collapsible refs;
/// each row carries a "Ir a" button that drives `on_jump`. The icon
/// stays single + unobtrusive so the paragraph flow is not broken by
/// per-letter superscripts.
#[derive(Clone, PartialEq)]
struct RefHoverButton {
    refs: Vec<CrossRefView>,
    jump: OnJump,
    font_size: f32,
}

impl std::fmt::Debug for RefHoverButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefHoverButton")
            .field("ref_count", &self.refs.len())
            .finish()
    }
}

impl Component for RefHoverButton {
    fn render(&self) -> impl IntoElement {
        let hover = use_state(|| false);
        let version = use_state(|| 0u64);
        let expanded = use_state(|| std::collections::HashSet::<usize>::new());

        let icon_size = self.font_size * 0.55;
        let blue = Color::from_rgb(100, 150, 230);
        let refs_for_list = self.refs.clone();
        let jump_for_list = self.jump.clone();
        let font_size = self.font_size;

        let mut container = rect()
            .padding((0., 2.))
            .margin((0., 2.))
            .cross_align(Alignment::Start)
            .main_align(Alignment::Start)
            .on_pointer_enter({
                let mut hover = hover;
                let mut version = version;
                move |_| {
                    hover.set(true);
                    let next = version.peek().wrapping_add(1);
                    version.set(next);
                }
            })
            .on_pointer_leave({
                let mut hover = hover;
                let mut version = version;
                move |_| close_soon(&mut hover, &mut version)
            })
            .child(
                SvgViewer::new(freya::icons::lucide::trending_up_down())
                    .color(blue)
                    .width(Size::px(icon_size))
                    .height(Size::px(icon_size)),
            );

        if *hover.read() {
            let popup = rect()
                .layer(Layer::Overlay)
                .position(Position::new_absolute().top(icon_size * 1.4).left(0.))
                .width(Size::px(320.0))
                .max_height(Size::px(360.0))
                .padding((6., 8.))
                .with_corner_radius(10.0)
                .background(parse_rgb(theme::CHIP_BG))
                .spacing(4.0)
                .on_pointer_enter({
                    let mut hover = hover;
                    let mut version = version;
                    move |_| {
                        hover.set(true);
                        let next = version.peek().wrapping_add(1);
                        version.set(next);
                    }
                })
                .on_pointer_leave({
                    let mut hover = hover;
                    let mut version = version;
                    move |_| close_soon(&mut hover, &mut version)
                })
                .child(ScrollView::new().child({
                    let mut col = rect().width(Size::fill()).spacing(4.0);
                    for (idx, xref) in refs_for_list.iter().enumerate() {
                        col = col.child(RefListRow {
                            idx,
                            view: xref.clone(),
                            jump: jump_for_list.clone(),
                            expanded,
                            parent_hover: hover,
                            parent_version: version,
                            font_size,
                        });
                    }
                    col
                }));
            container = container.child(popup);
        }
        container
    }
}

/// One collapsible row inside the hover popup. Clicking anywhere on
/// the header expands / collapses; the "Ir a" button navigates
/// unconditionally.
#[derive(Clone, PartialEq)]
struct RefListRow {
    idx: usize,
    view: CrossRefView,
    jump: OnJump,
    expanded: State<std::collections::HashSet<usize>>,
    parent_hover: State<bool>,
    parent_version: State<u64>,
    font_size: f32,
}

impl std::fmt::Debug for RefListRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefListRow")
            .field("ref", &self.view.label)
            .finish()
    }
}

impl Component for RefListRow {
    fn render(&self) -> impl IntoElement {
        let is_open = self.expanded.read().contains(&self.idx);
        let idx = self.idx;
        let mut expanded = self.expanded;
        let on_header = move |_: Event<PressEventData>| {
            let mut set = expanded.write();
            if set.contains(&idx) {
                set.remove(&idx);
            } else {
                set.insert(idx);
            }
        };
        let target = (
            self.view.tgt_book.clone(),
            self.view.tgt_chap,
            self.view.tgt_verse,
        );
        let on_jump = self.jump.clone();
        let mut parent_hover = self.parent_hover;
        let mut parent_version = self.parent_version;
        let goto = move |_: Event<PressEventData>| {
            parent_hover.set(false);
            let next = parent_version.peek().wrapping_add(1);
            parent_version.set(next);
            on_jump.call(target.clone());
        };

        let header = rect()
            .width(Size::fill())
            .padding((6., 10.))
            .direction(Direction::Horizontal)
            .cross_align(Alignment::Center)
            .main_align(Alignment::SpaceBetween)
            .with_corner_radius(6.0)
            .background(parse_rgb(theme::BG))
            .on_press(on_header)
            .child(
                label()
                    .font_size(self.font_size * 0.65)
                    .color(Color::from_rgb(100, 150, 230))
                    .text(self.view.label.clone()),
            )
            .child(
                rect()
                    .padding((4., 10.))
                    .with_corner_radius(5.0)
                    .background(Color::from_rgb(55, 80, 140))
                    .on_press(goto)
                    .child(
                        label()
                            .font_size(self.font_size * 0.58)
                            .color(Color::from_rgb(240, 240, 240))
                            .text("Ir a"),
                    ),
            );

        let mut body = rect().width(Size::fill()).spacing(2.0).child(header);
        if is_open && !self.view.tooltip.is_empty() {
            body = body.child(
                rect().width(Size::fill()).padding((8., 10.)).child(
                    label()
                        .font_size(self.font_size * 0.65)
                        .color(parse_rgb(theme::TEXT_SECONDARY))
                        .text(self.view.tooltip.clone()),
                ),
            );
        }
        body
    }
}

fn note_icon(font_size: f32) -> impl IntoElement {
    let size = font_size * 0.75;
    rect()
        .width(Size::px(size + 6.0))
        .height(Size::px(size + 2.0))
        .center()
        .child(
            SvgViewer::new(freya::icons::lucide::file_text())
                .color(parse_rgb(theme::VERSE_NUM))
                .width(Size::px(size))
                .height(Size::px(size)),
        )
}

fn section_header(text: &str) -> impl IntoElement {
    rect().width(Size::fill()).padding((12., 0., 4., 0.)).child(
        label()
            .font_family(typography::FAMILY)
            .font_size(typography::SECTION_HEADER_PX)
            .color(parse_rgb(theme::HEADER))
            .text(text.to_string()),
    )
}

fn spacer(height: f32) -> impl IntoElement {
    rect().width(Size::fill()).height(Size::px(height))
}

/// Parse an `"rgb(r, g, b)"` string into a Freya `Color`. The theme
/// module keeps the strings for parity with the inline markup used in
/// other Freya components in this project.
fn parse_rgb(s: &str) -> Color {
    let trimmed = s.trim_start_matches("rgb(").trim_end_matches(')');
    let parts: Vec<u8> = trimmed
        .split(',')
        .filter_map(|t| t.trim().parse::<u8>().ok())
        .collect();
    if parts.len() == 3 {
        Color::from_rgb(parts[0], parts[1], parts[2])
    } else {
        Color::WHITE
    }
}

//! Freya chapter reader.
//!
//! Pure builders — the host embeds [`chapter_view`] inside a reactive
//! scope and feeds a loaded [`ChapterPlan`]. The reader itself does not
//! own state; chapter navigation and selection live on
//! [`crate::state::BibleViewerState`].

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use freya::components::ScrollController;
use freya::elements::paragraph::ParagraphCursorExt;
use freya::prelude::*;
use freya_engine::prelude::{Paragraph as SkParagraph, RectHeightStyle, RectWidthStyle};

use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;

use crate::state::{BibleBookmark, u32_to_rgba};

use super::chapter::{Block, ChapterPlan, ParagraphSpan};
use super::typography;

/// Context the host installs so verse-level widgets can scroll the
/// chapter viewport into focus when a chip-tap or TSK jump sets
/// `flash_verse`.
///
/// `viewport_top` is published by `chapter_view`'s outer rect `on_sized` and
/// read inside `VerseMarker::on_sized` so scroll targets translate verse
/// window-coords into content-coords without pulling in a shared layout crate.
///
/// Vertical margin applied so the target verse sits a comfortable
/// distance below the chapter header + translation strip.
pub const VERSE_SCROLL_MARGIN: f32 = 96.0;

/// How long a jump keeps realigning its verse while the chapter settles.
const SCROLL_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);

/// A pending jump to `verse`.
///
/// The verse marker aligns the viewport on it as the chapter lays out, then
/// drops the target: once aligned, or [`SCROLL_SETTLE`] after the first
/// attempt, so scrolling afterwards is the user's again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollTarget {
    verse: u16,
    since: Option<std::time::Instant>,
}

impl ScrollTarget {
    #[must_use]
    pub const fn new(verse: u16) -> Self {
        Self { verse, since: None }
    }
}

#[derive(Clone)]
pub struct VerseScrollCtx {
    pub controller: ScrollController,
    pub flash_verse: State<Option<u16>>,
    pub scroll_to: State<Option<ScrollTarget>>,
    pub viewport_top: State<f32>,
}

impl PartialEq for VerseScrollCtx {
    fn eq(&self, _other: &Self) -> bool {
        // Shared signal handles — the inner `State`s drive the diff.
        true
    }
}

impl std::fmt::Debug for VerseScrollCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerseScrollCtx").finish_non_exhaustive()
    }
}

/// UI-side projection of a publisher footnote. `caller` is the display
/// letter (publisher-assigned — "a", "b", "c" or "+" for autogen).
///
/// `text` is the footnote body shown in the hover popup. `target`, when
/// present, lets the click handler navigate to the referenced verse.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// UI-side projection of a cross-reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossRefView {
    pub src_verse: u16,
    /// Short label of the target (e.g. `"2 Co 4:6"`).
    pub label: String,
    /// Target coordinates, where pressing the reference goes.
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
            .or_else(|| Some(palette().highlighters[0]));
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

const fn rgba_to_freya(rgba: [u8; 4]) -> Color {
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

/// Callback fired by a verse's cross-reference icon, with the verse.
pub type OnRefs = EventHandler<u16>;

/// Hold time before a still press on a verse selects it.
const LONG_PRESS: Duration = Duration::from_millis(450);
/// Most time between the two taps of a double tap.
const DOUBLE_TAP: Duration = Duration::from_millis(350);
/// Travel, in logical pixels, past which a hold is a scroll instead.
const SLOP_PX: f32 = 8.0;

/// Chapter-scoped gesture state, shared by every paragraph of the
/// chapter through context.
///
/// - A mouse drag across verses: `start` sticks on pointer-down, `end`
///   follows the verse under the pointer, and the lift opens the menu
///   for the range. A finger drag scrolls instead, so it never grows
///   the range.
/// - A double tap on a verse opens the menu for it.
/// - A long press selects the verse at once (it shows underlined) and
///   opens the menu on the lift, so the lifting finger cannot land on
///   the menu that just appeared under it.
#[derive(Clone, Copy)]
struct DragSelect {
    start: State<Option<u16>>,
    end: State<Option<u16>>,
    /// Set on a paragraph's pointer-down and cleared on the global
    /// pointer-up. Paragraphs only extend `end` while it is set, so
    /// hovering without a press never grows the range.
    pressing: State<bool>,
    /// The previous tap, to tell a double tap.
    last_tap: State<Option<(Instant, u16)>>,
    /// Bumped by every press; a long press only fires while it holds.
    hold: State<u64>,
    /// The verse a long press selected, until the finger lifts.
    held: State<Option<u16>>,
}

impl DragSelect {
    /// Install (once) and return the chapter-scoped drag cursor.
    /// Not a `use_hook` — `chapter_view` is called conditionally
    /// (hidden while the chapter loads), so a real hook would
    /// violate Freya's "no conditional hooks" rule. Instead, this
    /// does a plain context lookup and provides the context on
    /// miss. `State::create_in_scope(ScopeId::ROOT)` keeps the slot
    /// alive across any re-render where the view disappears.
    fn use_context() -> Self {
        if let Some(existing) = try_consume_context::<Self>() {
            return existing;
        }
        let fresh = Self {
            start: State::create_in_scope(None, ScopeId::ROOT),
            end: State::create_in_scope(None, ScopeId::ROOT),
            pressing: State::create_in_scope(false, ScopeId::ROOT),
            last_tap: State::create_in_scope(None, ScopeId::ROOT),
            hold: State::create_in_scope(0, ScopeId::ROOT),
            held: State::create_in_scope(None, ScopeId::ROOT),
        };
        provide_context(fresh);
        fresh
    }

    /// A press on `verse`; returns whether it completes a double tap.
    fn begin(mut self, verse: u16) -> bool {
        self.start.set(Some(verse));
        self.end.set(Some(verse));
        self.pressing.set(true);
        let now = Instant::now();
        let double = self
            .last_tap
            .peek()
            .is_some_and(|(at, v)| v == verse && now.duration_since(at) <= DOUBLE_TAP);
        self.last_tap.set((!double).then_some((now, verse)));
        double
    }

    /// Start the long-press timer of the press just begun on the
    /// paragraph `placed` tracks.
    fn arm_hold(mut self, placed: Rc<Cell<Area>>) {
        let token = self.hold.peek().wrapping_add(1);
        self.hold.set(token);
        let origin = placed.get().origin;
        spawn(async move {
            async_io::Timer::after(LONG_PRESS).await;
            // A finger that scrolled moved the paragraph, even when the
            // scroll view swallowed its moves.
            let now = placed.get().origin;
            if (now.x - origin.x).hypot(now.y - origin.y) >= SLOP_PX {
                self.cancel_hold();
            } else {
                self.hold_due(token);
            }
        });
    }

    /// The long press armed with `token` came due: select its verse when
    /// the press is still the same, still and on one verse.
    fn hold_due(mut self, token: u64) {
        let still_pressing = *self.pressing.peek() && *self.hold.peek() == token;
        let start = *self.start.peek();
        if let Some(verse) = start.filter(|_| still_pressing && *self.end.peek() == start) {
            self.held.set(Some(verse));
            self.last_tap.set(None);
        }
    }

    /// Forget the press: a scroll or a drag took it over.
    fn cancel_hold(mut self) {
        let token = self.hold.peek().wrapping_add(1);
        self.hold.set(token);
    }

    /// The finger or button lifted: what the menu opens on, if the press
    /// was a long press or a drag across verses. Every paragraph hears
    /// the lift; the first one ends the press for all.
    fn lift(self) -> Option<HighlightKind> {
        if !*self.pressing.peek() {
            return None;
        }
        match self.finish() {
            (_, _, Some(verse)) => Some(HighlightKind::Verse(verse)),
            (Some(a), Some(b), None) if a != b => Some(HighlightKind::Range(a.min(b), a.max(b))),
            _ => None,
        }
    }

    /// End the press; returns the verses it spanned and the verse a long
    /// press selected.
    fn finish(mut self) -> (Option<u16>, Option<u16>, Option<u16>) {
        let ends = (*self.start.peek(), *self.end.peek(), *self.held.peek());
        self.start.set(None);
        self.end.set(None);
        self.pressing.set(false);
        self.cancel_hold();
        if self.held.peek().is_some() {
            self.held.set(None);
        }
        ends
    }
}

/// Re-render a few times right after mount, so highlight rects computed
/// from the laid-out paragraph appear even when `on_sized` fired before
/// the handler was attached. At most three renders in the first 150 ms.
fn use_settle_ticks() {
    let tick: State<u32> = use_state(|| 0);
    use_hook(move || {
        let mut tick = tick;
        spawn(async move {
            for i in 1u32..=3 {
                async_io::Timer::after(std::time::Duration::from_millis(40)).await;
                tick.set(i);
            }
        });
    });
    let _ = *tick.read();
}

/// Which verse(s) a bookmark gesture targets. `Verse` fires for a
/// double-tap or long-press on a single verse; `Range` fires when the
/// user drags across multiple verses (inclusive on both ends).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightKind {
    Verse(u16),
    Range(u16, u16),
}

impl HighlightKind {
    /// First and last verse.
    #[must_use]
    pub const fn span(self) -> (u16, u16) {
        match self {
            Self::Verse(v) => (v, v),
            Self::Range(a, b) => (a, b),
        }
    }
}

/// Full gesture payload.
///
/// `anchor` is the global (window-relative) pointer location where the gesture
/// started, so the host can float the colour picker next to the touch point
/// instead of pinning it to a fixed corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HighlightTarget {
    pub kind: HighlightKind,
    pub anchor_x: f32,
    pub anchor_y: f32,
}

/// Callback the reader fires when a bookmark gesture completes.
pub type OnHighlight = EventHandler<HighlightTarget>;

/// Live-preview highlight shown while the colour picker is open.
///
/// The shell forwards whatever colour the user is currently hovering / dragging
/// on the wheel; the reader overrides those verses' tint so the paragraph
/// updates in real time instead of only on commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HighlightPreview {
    pub lo: u16,
    pub hi: u16,
    pub color: Color,
}

/// What every paragraph of a chapter shares: references, footnotes, the
/// gesture callbacks and the type settings.
#[derive(Clone, PartialEq)]
pub struct Reading {
    pub cross_refs: Rc<HashMap<u16, Vec<CrossRefView>>>,
    pub footnotes: Rc<HashMap<u16, Vec<FootnoteView>>>,
    pub on_jump: OnJump,
    /// Show the cross-references of a verse.
    pub on_refs: OnRefs,
    /// The verse whose cross-references are showing.
    pub refs_open: Option<u16>,
    pub on_highlight: OnHighlight,
    /// Colour being tried on a range while the picker is open.
    pub preview: Option<HighlightPreview>,
    /// First and last verse the selection menu works on, underlined.
    pub selected: Option<(u16, u16)>,
    pub font_size: f32,
    pub line_height: f32,
}

impl std::fmt::Debug for Reading {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reading")
            .field("cross_refs", &self.cross_refs.len())
            .field("footnotes", &self.footnotes.len())
            .field("preview", &self.preview)
            .field("selected", &self.selected)
            .field("font_size", &self.font_size)
            .field("line_height", &self.line_height)
            .finish_non_exhaustive()
    }
}

/// The whole chapter: title, sections and paragraphs, scrolled through
/// `scroll_ctx` when the host drives it. `flash_verse` is washed over any
/// highlight so a jump target stands out.
#[must_use]
pub fn chapter_view(
    plan: &ChapterPlan,
    highlights: &[&BibleBookmark],
    reading: &Reading,
    flash_verse: Option<u16>,
    scroll_ctx: Option<VerseScrollCtx>,
) -> impl IntoElement {
    // Install the chapter-scoped drag-select cursor so every verse
    // marker rendered under this view shares one `(start, end)` state
    // for the drag-highlight gesture.
    let _drag = DragSelect::use_context();
    let mut styles = style_map(highlights);
    if let Some(v) = flash_verse {
        // Transient wash — overrides any user highlight on the flash
        // target so the eye lands immediately.
        styles.insert(
            v,
            VerseStyle {
                tint: Some(palette().highlighters[0]),
                has_note: styles.get(&v).is_some_and(|s| s.has_note),
            },
        );
    }
    if let Some(prev) = reading.preview {
        for v in prev.lo..=prev.hi {
            styles.insert(
                v,
                VerseStyle {
                    tint: Some(prev.color),
                    has_note: styles.get(&v).is_some_and(|s| s.has_note),
                },
            );
        }
    }

    // Install the scroll context so inline VerseMarkers can snap the
    // chapter viewport onto a chip-tap / TSK flash target during
    // layout. Controller + flash signal travel together; the outer
    // rect publishes its window-coord top so each verse can convert
    // its own `on_sized` area into a scroll offset.
    let viewport_top_setter = scroll_ctx.as_ref().map(|c| c.viewport_top);
    if let Some(ctx) = scroll_ctx.as_ref() {
        provide_context(ctx.clone());
    }
    let scroll = scroll_ctx.map_or_else(ScrollView::new, |ctx| {
        ScrollView::new_controlled(ctx.controller)
    });
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(palette().bg)
        .padding((0., 28., 12., 28.))
        .on_sized(move |e: Event<SizedEventData>| {
            if let Some(mut setter) = viewport_top_setter
                && (*setter.peek() - e.area.min_y()).abs() > 0.5
            {
                setter.set(e.area.min_y());
            }
        })
        .font_family(typography::FAMILY)
        .child(
            scroll.child(notein_ui_kit::ScrollContent::new(
                rect()
                    .width(Size::fill())
                    .cross_align(Alignment::Center)
                    .child(
                        rect()
                            .width(Size::fill())
                            .max_width(Size::px(typography::MEASURE_PX))
                            .padding(16.)
                            .spacing(14.)
                            .child(book_title_header(plan))
                            .child(all_blocks(plan, &styles, reading)),
                    ),
            )),
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
                    .color(palette().t1)
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
    reading: &Reading,
) -> impl IntoElement {
    let mut root = rect().width(Size::fill()).spacing(14.);
    for block in &plan.blocks {
        match block {
            Block::Paragraph(spans) => {
                let starts_chapter = spans.first().is_some_and(|s| s.verse == 1);
                root = root.child(VerseParagraph {
                    spans: spans.clone(),
                    styles: styles.clone(),
                    reading: reading.clone(),
                    starts_chapter: starts_chapter.then_some(plan.chapter),
                });
            }
            Block::Header(text) => {
                root = root.child(section_header(text));
            }
        }
    }
    root
}

/// A paragraph of verses with inline verse numbers, highlight tints,
/// note icons, footnote letters and cross-reference buttons.
///
/// It owns the highlight gestures: a tap or drag anywhere on the text is
/// hit-tested to a UTF-16 offset and mapped to its verse, so the small
/// inline markers carry no handlers. `starts_chapter` holds the chapter
/// number when the paragraph contains verse 1, whose number is then drawn
/// as a large inline chapter number.
#[derive(Clone, PartialEq)]
pub(super) struct VerseParagraph {
    pub(super) spans: Vec<ParagraphSpan>,
    pub(super) styles: HashMap<u16, VerseStyle>,
    pub(super) reading: Reading,
    pub(super) starts_chapter: Option<u16>,
}

impl std::fmt::Debug for VerseParagraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerseParagraph")
            .field("spans", &self.spans.len())
            .finish_non_exhaustive()
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

/// Resolve a global pointer location to the UTF-16 offset inside the
/// laid-out paragraph the holder references. Returns `None` before the
/// first frame has published the paragraph (holder empty) or before
/// `on_sized` has published the paragraph area (zero area).
///
/// Freya bubbles pointer events from the target node up to ancestor
/// handlers but keeps `element_location` relative to the ORIGINAL hit
/// target. For a verse paragraph that means a tap on an inline child
/// (`VerseMarker`, `FootnoteLetter`, `RefHoverButton`) arrives at the
/// paragraph handler with coordinates relative to the tiny child box,
/// not the paragraph — hit testing against the laid-out paragraph then
/// returns a stale offset and the handler fires for the wrong verse.
/// Rebasing from `global_location` against the paragraph's own area
/// closes the hole.
///
/// The laid-out paragraph measures in physical pixels, the pointer in
/// logical ones: the point is scaled before the hit test, or on a dense
/// screen it lands up and to the left, on an earlier verse.
fn hit_offset(holder: &ParagraphHolder, global: CursorPoint, area: Area) -> Option<usize> {
    if area.width() <= 0.0 || area.height() <= 0.0 {
        return None;
    }
    let elem = CursorPoint::new(
        global.x - f64::from(area.min_x()),
        global.y - f64::from(area.min_y()),
    );
    let vis = holder.visible_location(elem);
    let inner = holder.0.borrow();
    let data = inner.as_ref()?;
    let point = (vis.x * data.scale_factor, vis.y * data.scale_factor);
    let caret = data.paragraph.cursor_index_at_point(point);
    Some(character_at(&data.paragraph, caret, point))
}

/// The character under `point`, given the caret `cursor_index_at_point`
/// found there. Past the middle of a character the caret sits after it,
/// which at a verse's last character is already the next verse.
fn character_at(paragraph: &SkParagraph, caret: usize, (x, y): (f64, f64)) -> usize {
    let Some(before) = caret.checked_sub(1) else {
        return caret;
    };
    #[allow(
        clippy::cast_possible_truncation,
        reason = "pixel coordinates fit an f32"
    )]
    let (x, y) = (x as f32, y as f32);
    let under = paragraph
        .get_rects_for_range(before..caret, RectHeightStyle::Max, RectWidthStyle::Tight)
        .iter()
        .any(|b| {
            (b.rect.left..=b.rect.right).contains(&x) && (b.rect.top..=b.rect.bottom).contains(&y)
        });
    if under { before } else { caret }
}

impl VerseParagraph {
    /// Add the verses to `p`; returns it with each verse's UTF-16 range
    /// and the ranges to tint.
    fn compose(
        &self,
        mut p: Paragraph,
    ) -> (Paragraph, Vec<VerseRange>, Vec<(usize, usize, Color)>) {
        let on_jump = &self.reading.on_jump;
        let font_size = self.reading.font_size;
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
                .reading
                .preview
                .as_ref()
                .filter(|p| span.verse >= p.lo && span.verse <= p.hi)
                .map(|p| p.color);

            p = p.child(VerseMarker {
                verse: span.verse,
                display: match self.starts_chapter {
                    Some(chapter) if span.verse == 1 => MarkerDisplay::Chapter(chapter),
                    _ => MarkerDisplay::Verse,
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

            if style.is_some_and(|s| s.has_note) {
                p = p.child(note_icon(font_size));
                utf16_offset += 1;
            }
            if let Some(notes) = self.reading.footnotes.get(&span.verse) {
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
            if self.reading.cross_refs.contains_key(&span.verse) {
                p = p.child(RefButton {
                    verse: span.verse,
                    open: self.reading.refs_open == Some(span.verse),
                    on_press: self.reading.on_refs.clone(),
                    font_size,
                });
                utf16_offset += 1;
            }
        }
        (p, verse_ranges, highlight_items)
    }

    /// Text ranges of the verses the selection works on: the one a long
    /// press holds, else the ones the host's menu is open on.
    fn underlined(&self, ranges: &[VerseRange], held: Option<u16>) -> Vec<(usize, usize)> {
        let Some((lo, hi)) = held.map(|v| (v, v)).or(self.reading.selected) else {
            return Vec::new();
        };
        ranges
            .iter()
            .filter(|(_, _, v)| (lo..=hi).contains(v))
            .map(|(start, end, _)| (*start, *end))
            .collect()
    }
}

impl Component for VerseParagraph {
    fn render(&self) -> impl IntoElement {
        let holder = use_hook(ParagraphHolder::default);
        let drag = use_consume::<DragSelect>();
        // Global pointer location captured on `pointer_down`. The
        // host anchors its colour picker here so the popup opens next
        // to the finger / cursor instead of pinned to a fixed corner.
        let anchor: State<(f32, f32)> = use_state(|| (0.0, 0.0));
        // Where the paragraph sits on screen, for hit testing. A plain
        // cell: it moves every scrolled frame, which must not render
        // the paragraph again.
        let placed = use_hook(|| Rc::new(Cell::new(Area::default())));
        // Its size only flips 0 → real size after the first layout;
        // reading it renders once more, so the second pass computes the
        // bg rects from a laid-out paragraph.
        let mut laid_out: State<(f32, f32)> = use_state(|| (0.0, 0.0));
        use_settle_ticks();

        let on_highlight = self.reading.on_highlight.clone();
        let p = paragraph()
            .width(Size::fill())
            .line_height(self.reading.line_height)
            .color(palette().t1)
            .font_family(typography::FAMILY)
            .font_size(self.reading.font_size)
            .holder(holder.clone())
            .on_sized({
                let placed = Rc::clone(&placed);
                move |e: Event<SizedEventData>| {
                    let area = e.data().area;
                    placed.set(area);
                    laid_out.set_if_modified((area.width(), area.height()));
                }
            });
        let (p, verse_ranges, highlight_items) = self.compose(p);

        let _ = *laid_out.read();
        let underlined = self.underlined(&verse_ranges, *drag.held.read());
        let bg_rects = build_bg_rects(&holder, &highlight_items, &underlined);

        // Gesture wiring: paragraph-level so a tap anywhere in verse
        // text — not just the tiny superscript marker — fires.
        let verse_at = {
            let (holder, placed) = (holder, Rc::clone(&placed));
            Rc::new(move |at: CursorPoint| {
                hit_offset(&holder, at, placed.get())
                    .and_then(|offset| offset_to_verse(&verse_ranges, offset))
            })
        };
        let verse_at_move = Rc::clone(&verse_at);

        let p = p
            .on_pointer_down({
                let on_highlight = on_highlight.clone();
                let mut anchor = anchor;
                move |e: Event<PointerEventData>| {
                    if !e.data().is_primary() {
                        return;
                    }
                    let g = e.global_location();
                    let Some(verse) = verse_at(g) else {
                        return;
                    };

                    #[allow(clippy::cast_possible_truncation)] // window coordinates
                    let anchor_pt = (g.x as f32, g.y as f32);
                    anchor.set(anchor_pt);

                    if drag.begin(verse) {
                        on_highlight.call(HighlightTarget {
                            kind: HighlightKind::Verse(verse),
                            anchor_x: anchor_pt.0,
                            anchor_y: anchor_pt.1,
                        });
                        drag.finish();
                        return;
                    }

                    drag.arm_hold(Rc::clone(&placed));
                }
            })
            .on_pointer_move({
                let mut drag = drag;
                move |e: Event<PointerEventData>| {
                    if !*drag.pressing.peek() {
                        return;
                    }
                    // A finger dragging over the text scrolls it.
                    if !matches!(e.data(), PointerEventData::Mouse(_)) {
                        drag.cancel_hold();
                        return;
                    }
                    let Some(verse) = verse_at_move(e.global_location()) else {
                        return;
                    };
                    if *drag.end.peek() != Some(verse) {
                        drag.end.set(Some(verse));
                        drag.cancel_hold();
                    }
                }
            })
            .on_global_pointer_up({
                move |_: Event<PointerEventData>| {
                    let Some(kind) = drag.lift() else {
                        return;
                    };
                    let (ax, ay) = *anchor.peek();
                    on_highlight.call(HighlightTarget {
                        kind,
                        anchor_x: ax,
                        anchor_y: ay,
                    });
                }
            });

        // Wrap: bg rects first (painted behind) + paragraph with
        // gestures second. Both are children of a `rect` whose own
        // size is driven by the paragraph (bg rects are absolute, so
        // they do not contribute to layout).
        rect().width(Size::fill()).child(bg_rects).child(p)
    }
}

/// Dot of the selection underline and the gap after it, in logical px.
const UNDERLINE_DOT: f32 = 2.5;
const UNDERLINE_GAP: f32 = 2.5;

/// Build the per-verse overlay behind the text: one coloured rect per
/// text box of each highlight range, and a dotted underline under each
/// text box of the `underlined` ranges, the verses the selection menu
/// works on. Multi-line verses produce one box per visual line, so the
/// paint hugs the lines naturally.
///
/// Before the first layout publishes into the holder it returns an
/// empty rect; the paragraph's `on_sized` renders again once laid out.
fn build_bg_rects(
    holder: &ParagraphHolder,
    items: &[(usize, usize, Color)],
    underlined: &[(usize, usize)],
) -> impl IntoElement + use<> {
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
    if items.is_empty() && underlined.is_empty() {
        return container;
    }
    let inner = holder.0.borrow();
    let Some(data) = inner.as_ref() else {
        return container;
    };
    #[allow(clippy::cast_possible_truncation)] // a display scale factor
    let scale = data.scale_factor.max(1e-3) as f32;
    // Text boxes of `start..end`, in logical px.
    let boxes = |start: usize, end: usize| {
        data.paragraph
            .get_rects_for_range(start..end, RectHeightStyle::Tight, RectWidthStyle::Tight)
            .into_iter()
            .map(|tb| {
                Area::new(
                    (tb.rect.left / scale, tb.rect.top / scale).into(),
                    (
                        (tb.rect.right - tb.rect.left) / scale,
                        (tb.rect.bottom - tb.rect.top) / scale,
                    )
                        .into(),
                )
            })
            .filter(|b| b.width() > 0.0 && b.height() > 0.0)
            .collect::<Vec<_>>()
    };
    for (start, end, color) in items.iter().filter(|(s, e, _)| s != e) {
        for b in boxes(*start, *end) {
            container = container.child(
                rect()
                    // Mirror the parent's negative relative so each
                    // painted rect sits strictly below the paragraph
                    // sibling in the paint order — otherwise the
                    // auto `+1` Freya applies to every element pulls
                    // them up to the paragraph's layer and the rect
                    // ends up on top of the text + swallowing pointer
                    // events.
                    .layer(Layer::Relative(-1))
                    .position(Position::new_absolute().top(b.min_y()).left(b.min_x()))
                    .width(Size::px(b.width()))
                    .height(Size::px(b.height()))
                    .background(*color),
            );
        }
    }
    let ink = palette().accent;
    for (start, end) in underlined.iter().filter(|(s, e)| s != e) {
        for b in boxes(*start, *end) {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a line holds a few hundred dots at most"
            )]
            let dots = ((b.width() + UNDERLINE_GAP) / (UNDERLINE_DOT + UNDERLINE_GAP)) as usize;
            container = container.child(
                rect()
                    .layer(Layer::Relative(-1))
                    .position(
                        Position::new_absolute()
                            .top(b.max_y() - UNDERLINE_DOT / 2.0)
                            .left(b.min_x()),
                    )
                    .horizontal()
                    .spacing(UNDERLINE_GAP)
                    .children((0..dots).map(|i| {
                        rect()
                            .key(i)
                            .width(Size::px(UNDERLINE_DOT))
                            .height(Size::px(UNDERLINE_DOT))
                            .corner_radius(UNDERLINE_DOT / 2.0)
                            .background(ink)
                            .into_element()
                    })),
            );
        }
    }
    container
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
            .finish_non_exhaustive()
    }
}

impl Component for VerseMarker {
    fn render(&self) -> impl IntoElement {
        let verse = self.verse;
        let inner = match self.display {
            MarkerDisplay::Chapter(chap) => label()
                .font_family(typography::FAMILY)
                .font_size(self.font_size * 2.5)
                .color(palette().line_strong)
                .text(format!("{chap} ")),
            MarkerDisplay::Verse => label()
                .font_size(self.font_size * 0.6)
                .color(palette().t3)
                .text(format!(" {verse} ")),
        };
        // Scroll-into-view hook: when a chip-tap / TSK jump has
        // written this verse into `scroll_to`, align the chapter
        // viewport on it as Freya reports the marker's laid-out area,
        // then consume the target (see `ScrollTarget`).
        let scroll_ctx = try_consume_context::<VerseScrollCtx>();
        rect()
            .padding((0., 2.))
            .on_sized(move |e: Event<SizedEventData>| {
                let Some(ctx) = scroll_ctx.as_ref() else {
                    return;
                };
                let Some(target) = *ctx.scroll_to.peek() else {
                    return;
                };
                if target.verse != verse {
                    return;
                }
                let viewport_top = *ctx.viewport_top.peek();
                if viewport_top == 0.0 && e.area.min_y() == 0.0 {
                    return;
                }
                let mut scroll_to = ctx.scroll_to;
                let since = target.since.unwrap_or_else(std::time::Instant::now);
                let delta = e.area.min_y() - viewport_top - VERSE_SCROLL_MARGIN;
                if delta.abs() < 1.0 || since.elapsed() > SCROLL_SETTLE {
                    scroll_to.set(None);
                    return;
                }
                if target.since.is_none() {
                    scroll_to.set(Some(ScrollTarget {
                        since: Some(since),
                        ..target
                    }));
                }
                let (_, cur_y): (i32, i32) = ctx.controller.into();
                #[allow(clippy::cast_possible_truncation)]
                let desired = cur_y + delta.round() as i32;
                let mut controller = ctx.controller;
                controller.scroll_to_y(desired.max(0));
            })
            .child(inner)
    }
}

/// Normalise the publisher-supplied caller glyph. Many translations
/// ship `"+"` as an autogen placeholder — fall back to positional
/// letters a/b/c in that case so the UI stays readable.
fn display_letter(caller: &str, idx: usize) -> String {
    let trimmed = caller.trim();
    if trimmed.is_empty() || trimmed == "+" || trimmed == "*" {
        u8::try_from(idx)
            .ok()
            .filter(|i| *i < 26)
            .map_or_else(|| idx.to_string(), |i| char::from(b'a' + i).to_string())
    } else {
        trimmed.to_string()
    }
}

/// Open state of an inline hover popup. Leaving waits a moment before
/// closing, and entering cancels that, so the pointer can travel from
/// the trigger to the popup.
#[derive(Clone, Copy, PartialEq)]
struct Hover {
    open: State<bool>,
    /// Bumped on every change; a pending close only fires while it
    /// still holds the value it saw.
    version: State<u64>,
}

impl std::fmt::Debug for Hover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hover")
            .field("open", &*self.open.peek())
            .finish_non_exhaustive()
    }
}

impl Hover {
    fn use_hover() -> Self {
        Self {
            open: use_state(|| false),
            version: use_state(|| 0),
        }
    }

    fn bump(mut self) -> u64 {
        let next = self.version.peek().wrapping_add(1);
        self.version.set(next);
        next
    }

    fn enter(mut self) {
        self.open.set(true);
        self.bump();
    }

    fn close(mut self) {
        self.open.set(false);
        self.bump();
    }

    /// Close after a short delay unless the pointer comes back.
    fn leave(self) {
        let token = self.bump();
        let mut open = self.open;
        let version = self.version;
        spawn(async move {
            async_io::Timer::after(std::time::Duration::from_millis(350)).await;
            if *version.peek() == token {
                open.set(false);
            }
        });
    }
}

/// Place a `width` × `height` popup under the trigger at `anchor`,
/// clamped inside the viewport; until both are measured it sits
/// `fallback_top` below the trigger.
fn place_popup(
    popup: Rect,
    anchor: Area,
    viewport: Option<notein_ui_kit::ViewportArea>,
    (width, height): (f32, f32),
    fallback_top: f32,
) -> Rect {
    const MARGIN: f32 = 8.0;
    let viewport_area = viewport.map_or(anchor, notein_ui_kit::ViewportArea::get);
    let has_viewport = viewport_area.width() > 0.0 && viewport_area.height() > 0.0;
    if has_viewport && anchor.width() > 0.0 && height > 0.0 {
        let top = notein_ui_kit::clamp_popup_top(
            viewport_area,
            anchor,
            anchor.max_y() + 4.0,
            height,
            MARGIN,
        );
        let left = notein_ui_kit::clamp_popup_left(viewport_area, anchor.min_x(), width, MARGIN);
        popup.position(Position::new_global().top(top).left(left))
    } else {
        popup.position(Position::new_absolute().top(fallback_top).left(0.))
    }
}

/// A state that follows an element's area.
fn track_area(mut area: State<Area>) -> impl FnMut(Event<SizedEventData>) + 'static {
    move |e: Event<SizedEventData>| {
        let new_area = e.data().area;
        if *area.peek() != new_area {
            area.set(new_area);
        }
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
            .finish_non_exhaustive()
    }
}

impl Component for FootnoteLetter {
    fn render(&self) -> impl IntoElement {
        const POPUP_WIDTH: f32 = 300.0;
        let hover = Hover::use_hover();
        let letter_area: State<Area> = use_state(Area::default);
        let popup_area: State<Area> = use_state(Area::default);
        let viewport = try_consume_context::<notein_ui_kit::ViewportArea>();
        let target = self.target.clone();
        let on_jump = self.jump.clone();
        let marker_font = self.font_size * 0.55;

        let container = rect()
            .padding((0., 1.))
            .margin((0., 1.))
            .on_sized(track_area(letter_area))
            .on_pointer_enter(move |_| hover.enter())
            .on_pointer_leave(move |_| hover.leave())
            // Swallow pointer_down so a footnote tap does not seed
            // the paragraph's drag-highlight or double-tap gestures.
            .on_pointer_down(|e: Event<PointerEventData>| {
                e.stop_propagation();
            })
            .on_tap(move |_: Event<PressEventData>| {
                hover.close();
                if let Some(t) = target.clone() {
                    on_jump.call(t);
                }
            })
            .child(
                label()
                    .font_size(marker_font)
                    .color(palette().accent_text)
                    .text(self.letter.clone()),
            );
        if !*hover.open.read() || self.text.is_empty() {
            return container;
        }

        let anchor = *letter_area.read();
        let height = popup_area.read().height().max(0.0);
        let measured = anchor.width() > 0.0 && height > 0.0;
        let popup = rect()
            .layer(Layer::Overlay)
            .width(Size::px(POPUP_WIDTH))
            .padding((10., 12.))
            .with_corner_radius(8.0)
            .background(palette().s2)
            .on_sized(track_area(popup_area))
            .opacity(if measured { 1.0 } else { 0.0 })
            .on_pointer_down(|e: Event<PointerEventData>| {
                e.stop_propagation();
            })
            .on_pointer_enter(move |_| hover.enter())
            .on_pointer_leave(move |_| hover.leave())
            .child(
                label()
                    .font_size(self.font_size * 0.65)
                    .color(palette().t1)
                    .text(self.text.clone()),
            );
        container.child(place_popup(
            popup,
            anchor,
            viewport,
            (POPUP_WIDTH, height),
            marker_font * 1.6,
        ))
    }
}

/// The icon closing a verse that has cross-references; pressing it
/// opens them in the host's panel. Accented while that panel shows this
/// verse.
#[derive(Clone, Debug, PartialEq)]
struct RefButton {
    verse: u16,
    open: bool,
    on_press: OnRefs,
    font_size: f32,
}

impl Component for RefButton {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let icon_size = self.font_size * 0.6;
        let (verse, on_press) = (self.verse, self.on_press.clone());
        rect()
            .padding((2., 4.))
            .margin((0., 2.))
            .corner_radius(4.0)
            .maybe(self.open, |r| r.background(p.accent_soft))
            .a11y_role(AccessibilityRole::Button)
            // A tap on the icon must not reach the paragraph, where a
            // second tap would open the highlight picker.
            .on_pointer_down(|e: Event<PointerEventData>| e.stop_propagation())
            .on_tap(move |_| on_press.call(verse))
            .child(
                SvgViewer::new(freya::icons::lucide::trending_up_down())
                    .color(p.accent_text)
                    .width(Size::px(icon_size))
                    .height(Size::px(icon_size)),
            )
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
                .color(palette().t3)
                .width(Size::px(size))
                .height(Size::px(size)),
        )
}

fn section_header(text: &str) -> impl IntoElement {
    rect()
        .width(Size::fill())
        .center()
        .padding((12., 0., 4., 0.))
        .child(
            label()
                .font_family(typography::FAMILY)
                .font_size(typography::SECTION_HEADER_PX)
                .color(palette().t1)
                .text(text.to_string()),
        )
}

fn spacer(height: f32) -> impl IntoElement {
    rect().width(Size::fill()).height(Size::px(height))
}

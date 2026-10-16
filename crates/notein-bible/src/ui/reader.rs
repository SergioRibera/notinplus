//! Freya chapter reader.
//!
//! Pure builders — the host embeds [`chapter_view`] inside a reactive
//! scope and feeds a loaded [`ChapterPlan`]. The reader itself does not
//! own state; chapter navigation and selection live on
//! [`crate::state::BibleViewerState`].

use std::collections::HashMap;

use freya::prelude::*;

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
#[derive(Clone, Debug, Default)]
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

/// Pending-highlight target emitted by the three bookmark gestures.
/// `Verse` fires for a double-tap or long-press on a single verse
/// marker; `Range` fires when the user drags between two verse markers
/// (inclusive on both ends — the shell applies the chosen colour to
/// every verse in the range).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightTarget {
    Verse(u16),
    Range(u16, u16),
}

/// Callback the reader fires when a bookmark gesture completes.
pub type OnHighlight = EventHandler<HighlightTarget>;

pub fn chapter_view(
    plan: &ChapterPlan,
    highlights: &[&BibleBookmark],
    cross_refs: &HashMap<u16, Vec<CrossRefView>>,
    footnotes: &HashMap<u16, Vec<FootnoteView>>,
    on_jump: OnJump,
    on_highlight: OnHighlight,
    flash_verse: Option<u16>,
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
) -> impl IntoElement {
    let mut p = paragraph()
        .width(Size::fill())
        .line_height(line_height)
        .color(parse_rgb(theme::TEXT_PRIMARY))
        .font_family(typography::FAMILY)
        .font_size(font_size);

    // Freya's `paragraph.highlights` indexes UTF-16 code units inside
    // the paragraph (spans + inline placeholders). Each `.child(...)`
    // adds one placeholder (one UTF-16 unit); each span contributes
    // `text.encode_utf16().count()` units. Track both so the highlight
    // range covers only the verse text — not the preceding verse
    // marker nor the trailing footnote / cross-ref icon.
    let mut utf16_offset: usize = 0;
    let mut highlight_ranges: Vec<(usize, usize)> = Vec::new();
    let highlight_color = Color::from_rgb(170, 150, 60);

    for span in spans {
        let style = styles.get(&span.verse);

        p = p.child(VerseMarker {
            verse: span.verse,
            display: if span.verse == 1 && starts_chapter.is_some() {
                MarkerDisplay::Chapter(starts_chapter.unwrap())
            } else {
                MarkerDisplay::Verse
            },
            on_highlight: on_highlight.clone(),
            font_size,
        });
        utf16_offset += 1; // placeholder for the marker child

        let text = format!("{} ", span.text);
        let text_units = text.encode_utf16().count();
        let start = utf16_offset;
        p = p.span(Span::new(text).font_size(font_size));
        utf16_offset += text_units;
        let end = utf16_offset;

        if style.is_some() {
            highlight_ranges.push((start, end));
        }

        if style.map(|s| s.has_note).unwrap_or(false) {
            p = p.child(note_icon(font_size));
            utf16_offset += 1;
        }
        if let Some(notes) = footnotes.get(&span.verse) {
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
        if let Some(refs) = cross_refs.get(&span.verse) {
            p = p.child(RefHoverButton {
                refs: refs.clone(),
                jump: on_jump.clone(),
                font_size,
            });
            utf16_offset += 1;
        }
    }

    if !highlight_ranges.is_empty() {
        p = p.highlights(highlight_ranges).highlight_color(highlight_color);
    }
    p
}

/// What the clickable verse marker should display — the superscript
/// verse number OR the big chapter number (when the marker opens a
/// chapter + sits on verse 1).
#[derive(Clone, Copy, PartialEq, Debug)]
enum MarkerDisplay {
    Verse,
    Chapter(u16),
}

/// Clickable verse number. Opens the colour-picker overlay via
/// `on_highlight(verse)` on **two** gestures — double-tap or
/// long-press — so a stray single tap while reading never commits a
/// highlight. The display styling matches the previous span-based
/// renderer so paragraph flow stays unchanged.
#[derive(Clone, PartialEq)]
struct VerseMarker {
    verse: u16,
    display: MarkerDisplay,
    on_highlight: OnHighlight,
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
        let on_highlight = self.on_highlight.clone();
        // Monotonic token so a `pointer_up` / `pointer_leave` that
        // arrives after the long-press timer has already fired is a
        // no-op, and so a second press starts a fresh timer.
        let long_press_token = use_state(|| 0u64);
        // Chapter-level drag cursor (installed by `chapter_view`).
        let drag = use_consume::<DragSelect>();

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

        rect()
            .padding((0., 2.))
            .on_pointer_down({
                let mut long_press_token = long_press_token;
                let on_highlight = on_highlight.clone();
                let mut drag = drag;
                move |e: Event<PointerEventData>| {
                    // Seed the drag cursor on this verse so a drag that
                    // ends on a different marker produces a range.
                    drag.start.set(Some(verse));
                    drag.end.set(Some(verse));
                    drag.pressing.set(true);

                    // Advance the token to invalidate any still-queued
                    // long-press timer from a previous press, then bind
                    // a copy for this press.
                    let my_token = long_press_token.peek().wrapping_add(1);
                    long_press_token.set(my_token);

                    // Double-tap: EventsCombos tracks consecutive
                    // presses at the same global location within the
                    // multi-press window. `is_double` fires on the
                    // second tap only.
                    if EventsCombos::<()>::pressed(e.global_location()).is_double() {
                        on_highlight.call(HighlightTarget::Verse(verse));
                        // Collapse the drag so pointer-up skips the
                        // range emit path.
                        drag.start.set(None);
                        drag.end.set(None);
                        drag.pressing.set(false);
                        return;
                    }

                    // Long-press: fire after the press has survived
                    // `LONG_PRESS_MS` without the token being bumped
                    // (release / move-away / another press all bump).
                    let on_highlight = on_highlight.clone();
                    let long_press_token = long_press_token;
                    let mut drag_for_long = drag;
                    spawn(async move {
                        async_io::Timer::after(std::time::Duration::from_millis(
                            LONG_PRESS_MS,
                        ))
                        .await;
                        if *long_press_token.peek() == my_token {
                            on_highlight.call(HighlightTarget::Verse(verse));
                            drag_for_long.start.set(None);
                            drag_for_long.end.set(None);
                            drag_for_long.pressing.set(false);
                        }
                    });
                }
            })
            .on_pointer_enter({
                let mut drag = drag;
                move |_: Event<PointerEventData>| {
                    // Extend the drag end while the pointer is held
                    // down. `EventsCombos::pressing` is true from the
                    // first marker's pointer-down until the global
                    // pointer-up.
                    if *drag.pressing.peek() && drag.start.peek().is_some() {
                        drag.end.set(Some(verse));
                    }
                }
            })
            .on_global_pointer_up({
                let mut long_press_token = long_press_token;
                let on_highlight = on_highlight.clone();
                let mut drag = drag;
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
                        on_highlight.call(HighlightTarget::Range(lo, hi));
                    }
                }
            })
            .on_pointer_leave({
                let mut long_press_token = long_press_token;
                move |_: Event<PointerEventData>| {
                    let next = long_press_token.peek().wrapping_add(1);
                    long_press_token.set(next);
                }
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

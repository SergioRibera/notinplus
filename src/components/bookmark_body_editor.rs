//! Rich body editor for [`crate::bookmark::Bookmark`] cards.
//!
//! Phase 6 is layered as follows inside this file:
//!
//! - 6.1 (chip rendering). The editor keeps a rope (via
//!   [`freya_edit::use_editable`]) where every chip is a single
//!   `\u{FFFC}` object replacement char, plus a parallel `chip_map`
//!   keyed by UTF-16 code-unit index in that rope. The map is
//!   rebased after every edit so indices stay consistent with the
//!   rope. On mount the stored body string (markdown-lite with
//!   `[[kind:value]]`) is parsed into both pieces; on every mutation
//!   the pieces are joined back into the canonical markdown-lite
//!   form and written to the parent `Writable<String>`.
//! - 6.1 (atomic chip delete) falls out of the chip-as-one-char
//!   model. Backspace / Delete remove a single `\u{FFFC}` which
//!   `rebase_chips` detects and drops atomically.
//! - 6.2 (`[[` autocomplete) adds an overlay below the field. The
//!   trigger char is a single `[`; typing opens the popup, further
//!   typing filters, Up/Down selects, Enter commits (removes the
//!   typed `[query`, inserts a chip sentinel + chip_map entry),
//!   Escape or Backspace-past-trigger closes.
//!
//! Completion sources are supplied by the parent via
//! [`BookmarkBodyEditor::completions`]. Plugin-driven completions
//! land once the plugin trait gets a `completions` method; today the
//! shell builds the list from the current library's bookmarks.

use std::borrow::Cow;

use freya::prelude::*;
use freya_core::elements::paragraph::{ParagraphCursorExt, ParagraphHolderInner};
use freya_edit::{EditableConfig, EditableEvent, EditorLine, TextEditor, use_editable};
use notein_plugin::{ChipView, SourceRef};

use crate::bookmark_body::{
    CHIP_SENTINEL, Completion, from_rope_with_chips, parse, rebase_chips, serialize,
    to_rope_with_chips,
};
use crate::components::theme::{
    BORDER, BORDER_FOCUS, PRIMARY, SURFACE_PRIMARY, SURFACE_SECONDARY, SURFACE_TERTIARY,
    TEXT_PLACEHOLDER, TEXT_PRIMARY, TEXT_SECONDARY,
};
use crate::source_ref_nav;

/// Minimum height so an empty body still shows a comfortable click
/// target in the floating card.
const DEFAULT_MIN_HEIGHT: f32 = 72.0;

/// Pill background for chip widgets. Slightly brighter than the
/// editor surface so chips read as actionable without feeling like
/// buttons.
const CHIP_BG: Color = Color::from_rgb(44, 52, 70);
const CHIP_FG: Color = Color::from_rgb(200, 215, 240);

/// Cap how many autocomplete entries the popup lists at once so the
/// scroll view stays snappy on long libraries.
const COMPLETION_LIMIT: usize = 32;

/// State of the `[[`-autocomplete popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AutocompleteState {
    /// UTF-16 cu idx of the trigger `[` char in the rope.
    trigger_at: usize,
    /// Highlighted entry index in the current filtered completions.
    selected: usize,
}

/// Multi-line text editor tuned for bookmark bodies.
#[derive(Clone, PartialEq)]
pub struct BookmarkBodyEditor {
    value: Writable<String>,
    placeholder: Option<Cow<'static, str>>,
    min_height: f32,
    completions_provider: Option<Callback<String, Vec<Completion>>>,
}

impl std::fmt::Debug for BookmarkBodyEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BookmarkBodyEditor")
            .field("value", &self.value.read_unchecked())
            .field("placeholder", &self.placeholder)
            .field("min_height", &self.min_height)
            .field(
                "has_completions_provider",
                &self.completions_provider.is_some(),
            )
            .finish()
    }
}

impl BookmarkBodyEditor {
    #[must_use]
    pub fn new(value: impl Into<Writable<String>>) -> Self {
        Self {
            value: value.into(),
            placeholder: None,
            min_height: DEFAULT_MIN_HEIGHT,
            completions_provider: None,
        }
    }

    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'static, str>>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    #[must_use]
    pub const fn min_height(mut self, height: f32) -> Self {
        self.min_height = height;
        self
    }

    /// Register a completion source. Called every render while the
    /// `[[` popup is open; receives the current query string
    /// (everything the user typed after the trigger `[`) and returns
    /// the filtered list to show.
    #[must_use]
    pub fn completions(mut self, provider: impl Into<Callback<String, Vec<Completion>>>) -> Self {
        self.completions_provider = Some(provider.into());
        self
    }
}

impl Component for BookmarkBodyEditor {
    fn render(&self) -> impl IntoElement {
        let a11y_id = use_hook(AccessibilityId::new_unique);
        let focus = use_focus(a11y_id);
        let holder = use_state(ParagraphHolder::default);
        let mut area = use_state(Area::default);
        let mut is_dragging = use_state(|| false);

        let init_value = self.value.clone();
        let init_value_chips = self.value.clone();
        let mut editable = use_editable(
            move || {
                let segs = parse(&init_value.read());
                to_rope_with_chips(&segs).0
            },
            EditableConfig::new,
        );
        let mut chip_map = use_state::<Vec<(usize, SourceRef)>>(move || {
            let segs = parse(&init_value_chips.read());
            to_rope_with_chips(&segs).1
        });
        let mut autocomplete = use_state::<Option<AutocompleteState>>(|| None);
        let mut value = self.value.clone();

        // Pull external edits (someone else mutated the body signal)
        // into the editor + chip map so the rope keeps up.
        let canonical = serialize(&from_rope_with_chips(
            &editable.editor().read().committed_text(),
            &chip_map.read(),
        ));
        if *value.read() != canonical {
            let segs = parse(&value.read());
            let (rope, chips) = to_rope_with_chips(&segs);
            {
                let mut editor = editable.editor_mut().write();
                editor.clear_preedit();
                editor.set(&rope);
                editor.editor_history_mut().clear();
                editor.clear_selection();
            }
            chip_map.set(chips);
            autocomplete.set(None);
        }

        let display_placeholder = value.read().is_empty()
            && self.placeholder.is_some()
            && !editable.editor().read().has_preedit();

        // Compute the current filtered completions so the key handler
        // and the popup render agree on what Enter commits.
        let (ac_state, ac_filtered) = compute_autocomplete(
            autocomplete.read().as_ref().copied(),
            &editable,
            self.completions_provider.as_ref(),
        );

        let on_ime_preedit = move |e: Event<ImePreeditEventData>| {
            let mut editor = editable.editor_mut().write();
            if e.data().text.is_empty() {
                editor.clear_preedit();
            } else {
                editor.set_preedit(&e.data().text);
            }
        };

        // Captured for use inside the key handler. Rebuilt on each
        // render — closures observe the latest filtered list.
        let ac_filtered_for_keys = ac_filtered.clone();
        let ac_state_for_keys = ac_state;

        let on_key_down = move |e: Event<KeyboardEventData>| {
            let key = e.key.clone();
            let modifiers = e.modifiers;

            // Autocomplete-active key routing. Nav keys never fall
            // through to the editor; Enter commits + swallows.
            if let Some(state) = ac_state_for_keys {
                let n = ac_filtered_for_keys.len();
                match &key {
                    Key::Named(NamedKey::Escape) => {
                        e.stop_propagation();
                        e.prevent_default();
                        autocomplete.set(None);
                        return;
                    }
                    Key::Named(NamedKey::ArrowDown) if n > 0 => {
                        e.stop_propagation();
                        e.prevent_default();
                        autocomplete.set(Some(AutocompleteState {
                            trigger_at: state.trigger_at,
                            selected: (state.selected + 1) % n,
                        }));
                        return;
                    }
                    Key::Named(NamedKey::ArrowUp) if n > 0 => {
                        e.stop_propagation();
                        e.prevent_default();
                        autocomplete.set(Some(AutocompleteState {
                            trigger_at: state.trigger_at,
                            selected: (state.selected + n - 1) % n,
                        }));
                        return;
                    }
                    Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Tab) if n > 0 => {
                        e.stop_propagation();
                        e.prevent_default();
                        if let Some(pick) = ac_filtered_for_keys.get(state.selected).cloned() {
                            commit_completion(
                                &editable,
                                &mut chip_map,
                                &mut value,
                                state.trigger_at,
                                pick,
                            );
                        }
                        autocomplete.set(None);
                        return;
                    }
                    _ => {}
                }
            }

            // Escape outside autocomplete: unfocus the field.
            if matches!(&key, Key::Named(NamedKey::Escape)) {
                a11y_id.request_unfocus();
                return;
            }

            // Detect `[` trigger BEFORE the editor consumes the key —
            // we need the cursor position of the `[` char that's
            // about to be inserted.
            let maybe_new_trigger = match &key {
                Key::Character(ch) if ch == "[" && ac_state_for_keys.is_none() => {
                    Some(editable.editor().read().cursor_pos())
                }
                _ => None,
            };

            e.stop_propagation();
            e.prevent_default();
            editable.process_event(EditableEvent::KeyDown {
                key: &key,
                modifiers,
                editor_line: Some(EditorLine::SingleParagraph),
                holder: Some(&holder.read()),
            });
            sync_value(&editable, &mut chip_map, &mut value);

            if let Some(trigger_at) = maybe_new_trigger {
                log::debug!(
                    "bookmark_body_editor: autocomplete triggered at utf16 idx {trigger_at}"
                );
                autocomplete.set(Some(AutocompleteState {
                    trigger_at,
                    selected: 0,
                }));
            } else if let Some(state) = ac_state_for_keys {
                // Still active — check that the trigger char is still
                // in place and the cursor hasn't moved past the start.
                let editor = editable.editor().read();
                let rope = editor.committed_text();
                let cursor = editor.cursor_pos();
                if cursor <= state.trigger_at || !trigger_char_present(&rope, state.trigger_at) {
                    drop(editor);
                    autocomplete.set(None);
                }
            }
        };

        let on_key_up = move |e: Event<KeyboardEventData>| {
            e.stop_propagation();
            editable.process_event(EditableEvent::KeyUp { key: &e.key });
        };

        let on_focus_press = move |e: Event<FocusPressEventData>| {
            e.stop_propagation();
            e.prevent_default();
            is_dragging.set_if_modified(true);
            if !display_placeholder {
                editable.process_event(EditableEvent::Down {
                    location: e.element_location(),
                    editor_line: EditorLine::SingleParagraph,
                    holder: &holder.read(),
                });
            }
            a11y_id.request_focus();
            // Clicking elsewhere in the field dismisses the popup.
            if autocomplete.peek().is_some() {
                autocomplete.set(None);
            }
        };

        let on_global_pointer_move = move |e: Event<PointerEventData>| {
            if a11y_id.is_focused() && *is_dragging.read() {
                let text_area = area.read().to_f64();
                let location = (e.global_location() - text_area.min()).to_point();
                editable.process_event(EditableEvent::Move {
                    location,
                    editor_line: EditorLine::SingleParagraph,
                    holder: &holder.read(),
                });
            }
        };

        let on_global_pointer_up = move |_: Event<PointerEventData>| {
            if a11y_id.is_focused() {
                editable.process_event(EditableEvent::Release);
                if *is_dragging.read() {
                    is_dragging.set(false);
                }
            }
        };

        let on_paragraph_sized = move |e: Event<SizedEventData>| {
            area.set_if_modified(e.area);
        };

        let cursor_index = focus()
            .is_focused()
            .then(|| editable.editor().read().cursor_pos());
        let text_selection = editable
            .editor()
            .read()
            .get_visible_selection(EditorLine::SingleParagraph);
        let border_color = if focus().is_focused() {
            BORDER_FOCUS
        } else {
            BORDER
        };

        let placeholder_text = self.placeholder.clone();
        let text_color = if display_placeholder {
            TEXT_PLACEHOLDER
        } else {
            TEXT_PRIMARY
        };

        let paragraph_body = {
            let editor = editable.editor().read();
            let chips = chip_map.read();
            if display_placeholder {
                ParagraphBody::Placeholder(placeholder_text.unwrap().to_string())
            } else if editor.has_preedit() {
                let (before, preedit, after) = editor.preedit_text_segments();
                ParagraphBody::Preedit {
                    before,
                    preedit,
                    after,
                }
            } else {
                ParagraphBody::Chips {
                    rope: editor.rope().to_string(),
                    chips: chips.clone(),
                }
            }
        };

        // Show the popup whenever autocomplete is active, even if the
        // provider returned no matches — a visible "sin sugerencias"
        // row confirms to the user that the `[` trigger fired.
        let show_popup = ac_state.is_some();
        let popup_selected = ac_state.map(|s| s.selected).unwrap_or(0);
        let popup_trigger_at = ac_state.map(|s| s.trigger_at).unwrap_or(0);
        if show_popup {
            log::debug!(
                "bookmark_body_editor: popup render — trigger_at={popup_trigger_at}, \
                 selected={popup_selected}, entries={}",
                ac_filtered.len()
            );
        }

        // Caret-anchored popup Y offset. The popup wrapper naturally
        // sits right below the paragraph; shifting its absolute child
        // by `(caret_bottom - paragraph_height + gap)` moves the
        // popup up to the current caret's baseline for multi-line
        // bodies. Fallback = 0 (below-paragraph) until the first
        // layout pass has populated the paragraph holder.
        let popup_top_offset: f32 = if show_popup {
            let text = editable.editor().read().rope().to_string();
            let cursor_pos = editable.editor().read().cursor_pos();
            let holder_ref = holder.read();
            let inner_opt = holder_ref.0.borrow();
            inner_opt
                .as_ref()
                .map(|inner: &ParagraphHolderInner| {
                    let sf = inner.scale_factor as f32;
                    if sf <= 0.0 {
                        return 0.0;
                    }
                    let rect =
                        inner.paragraph.cursor_rect(&text, cursor_pos, TextAlign::Left);
                    let caret_bottom = rect.bottom / sf;
                    let paragraph_h = inner.paragraph.height() / sf;
                    caret_bottom - paragraph_h + 4.0
                })
                .unwrap_or(0.0)
        } else {
            0.0
        };

        let picked_editable = editable;
        let mut picked_chip_map = chip_map;
        let mut picked_value = self.value.clone();
        let mut picked_autocomplete = autocomplete;
        let on_pick: Callback<Completion, ()> = Callback::new(move |completion| {
            commit_completion(
                &picked_editable,
                &mut picked_chip_map,
                &mut picked_value,
                popup_trigger_at,
                completion,
            );
            picked_autocomplete.set(None);
        });

        rect()
            .a11y_id(a11y_id)
            .a11y_focusable(true)
            .a11y_role(AccessibilityRole::MultilineTextInput)
            .on_key_up(on_key_up)
            .on_key_down(on_key_down)
            .on_focus_press(on_focus_press)
            .on_ime_preedit(on_ime_preedit)
            .on_global_pointer_up(on_global_pointer_up)
            .on_global_pointer_move(on_global_pointer_move)
            .width(Size::fill())
            .min_height(Size::px(self.min_height))
            .padding((8., 10.))
            .background(SURFACE_SECONDARY)
            .border(
                Border::new()
                    .width(1.)
                    .alignment(BorderAlignment::Inner)
                    .fill(border_color),
            )
            .corner_radius(8.)
            .cursor(CursorIcon::Text)
            .child(build_paragraph(
                holder.read().clone(),
                on_paragraph_sized,
                cursor_index,
                text_selection,
                text_color,
                paragraph_body,
            ))
            .maybe_child(show_popup.then(|| {
                completion_popup(ac_filtered, popup_selected, on_pick, popup_top_offset)
            }))
    }
}

/// Snapshot of how the paragraph should render for the current frame.
/// Split out so the render body stays readable; the paragraph builder
/// consumes this.
enum ParagraphBody {
    Placeholder(String),
    Preedit {
        before: String,
        preedit: String,
        after: String,
    },
    Chips {
        rope: String,
        chips: Vec<(usize, SourceRef)>,
    },
}

fn build_paragraph(
    holder: ParagraphHolder,
    on_sized: impl FnMut(Event<SizedEventData>) + 'static,
    cursor_index: Option<usize>,
    text_selection: Option<(usize, usize)>,
    color: Color,
    body: ParagraphBody,
) -> impl IntoElement {
    let p = paragraph()
        .holder(holder)
        .on_sized(on_sized)
        .min_width(Size::percent(100.))
        .max_width(Size::percent(100.))
        .cursor_index(cursor_index)
        .cursor_color(TEXT_PRIMARY)
        .color(color)
        .highlights(text_selection.map(|h| vec![h]).unwrap_or_default());

    match body {
        ParagraphBody::Placeholder(text) => p.span(text),
        ParagraphBody::Preedit {
            before,
            preedit,
            after,
        } => p
            .span(before)
            .span(Span::new(preedit).text_decoration(TextDecoration::Underline))
            .span(after),
        ParagraphBody::Chips { rope, chips } => build_interleaved(p, &rope, &chips),
    }
}

/// Walk `rope` and interleave text runs with chip widgets. Chip
/// consumption is in-order — `chip_map` is kept 1:1 with sentinels
/// after every edit by [`sync_value`] / [`commit_completion`].
fn build_interleaved(mut p: Paragraph, rope: &str, chips: &[(usize, SourceRef)]) -> Paragraph {
    let mut buf = String::new();
    let mut chip_iter = chips.iter();
    for ch in rope.chars() {
        if ch == CHIP_SENTINEL {
            if !buf.is_empty() {
                p = p.span(std::mem::take(&mut buf));
            }
            match chip_iter.next() {
                Some((_, chip)) => p = p.child(ChipWidget { src: chip.clone() }),
                None => buf.push(' '),
            }
        } else {
            buf.push(ch);
        }
    }
    if !buf.is_empty() {
        p = p.span(buf);
    }
    p
}

/// Shell-side chip widget. Consults each plugin's
/// [`notein_plugin::Source::render_chip`] for Plugin-variant refs
/// before falling back to [`source_ref_nav::chip_label`]. Plugins
/// that override `render_chip` get a tinted pill with their own icon
/// + label; shell handles the generic Canvas / External cases.
///
/// Clicking the chip calls [`source_ref_nav::navigate`] — Canvas
/// → scroll + highlight, Plugin → open the owning viewer, External
/// → platform-default URL handler. Hover state tints the pill so
/// chips telegraph that they're actionable.
///
/// Wrapped as a `Component` (instead of a plain `fn`) because the
/// chip is instantiated inside a `for` loop in `build_interleaved`,
/// and Freya rejects hook calls in loops. Giving each chip its own
/// scope lets `use_state` live safely on the hover tint.
#[derive(Clone, PartialEq)]
struct ChipWidget {
    src: SourceRef,
}

impl std::fmt::Debug for ChipWidget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChipWidget").finish_non_exhaustive()
    }
}

impl Component for ChipWidget {
    fn render(&self) -> impl IntoElement {
        let src = self.src.clone();
        let view = resolve_chip_view(&src);
        let base_bg = view.tint.map(rgba_to_color).unwrap_or(CHIP_BG);
        let _ = view.tooltip.clone(); // reserved for a future hover popover.

        let nav_src = src.clone();

        let label_body = label()
            .font_size(11.5)
            .color(CHIP_FG)
            .text(view.label.clone());

        // Prefer a lucide SVG keyed on the ref kind — gives every chip
        // a consistent iconography (Bible = scroll, PDF = book, link =
        // chain). Falls back to the plugin-supplied emoji label when
        // the ref kind has no shell-side SVG mapping.
        let svg_icon_el = {
            use freya::icons::lucide;
            let bytes = match &src {
                SourceRef::Plugin { plugin_id, .. }
                    if plugin_id == crate::pdf_viewer::PDF_PLUGIN_ID =>
                {
                    Some(lucide::book_text())
                }
                SourceRef::Plugin { plugin_id, .. } if plugin_id == "io.notein.bible" => {
                    Some(lucide::scroll_text())
                }
                SourceRef::External { .. } => Some(lucide::link_2()),
                _ => None,
            };
            bytes.map(|b| {
                SvgViewer::new(b)
                    .color(CHIP_FG)
                    .width(Size::px(CHIP_ICON_SIZE))
                    .height(Size::px(CHIP_ICON_SIZE))
            })
        };
        let emoji_icon_el = (svg_icon_el.is_none())
            .then(|| view.icon.clone())
            .flatten()
            .map(|icon| {
                label()
                    .font_size(11.5)
                    .color(CHIP_FG)
                    .text(icon)
            });

        let mut hover = use_state(|| false);
        let bg = if hover() {
            lighten(base_bg, 0.12)
        } else {
            base_bg
        };

        // Freya requires explicit dimensions for inline paragraph
        // children (see `feature_inlined_text_elements` demo).
        // Without an explicit `height`, the placeholder rect
        // collapses and pointer events never hit.
        //
        // `on_press` won't fire reliably on a small inline chip
        // (release often lands outside the pixel-tight rect) —
        // `on_pointer_down` triggers on the press itself, which
        // matches the UX a user expects from a link-shaped chip.
        //
        // `spacing(6)` replaces the old trailing-space hack on the
        // icon label — the glyph-space combo left a visible gap on
        // chips where the fallback emoji was absent.
        rect()
            .height(Size::px(20.))
            .padding((2., 8.))
            .background(bg)
            .corner_radius(10.)
            .border(
                Border::new()
                    .width(1.)
                    .alignment(BorderAlignment::Inner)
                    .fill(SURFACE_TERTIARY),
            )
            .cursor(CursorIcon::Pointer)
            .direction(Direction::Horizontal)
            .cross_align(Alignment::center())
            .spacing(6.0)
            .on_pointer_enter(move |_| hover.set(true))
            .on_pointer_leave(move |_| hover.set_if_modified(false))
            .on_pointer_down(move |_: Event<PointerEventData>| {
                let navigates_in_app = !matches!(nav_src, SourceRef::External { .. });
                source_ref_nav::navigate(&nav_src);
                // External URL chips keep the editor open — the user
                // stays in notinplus. Every other chip hands off to
                // another route / modal, so the current bookmark
                // popup must close so it doesn't block the
                // destination.
                if navigates_in_app
                    && let Some(close) =
                        try_consume_context::<ChipTapCloseCallback>()
                {
                    (close.0)();
                }
            })
            .maybe_child(svg_icon_el)
            .maybe_child(emoji_icon_el)
            .child(label_body)
    }
}

/// Context installed by a bookmark popup / modal to let in-tree chip
/// taps close the host overlay. Wrapping a bare `Rc<dyn Fn()>` —
/// Freya state handles are `!Send`, so the close callback captures
/// one and stays on the UI thread. Context lifetime is per-mount.
#[derive(Clone)]
pub struct ChipTapCloseCallback(pub std::rc::Rc<dyn Fn()>);

impl PartialEq for ChipTapCloseCallback {
    fn eq(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for ChipTapCloseCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChipTapCloseCallback").finish_non_exhaustive()
    }
}

/// Logical pixel size of the leading SVG icon inside a chip pill.
/// Picked to sit comfortably below the 20 px chip height while still
/// reading at inline-paragraph scale.
const CHIP_ICON_SIZE: f32 = 12.0;

/// Blend `color` toward white by `t` (0 = original, 1 = white).
fn lighten(color: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let mix = |c: u8| -> u8 {
        let base = f32::from(c);
        let mixed = base + (255.0 - base) * t;
        mixed.round().clamp(0.0, 255.0) as u8
    };
    Color::from_argb(color.a(), mix(color.r()), mix(color.g()), mix(color.b()))
}

fn resolve_chip_view(src: &SourceRef) -> ChipView {
    if let SourceRef::Plugin {
        plugin_id, anchor, ..
    } = src
    {
        let reg = crate::plugins::registry();
        let reg_guard = crate::plugins::lock(&reg);
        if let Some(plugin) = reg_guard.get_source(plugin_id)
            && let Some(view) = plugin.render_chip(anchor)
        {
            return view;
        }
    }
    ChipView {
        label: source_ref_nav::chip_label(src),
        icon: None,
        tint: None,
        tooltip: None,
    }
}

fn rgba_to_color(rgba: notein_plugin::Rgba) -> Color {
    Color::from_argb(rgba[3], rgba[0], rgba[1], rgba[2])
}

/// After the rope has been mutated by `use_editable`, re-anchor the
/// chip map against the new rope and push the canonical body string
/// into the parent `Writable<String>`.
fn sync_value(
    editable: &freya_edit::UseEditable,
    chip_map: &mut State<Vec<(usize, SourceRef)>>,
    value: &mut Writable<String>,
) {
    let new_rope = editable.editor().read().committed_text();
    let rebased = rebase_chips(&chip_map.read(), &new_rope);
    let segs = from_rope_with_chips(&new_rope, &rebased);
    let serialized = serialize(&segs);
    chip_map.set(rebased);
    if *value.read() != serialized {
        *value.write() = serialized;
    }
}

/// Commit the selected completion:
///
/// 1. Remove the typed `[query` range `[trigger_at..cursor_pos)` from
///    the rope.
/// 2. Insert a chip sentinel at `trigger_at`.
/// 3. Move the cursor to `trigger_at + 1` (right after the chip).
/// 4. Insert the new chip entry at the correct position in
///    `chip_map` (sorted by UTF-16 cu idx).
/// 5. [`sync_value`] does the rest — rebase + canonical write-back.
fn commit_completion(
    editable: &freya_edit::UseEditable,
    chip_map: &mut State<Vec<(usize, SourceRef)>>,
    value: &mut Writable<String>,
    trigger_at: usize,
    completion: Completion,
) {
    let mut editable = *editable;
    {
        let mut editor = editable.editor_mut().write();
        let cursor_pos = editor.cursor_pos();
        if cursor_pos > trigger_at {
            editor.remove(trigger_at..cursor_pos);
        }
        editor.insert_char(CHIP_SENTINEL, trigger_at);
        editor.move_cursor_to(trigger_at + 1);
    }
    // Chips before the trigger stay in place; chips at/after shift
    // right by 1 (we removed `cursor_pos - trigger_at` chars then
    // inserted 1 — net shift is handled by `rebase_chips` below).
    let mut map = chip_map.read().clone();
    let insert_at = map.partition_point(|(idx, _)| *idx < trigger_at);
    map.insert(insert_at, (trigger_at, completion.source));
    chip_map.set(map);
    sync_value(&editable, chip_map, value);
}

/// Resolve the current autocomplete state + filtered completions for
/// the frame. Returns `(active_state, filtered_entries)` — `state` is
/// `None` when the popup should be hidden.
fn compute_autocomplete(
    current: Option<AutocompleteState>,
    editable: &freya_edit::UseEditable,
    provider: Option<&Callback<String, Vec<Completion>>>,
) -> (Option<AutocompleteState>, Vec<Completion>) {
    let Some(state) = current else {
        return (None, Vec::new());
    };
    let Some(provider) = provider else {
        return (Some(state), Vec::new());
    };
    let editor = editable.editor().read();
    let rope = editor.committed_text();
    let cursor = editor.cursor_pos();
    drop(editor);
    if cursor <= state.trigger_at || !trigger_char_present(&rope, state.trigger_at) {
        return (None, Vec::new());
    }
    let query = utf16_slice(&rope, state.trigger_at + 1, cursor);
    let mut filtered = provider.call(query);
    filtered.truncate(COMPLETION_LIMIT);
    let selected = if filtered.is_empty() {
        0
    } else {
        state.selected.min(filtered.len().saturating_sub(1))
    };
    (
        Some(AutocompleteState {
            trigger_at: state.trigger_at,
            selected,
        }),
        filtered,
    )
}

/// Peek the char at a given UTF-16 cu index. Returns true when it's
/// `[` — the trigger char still present means the popup should stay
/// open; anything else (user deleted it) means close.
fn trigger_char_present(rope: &str, trigger_at: usize) -> bool {
    let mut utf16_idx = 0usize;
    for ch in rope.chars() {
        if utf16_idx == trigger_at {
            return ch == '[';
        }
        utf16_idx += ch.len_utf16();
        if utf16_idx > trigger_at {
            return false;
        }
    }
    false
}

/// Slice a rope by UTF-16 code-unit range. Used to compute the
/// autocomplete query — expensive relative to a direct str slice but
/// bodies are short and the popup only runs while open.
fn utf16_slice(rope: &str, start: usize, end: usize) -> String {
    let mut out = String::new();
    let mut utf16_idx = 0usize;
    for ch in rope.chars() {
        let next = utf16_idx + ch.len_utf16();
        if utf16_idx >= end {
            break;
        }
        if utf16_idx >= start {
            out.push(ch);
        }
        utf16_idx = next;
    }
    out
}

/// Popup rendered below the editor's inner rect. Reuses the
/// `height-0 wrapper + absolute overlay child` trick from
/// `FormInput::suggestions` so the popup sits above every other
/// paragraph on the card without pushing the layout.
fn completion_popup(
    entries: Vec<Completion>,
    selected: usize,
    on_pick: Callback<Completion, ()>,
    top_offset: f32,
) -> impl IntoElement {
    let is_empty = entries.is_empty();
    let popup_height = if is_empty { 44.0 } else { 220.0 };
    let hint = label()
        .font_size(10.5)
        .color(TEXT_PLACEHOLDER)
        .max_lines(1)
        .text("↑↓ seleccionar · Enter / Tab / clic insertar · Esc cerrar");
    let body: Element = if is_empty {
        rect()
            .width(Size::fill())
            .padding((8., 10.))
            .child(
                label()
                    .font_size(12.)
                    .color(TEXT_PLACEHOLDER)
                    .text("Sin sugerencias"),
            )
            .child(hint)
            .into_element()
    } else {
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .child(
                ScrollView::new()
                    .direction(Direction::Vertical)
                    .show_scrollbar(false)
                    .height(Size::flex(1.))
                    .children(entries.into_iter().enumerate().map(|(i, c)| {
                        CompletionItem {
                            completion: c,
                            selected: i == selected,
                            on_pick: on_pick.clone(),
                            key: DiffKey::U64(i as u64),
                        }
                        .into_element()
                    })),
            )
            .child(
                rect()
                    .width(Size::fill())
                    .padding((4., 8.))
                    .child(hint),
            )
            .into_element()
    };

    rect()
        .width(Size::fill())
        .height(Size::px(0.))
        .margin((2., 0., 0., 0.))
        .child(
            rect()
                .layer(Layer::Overlay)
                .width(Size::fill())
                .position(Position::new_absolute().top(top_offset))
                .height(Size::px(popup_height))
                .overflow(Overflow::Clip)
                .corner_radius(8.)
                .background(SURFACE_PRIMARY)
                .border(
                    Border::new()
                        .width(1.)
                        .alignment(BorderAlignment::Inner)
                        .fill(BORDER),
                )
                .padding(4.)
                .child(body),
        )
}

#[derive(Clone, PartialEq)]
struct CompletionItem {
    completion: Completion,
    selected: bool,
    on_pick: Callback<Completion, ()>,
    key: DiffKey,
}

impl KeyExt for CompletionItem {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for CompletionItem {
    fn render(&self) -> impl IntoElement {
        let mut is_hover = use_state(|| false);
        let completion = self.completion.clone();
        let on_pick = self.on_pick.clone();

        let bg = if self.selected {
            PRIMARY
        } else if is_hover() {
            SURFACE_SECONDARY
        } else {
            Color::TRANSPARENT
        };
        let fg = if self.selected { TEXT_PRIMARY } else { TEXT_PRIMARY };
        let sub_fg = if self.selected { Color::from_rgb(220, 230, 250) } else { TEXT_SECONDARY };

        let primary = label()
            .font_size(13.5)
            .color(fg)
            .max_lines(1)
            .text_overflow(TextOverflow::Ellipsis)
            .text(self.completion.label.clone());
        let maybe_subtitle = self.completion.subtitle.clone().map(|s| {
            label()
                .font_size(11.)
                .color(sub_fg)
                .max_lines(1)
                .text_overflow(TextOverflow::Ellipsis)
                .text(s)
        });

        rect()
            .width(Size::fill())
            .padding((6., 10.))
            .background(bg)
            .corner_radius(6.)
            .on_pointer_enter(move |_| is_hover.set(true))
            .on_pointer_leave(move |_| is_hover.set(false))
            .on_press(move |_| on_pick.call(completion.clone()))
            .child(primary)
            .maybe_child(maybe_subtitle)
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

//! The editable paragraph behind every text block and table cell.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use freya::clipboard::Clipboard;
use freya::prelude::*;
use freya_core::elements::paragraph::{ParagraphCursorExt, ParagraphHolderInner};
use freya_edit::{
    EditableConfig, EditableEvent, EditorLine, LONG_PRESS_TIMEOUT, TextEditor as _, TextSelection,
    TouchEditEvent, use_editable,
};
use freya_engine::prelude::{
    Paint, PaintStyle, PathEffect, RectHeightStyle, RectWidthStyle, SkRect,
};

use super::image::ImageAtom;
use super::math::MathAtom;
use super::paint::{Decor, DecorKind, PaintContext, Piece};
use super::reveal::{Reveal, RevealEdit};
use super::suggest::{MENU_WIDTH, MenuRow, OpenMenu, SuggestionMenu, item, item_count};
use super::{InputContext, Shared};
use crate::controller::AtomSlot;
use crate::controller::{CaretRequest, Selection};
use crate::decorator::EmbedContext;
use crate::document::{
    Atom, BlockType, HeadingLevel, Inline, MarkFamily, MarkKind, OBJECT_REPLACEMENT, TextRole,
    TextTarget, byte_index, utf16_len,
};
use crate::popup::ReportedPopup;

/// Base look of a text block, before inline marks apply.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TextLook {
    pub(crate) font_size: Option<f32>,
    pub(crate) color: Option<Color>,
    pub(crate) weight: Option<FontWeight>,
    pub(crate) italic: bool,
    pub(crate) monospace: bool,
    pub(crate) align: TextAlign,
}

/// Keyboard behavior that depends on where the text lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextPlace {
    Block,
    ListItem,
    Cell,
}

#[derive(Clone, PartialEq)]
pub(crate) struct TextBlock {
    pub(crate) target: TextTarget,
    pub(crate) content: Inline,
    pub(crate) role: TextRole,
    pub(crate) place: TextPlace,
    pub(crate) look: TextLook,
    pub(crate) caret: Option<CaretRequest>,
    pub(crate) placeholder: Option<String>,
    pub(crate) shared: Shared,
}

impl Component for TextBlock {
    #[allow(
        clippy::too_many_lines,
        reason = "event wiring of a single editable paragraph"
    )]
    fn render(&self) -> impl IntoElement {
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);
        let holder = use_state(ParagraphHolder::default);
        let mut area = use_state(Area::default);
        // Apart from `area` so scrolling, which moves the area, re-renders
        // nothing; images size themselves to it.
        let mut width = use_state(|| 0f32);
        let mut dragging = use_state(|| false);
        let mut long_press = use_state(|| None::<TaskHandle>);
        let scroll_views = use_hook(EnclosingScrollViews::get);
        let mut menu = use_state(|| None::<OpenMenu>);
        // Title of the `<abbr>` last pressed and where, in element coordinates.
        let mut abbreviation = use_state(|| None::<(String, f32, f32)>);
        let menu_cache = use_hook(|| Rc::new(RefCell::new(None::<(String, Rc<Vec<MenuRow>>)>)));
        let applied_caret = use_hook(|| Rc::new(Cell::new(0u64)));
        let command_held = use_hook(|| Rc::new(Cell::new(false)));
        let decor = use_hook(|| Rc::new(RefCell::new(Vec::<Decor>::new())));
        let reveal = use_hook(|| Rc::new(RefCell::new(None::<Reveal>)));
        let input_shown = use_hook(|| Rc::new(RefCell::new(None::<InputContext>)));
        let initial = self.content.text().to_owned();
        let mut editable = use_editable(move || initial, EditableConfig::new);

        let shared = self.shared.clone();
        let controller = shared.controller;
        let target = self.target;
        let role = self.role;
        let place = self.place;
        let read_only = shared.read_only;

        let is_focused = focus().is_focused();
        use_side_effect(move || {
            // The probe outlives this block when a paste or a split
            // replaces it, so every state access is fallible: a dropped
            // or borrowed state means "no caret here" rather than a panic.
            let probe = Rc::new(move || {
                let editor = editable.editor().try_peek()?;
                let holder = holder.try_peek()?;
                let (top, bottom) =
                    caret_rect(&holder, &editor.rope().to_string(), editor.cursor_pos())?;
                let origin = area.try_peek()?.min_y();
                Some((origin + top, origin + bottom))
            });
            controller.track_focus(target, focus().is_focused(), probe);
        });
        use_drop(move || controller.track_focus(target, false, Rc::new(|| None)));
        let request = self
            .caret
            .filter(|request| request.seq > applied_caret.get());
        let revealing = shared.syntax_reveal && role == TextRole::Rich && !read_only && is_focused;
        let revealed = sync_rope(&mut editable, &self.content, &reveal, request, revealing);
        if let Some(request) = request {
            applied_caret.set(request.seq);
            if !a11y_id.is_focused() {
                a11y_id.request_focus();
            }
        }

        let content = self.content.clone();
        let shown = revealed.as_ref().map_or_else(
            || content.clone(),
            |revealed| revealed.display_inline(&content, shared.theme.colors.muted.into()),
        );
        let rope_text = editable.editor().read().rope().to_string();
        if shared.on_input_context.is_set() {
            if is_focused && !read_only {
                let shown = input_context(&editable);
                if input_shown.borrow().as_ref() != Some(&shown) {
                    shared.on_input_context.call(&shown);
                    *input_shown.borrow_mut() = Some(shown);
                }
            } else {
                input_shown.borrow_mut().take();
            }
        }
        let has_preedit = editable.editor().read().has_preedit();

        let elsewhere = !is_focused
            && controller.peek(super::super::controller::EditorState::focused_target)
                != Some(target);
        if menu.peek().is_some() && (elsewhere || read_only) {
            menu.set(None);
        }
        let open_menu = *menu.read();
        let cursor = editable.editor().read().cursor_pos();
        let query = open_menu.and_then(|open| menu_query(&rope_text, open.at, cursor));
        let entries = match (open_menu, query) {
            (Some(open), Some(query)) => {
                let mut cache = menu_cache.borrow_mut();
                let rows = match &*cache {
                    Some((cached, rows)) if *cached == query => rows.clone(),
                    _ => {
                        let rows = Rc::new(shared.slash.rows(
                            &query,
                            &shared.theme.labels,
                            controller.can_pick_image(),
                        ));
                        *cache = Some((query, rows.clone()));
                        rows
                    }
                };
                Some((open, rows))
            }
            _ => None,
        };

        let content_for_keys = content.clone();
        let entries_for_keys = entries.clone();
        let shared_for_keys = shared.clone();
        let command_for_keys = command_held.clone();
        let reveal_for_keys = reveal.clone();
        let input_for_keys = input_shown.clone();
        let on_key_down = move |event: Event<KeyboardEventData>| {
            let key = enter_from_text(event.key.clone());
            let modifiers = event.modifiers;
            event.stop_propagation();
            event.prevent_default();
            let command = modifiers.contains(Modifiers::ctrl_or_meta());
            command_for_keys.set(command);
            let model = |display: usize| model_offset(&reveal_for_keys, display);
            let shift = modifiers.contains(Modifiers::SHIFT);
            let alt = modifiers.contains(Modifiers::ALT);

            if let Some((open, entries)) = &entries_for_keys {
                let count = item_count(entries);
                match &key {
                    Key::Named(NamedKey::Escape) => {
                        menu.set(None);
                        return;
                    }
                    Key::Named(NamedKey::ArrowDown) if count > 0 => {
                        menu.set(Some(OpenMenu {
                            selected: (open.selected + 1) % count,
                            ..*open
                        }));
                        return;
                    }
                    Key::Named(NamedKey::ArrowUp) if count > 0 => {
                        menu.set(Some(OpenMenu {
                            selected: (open.selected + count - 1) % count,
                            ..*open
                        }));
                        return;
                    }
                    Key::Named(NamedKey::Enter | NamedKey::Tab) if count > 0 => {
                        if let Some(pick) = item(entries, open.selected.min(count - 1)) {
                            let end = editable.editor().peek().cursor_pos();
                            let action = pick.action.clone();
                            menu.set(None);
                            controller.commit_suggestion(
                                target,
                                model(open.at)..model(end),
                                action,
                            );
                        }
                        return;
                    }
                    _ => {}
                }
            }

            let intended = controller.peek(super::super::controller::EditorState::focused_target);
            if intended.is_some_and(|intended| intended != target)
                && !command
                && let Key::Character(text) = &key
            {
                controller.insert_text(text);
                return;
            }

            if !command
                && role == TextRole::Rich
                && let Key::Character(text) = &key
                && is_committed_paste(text)
                && paste_markdown(&shared_for_keys, place, text)
            {
                return;
            }

            let (selection, cursor_pos, length) = {
                let editor = editable.editor().peek();
                (
                    editor.get_selection(),
                    editor.cursor_pos(),
                    editor.len_utf16_cu(),
                )
            };
            let collapsed = selection.is_none_or(|(from, to)| from == to);
            let model_selection = selection.map(|(from, to)| (model(from), model(to)));
            let rich = role == TextRole::Rich;

            if read_only {
                let allowed = matches!(
                    &key,
                    Key::Named(
                        NamedKey::ArrowLeft
                            | NamedKey::ArrowRight
                            | NamedKey::ArrowUp
                            | NamedKey::ArrowDown
                            | NamedKey::Home
                            | NamedKey::End
                            | NamedKey::Shift
                    )
                ) || (command
                    && matches!(&key, Key::Character(ch) if ch == "c" || ch == "a"));
                if allowed {
                    editable.process_event(EditableEvent::KeyDown {
                        key: &key,
                        modifiers,
                        editor_line: Some(EditorLine::SingleParagraph),
                        holder: Some(&holder.read()),
                    });
                }
                return;
            }

            if command
                && rich
                && collapsed
                && shared_for_keys.syntax_reveal
                && let Key::Character(ch) = &key
                && let Some(delimiter) = syntax_delimiter(&ch.to_lowercase(), shift, alt)
            {
                type_delimiters(
                    &mut editable,
                    controller,
                    target,
                    &reveal_for_keys,
                    delimiter,
                );
                return;
            }

            if command && let Key::Character(ch) = &key {
                let lower = ch.to_lowercase();
                let handled = match (lower.as_str(), shift, alt) {
                    ("b", false, false) if rich => {
                        controller.toggle_mark(MarkKind::Strong);
                        true
                    }
                    ("i", false, false) if rich => {
                        controller.toggle_mark(MarkKind::Emphasis);
                        true
                    }
                    ("u", false, false) if rich => {
                        controller.toggle_mark(MarkKind::Underline);
                        true
                    }
                    ("e", false, false) if rich => {
                        controller.toggle_mark(MarkKind::Code);
                        true
                    }
                    ("x" | "s", true, false) if rich => {
                        controller.toggle_mark(MarkKind::Strikethrough);
                        true
                    }
                    ("h", true, false) if rich => {
                        controller.toggle_mark(MarkKind::Highlight);
                        true
                    }
                    (".", false, false) if rich => {
                        controller.toggle_mark(MarkKind::Superscript);
                        true
                    }
                    (",", false, false) if rich => {
                        controller.toggle_mark(MarkKind::Subscript);
                        true
                    }
                    ("\\", false, false) if rich => {
                        controller.clear_formatting();
                        true
                    }
                    ("k", false, false) if rich => {
                        link_from_clipboard(&shared_for_keys, &content_for_keys, model_selection);
                        true
                    }
                    ("k", true, false) if rich => {
                        controller.set_link(None);
                        true
                    }
                    ("z", false, _) => {
                        controller.undo();
                        true
                    }
                    ("z", true, _) | ("y", _, _) => {
                        controller.redo();
                        true
                    }
                    (digit, false, true) if rich && target.cell.is_none() => {
                        if let Some(block_type) = heading_shortcut(digit) {
                            controller.set_block_type(block_type);
                        }
                        true
                    }
                    ("c" | "x", false, false) => {
                        if let Some((from, to)) = model_selection {
                            controller.remember_copy(target, from.min(to)..from.max(to));
                        }
                        false
                    }
                    ("v", false, false) => paste(&shared_for_keys, role, place),
                    ("a", _, _) => false,
                    _ => true,
                };
                if handled {
                    return;
                }
            }

            match &key {
                Key::Named(NamedKey::Enter) if command => {
                    if target.cell.is_none() {
                        controller.exit_block(target, false);
                    }
                    return;
                }
                Key::Named(NamedKey::Enter)
                    if !shift
                        && !rich
                        && target.cell.is_none()
                        && collapsed
                        && cursor_pos == length
                        && (length == 0 || content_for_keys.text().ends_with('\n')) =>
                {
                    controller.exit_block(target, true);
                    return;
                }
                Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown)
                    if command && shift && target.cell.is_none() =>
                {
                    let delta = if key == Key::Named(NamedKey::ArrowUp) {
                        -1
                    } else {
                        1
                    };
                    controller.move_current_block(delta);
                    return;
                }
                Key::Named(NamedKey::Enter) if !shift && rich => {
                    if place == TextPlace::Cell {
                        controller.step(target, true);
                    } else {
                        let caret = model(cursor_pos);
                        let range = model_selection
                            .map_or(caret..caret, |(from, to)| from.min(to)..from.max(to));
                        controller.split(target, range);
                    }
                    return;
                }
                Key::Named(NamedKey::Backspace) if collapsed && cursor_pos == 0 && !command => {
                    controller.join_backward(target);
                    return;
                }
                Key::Named(NamedKey::Delete) if collapsed && cursor_pos == length && !command => {
                    controller.join_forward(target);
                    return;
                }
                Key::Named(NamedKey::Tab) => {
                    match place {
                        TextPlace::Cell => controller.next_cell(target, shift),
                        TextPlace::ListItem if shift => controller.outdent(),
                        TextPlace::ListItem => controller.indent(),
                        TextPlace::Block if role == TextRole::Plain && !shift => {
                            controller.insert_text("    ");
                        }
                        TextPlace::Block => {}
                    }
                    return;
                }
                Key::Named(NamedKey::Escape) => {
                    a11y_id.request_unfocus();
                    return;
                }
                Key::Named(NamedKey::ArrowLeft) if collapsed && cursor_pos == 0 && !shift => {
                    controller.step(target, false);
                    return;
                }
                Key::Named(NamedKey::ArrowRight) if collapsed && cursor_pos == length && !shift => {
                    let escaped = rich
                        && reveal_for_keys.borrow().is_none()
                        && controller.escape_marks(target, cursor_pos);
                    if !escaped {
                        controller.step(target, true);
                    }
                    return;
                }
                Key::Named(NamedKey::ArrowUp) if !shift => {
                    let first_line = editable
                        .editor()
                        .peek()
                        .visual_line_position(&holder.read(), EditorLine::SingleParagraph, -1)
                        .is_none();
                    if first_line && controller.step(target, false) {
                        return;
                    }
                }
                Key::Named(NamedKey::ArrowDown) if !shift => {
                    let last_line = editable
                        .editor()
                        .peek()
                        .visual_line_position(&holder.read(), EditorLine::SingleParagraph, 1)
                        .is_none();
                    if last_line && controller.step(target, true) {
                        return;
                    }
                }
                _ => {}
            }

            let opening = match &key {
                Key::Character(ch)
                    if !command && ch == "/" && shared_for_keys.slash.is_enabled() =>
                {
                    let text = editable.editor().peek().rope().to_string();
                    let start = selection.map_or(cursor_pos, |(from, to)| from.min(to));
                    let before = text[..byte_index(&text, start)].chars().next_back();
                    before.is_none_or(char::is_whitespace).then(|| {
                        let (top, left) = menu_anchor(&holder.read(), &text, start);
                        let width = area.peek().width();
                        OpenMenu {
                            at: start,
                            selected: 0,
                            top,
                            left: left.min(width - MENU_WIDTH).max(0.),
                        }
                    })
                }
                _ => None,
            };

            let hint = selection.map_or(cursor_pos, |(from, to)| from.min(to));
            editable.process_event(EditableEvent::KeyDown {
                key: &key,
                modifiers,
                editor_line: Some(EditorLine::SingleParagraph),
                holder: Some(&holder.read()),
            });
            commit_rope(&editable, controller, target, hint, &reveal_for_keys);
            // The input method made this edit itself, so it already knows it.
            if !command
                && matches!(
                    key,
                    Key::Character(_) | Key::Named(NamedKey::Backspace | NamedKey::Delete)
                )
            {
                *input_for_keys.borrow_mut() = Some(input_context(&editable));
            }

            if let Some(open) = opening {
                menu.set(Some(open));
            } else if menu.peek().is_some() {
                let editor = editable.editor().peek();
                let text = editor.rope().to_string();
                let still_open = menu
                    .peek()
                    .and_then(|open| menu_query(&text, open.at, editor.cursor_pos()))
                    .is_some();
                drop(editor);
                if !still_open {
                    menu.set(None);
                }
            }
        };

        let command_for_up = command_held.clone();
        let on_key_up = move |event: Event<KeyboardEventData>| {
            event.stop_propagation();
            command_for_up.set(event.modifiers.contains(Modifiers::ctrl_or_meta()));
            editable.process_event(EditableEvent::KeyUp { key: &event.key });
        };

        let on_ime_preedit = move |event: Event<ImePreeditEventData>| {
            if read_only {
                return;
            }
            let mut editor = editable.editor_mut().write();
            if event.data().text.is_empty() {
                editor.clear_preedit();
            } else {
                editor.set_preedit(&event.data().text);
            }
        };

        let reveal_for_ime = reveal.clone();
        let input_for_ime = input_shown;
        let on_ime_edit = move |event: Event<ImeEditEventData>| {
            event.stop_propagation();
            if read_only {
                return;
            }
            let hint = {
                let editor = editable.editor().peek();
                editor
                    .get_selection()
                    .map_or_else(|| editor.cursor_pos(), |(from, to)| from.min(to))
            };
            match *event.data() {
                ImeEditEventData::DeleteSurrounding {
                    before_bytes,
                    after_bytes,
                } => {
                    editable
                        .editor_mut()
                        .write()
                        .delete_surrounding(before_bytes, after_bytes);
                    commit_rope(&editable, controller, target, hint, &reveal_for_ime);
                }
                ImeEditEventData::Selection {
                    cursor_bytes,
                    anchor_bytes,
                } => {
                    editable
                        .editor_mut()
                        .write()
                        .move_selection_by_bytes(cursor_bytes, anchor_bytes);
                    controller.set_selection(rope_selection(
                        &editable,
                        target,
                        reveal_for_ime.borrow().as_ref(),
                    ));
                }
            }
            // The input method made this change itself, so it already knows it.
            *input_for_ime.borrow_mut() = Some(input_context(&editable));
        };

        let content_for_press = shown.clone();
        let reveal_for_press = reveal.clone();
        let shared_for_press = shared.clone();
        let command_for_press = command_held;
        let on_focus_press = move |event: Event<FocusPressEventData>| {
            let location = event.element_location();
            let link = link_at(&holder.read(), location, &content_for_press);
            #[allow(clippy::cast_possible_truncation, reason = "element coordinates")]
            abbreviation.set_if_modified(
                abbreviation_at(&holder.read(), location, &content_for_press)
                    .map(|title| (title, location.x as f32, location.y as f32)),
            );
            // Read only text is not a field: no focus, caret or selection,
            // and presses outside links reach whatever lies below.
            if !read_only && let Some(atom) = atom_at(&holder.read(), location, &content_for_press)
            {
                event.stop_propagation();
                event.prevent_default();
                shared_for_press
                    .controller
                    .open_atom_menu(Some(AtomSlot { target, atom }));
                return;
            }
            if read_only {
                if let Some(url) = link {
                    event.stop_propagation();
                    event.prevent_default();
                    shared_for_press.on_link.call(&url);
                }
                return;
            }
            if let FocusPressEventData::Touch(_) = event.data() {
                // Touches edit the text when the finger lifts, see `on_global_pointer_up`. The
                // press keeps going up so the note can scroll with it.
                if content_for_press.is_empty() {
                    editable.editor_mut().write().move_cursor_to(0);
                }
                a11y_id.request_focus();
                menu.set_if_modified(None);
                return;
            }
            event.stop_propagation();
            event.prevent_default();
            if let Some(url) = link.filter(|_| command_for_press.get()) {
                shared_for_press.on_link.call(&url);
                return;
            }
            dragging.set_if_modified(true);
            if content_for_press.is_empty() {
                editable.editor_mut().write().move_cursor_to(0);
            } else {
                editable.process_event(EditableEvent::Down {
                    location,
                    editor_line: EditorLine::SingleParagraph,
                    holder: &holder.read(),
                });
            }
            a11y_id.request_focus();
            menu.set_if_modified(None);
            controller.set_selection(rope_selection(
                &editable,
                target,
                reveal_for_press.borrow().as_ref(),
            ));
        };

        // Touch editing as on mobile platforms, see `freya_edit::TouchSelection`.
        let content_for_touch = shown.clone();
        let reveal_for_touch = reveal.clone();
        // A touch handler of its own: the pointer down slot belongs to `on_focus_press`.
        let on_touch_start = move |event: Event<TouchEventData>| {
            if read_only || content_for_touch.is_empty() {
                return;
            }
            let location = event.element_location;
            // An image or a formula opens its menu instead, see `on_focus_press`:
            // the finger lifting there must not raise the keyboard.
            if atom_at(&holder.read(), location, &content_for_touch).is_some() {
                return;
            }
            editable.process_touch(TouchEditEvent::Down { location });
            let scroll_views = scroll_views.clone();
            let reveal = reveal_for_touch.clone();
            let task = spawn(async move {
                timer(LONG_PRESS_TIMEOUT).await;
                editable.process_touch(TouchEditEvent::LongPress {
                    location,
                    editor_line: EditorLine::SingleParagraph,
                    holder: &holder.read(),
                });
                if editable.touch().peek().is_selecting() {
                    scroll_views.take_over_drag();
                    a11y_id.request_focus();
                    controller.set_selection(rope_selection(
                        &editable,
                        target,
                        reveal.borrow().as_ref(),
                    ));
                }
            });
            if let Some(previous) = long_press.write().replace(task) {
                previous.cancel();
            }
        };

        let reveal_for_move = reveal.clone();
        let on_global_pointer_move = move |event: Event<PointerEventData>| {
            if let PointerEventData::Touch(_) = event.data() {
                let touch = editable.touch().peek().clone();
                // Handles report their own moves.
                if !touch.is_active() || touch.dragged_handle().is_some() {
                    return;
                }
                let origin = area.read().to_f64();
                editable.process_touch(TouchEditEvent::Move {
                    location: (event.global_location() - origin.min()).to_point(),
                    editor_line: EditorLine::SingleParagraph,
                    holder: &holder.read(),
                });
                if !editable.touch().peek().is_pressed()
                    && let Some(task) = long_press.write().take()
                {
                    task.cancel();
                }
                if editable.touch().peek().is_selecting() {
                    controller.set_selection(rope_selection(
                        &editable,
                        target,
                        reveal_for_move.borrow().as_ref(),
                    ));
                }
                return;
            }
            if a11y_id.is_focused() && *dragging.read() {
                let origin = area.read().to_f64();
                let location = (event.global_location() - origin.min()).to_point();
                editable.process_event(EditableEvent::Move {
                    location,
                    editor_line: EditorLine::SingleParagraph,
                    holder: &holder.read(),
                });
            }
        };

        let reveal_for_up = reveal.clone();
        let on_global_pointer_up = move |event: Event<PointerEventData>| {
            if let PointerEventData::Touch(_) = event.data() {
                if let Some(task) = long_press.write().take() {
                    task.cancel();
                }
                let touch = editable.touch().peek().clone();
                if !touch.is_active() {
                    return;
                }
                let origin = area.read().to_f64();
                editable.process_touch(TouchEditEvent::Up {
                    location: (event.global_location() - origin.min()).to_point(),
                    editor_line: EditorLine::SingleParagraph,
                    holder: &holder.read(),
                });
                if touch.is_pressed() {
                    a11y_id.request_focus();
                }
                controller.set_selection(rope_selection(
                    &editable,
                    target,
                    reveal_for_up.borrow().as_ref(),
                ));
                return;
            }
            if *dragging.peek() {
                editable.process_event(EditableEvent::Release);
                dragging.set(false);
                controller.set_selection(rope_selection(
                    &editable,
                    target,
                    reveal_for_up.borrow().as_ref(),
                ));
            }
        };

        let on_sized = move |event: Event<SizedEventData>| {
            width.set_if_modified(event.area.width());
            area.set_if_modified(event.area);
        };

        let shared_for_toolbar = shared.clone();
        let reveal_for_toolbar = reveal.clone();
        let on_toolbar_action = move |action: TextToolbarAction| {
            let (selection, hint) = {
                let editor = editable.editor().peek();
                let selection = editor.get_selection();
                let hint = selection.map_or_else(|| editor.cursor_pos(), |(from, to)| from.min(to));
                (selection, hint)
            };
            let model = |display: usize| model_offset(&reveal_for_toolbar, display);
            if let (TextToolbarAction::Copy | TextToolbarAction::Cut, Some((from, to))) =
                (action, selection)
            {
                controller.remember_copy(target, model(from.min(to))..model(from.max(to)));
            }
            if action == TextToolbarAction::Paste && paste(&shared_for_toolbar, role, place) {
                return;
            }
            let (key, modifiers) = action.shortcut();
            editable.process_event(EditableEvent::KeyDown {
                key: &key,
                modifiers,
                editor_line: Some(EditorLine::SingleParagraph),
                holder: Some(&holder.read()),
            });
            match action {
                TextToolbarAction::Cut | TextToolbarAction::Paste => {
                    commit_rope(&editable, controller, target, hint, &reveal_for_toolbar);
                }
                TextToolbarAction::Copy | TextToolbarAction::SelectAll => {
                    controller.set_selection(rope_selection(
                        &editable,
                        target,
                        reveal_for_toolbar.borrow().as_ref(),
                    ));
                    // The toolbar stays around what was selected or copied.
                    let mut touch = editable.touch_mut().write();
                    touch.show();
                    touch.set_toolbar_visible(action == TextToolbarAction::SelectAll);
                }
            }
        };

        let theme = shared.theme.clone();
        let colors = &theme.colors;
        let base_size = self.look.font_size.unwrap_or(theme.body_size);
        let show_placeholder = content.is_empty() && !has_preedit && self.placeholder.is_some();
        let paint_context = PaintContext {
            theme: &theme,
            decorators: &shared.decorators,
            abbreviations: &shared.abbreviations,
            base_size,
            read_only,
            plain: role == TextRole::Plain,
        };

        let editor = editable.editor().read();
        let cursor_index = (is_focused && !read_only).then(|| editor.cursor_pos());
        let highlights = if is_focused {
            editor.get_visible_selection(EditorLine::SingleParagraph)
        } else {
            None
        };

        let mut paragraph = paragraph()
            .holder(holder.read().clone())
            .on_sized(on_sized)
            .width(Size::fill())
            .cursor_index(cursor_index)
            .cursor_color(colors.caret)
            .highlight_color(colors.selection)
            .highlights(highlights.map(|range| vec![range]).unwrap_or_default())
            .line_height(theme.line_height)
            .text_align(self.look.align)
            .map(self.look.font_size, freya::prelude::TextStyleExt::font_size)
            .map(self.look.color, freya::prelude::TextStyleExt::color)
            .map(self.look.weight, freya::prelude::TextStyleExt::font_weight)
            .maybe(self.look.italic, |p| p.font_slant(FontSlant::Italic))
            .maybe(self.look.monospace, |p| {
                p.font_family(theme.monospace.clone())
            });

        let mut decor_list = Vec::new();
        if show_placeholder {
            paragraph = paragraph
                .color(colors.placeholder)
                .span(self.placeholder.clone().unwrap_or_default());
        } else if editor.has_preedit() {
            let (before, preedit, after) = editor.preedit_text_segments();
            paragraph = paragraph
                .span(before)
                .span(Span::new(preedit).text_decoration(TextDecoration::Underline))
                .span(after);
        } else {
            let painted = paint_context.paint(&shown);
            decor_list = painted.decor;
            for piece in painted.pieces {
                paragraph = match piece {
                    Piece::Text(span) => paragraph.span(span),
                    Piece::Atom(index) => paragraph.child(atom_element(
                        shown.atoms().get(index),
                        AtomSlot {
                            target,
                            atom: index,
                        },
                        width(),
                        &shared,
                        base_size,
                    )),
                };
            }
        }
        drop(editor);

        let carets: Vec<RemotePaint> = shared
            .remote_carets
            .iter()
            .filter(|(at, _)| *at == target)
            .map(|(_, caret)| {
                let (anchor, head) = (caret.position.anchor, caret.position.head);
                if anchor != head {
                    decor_list.push(Decor {
                        range: anchor.min(head)..anchor.max(head),
                        kind: DecorKind::Fill(caret.color.with_a(70).into()),
                    });
                }
                RemotePaint {
                    head,
                    color: caret.color.into(),
                    name: caret.name.clone(),
                }
            })
            .collect();
        let decor_key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            decor_list.hash(&mut hasher);
            carets.hash(&mut hasher);
            hasher.finish()
        };
        let has_decor = !decor_list.is_empty() || !carets.is_empty();
        *decor.borrow_mut() = decor_list;

        let popup = entries.map(|(open, rows)| {
            let pick_rows = rows.clone();
            let reveal = reveal.clone();
            let on_pick = EventHandler::new(move |index: usize| {
                let Some(entry) = item(&pick_rows, index) else {
                    return;
                };
                let end = editable.editor().peek().cursor_pos();
                let action = entry.action.clone();
                let mut menu = menu;
                menu.set(None);
                let range = model_offset(&reveal, open.at)..model_offset(&reveal, end);
                controller.commit_suggestion(target, range, action);
            });
            rect()
                .layer(Layer::Overlay)
                .position(Position::new_absolute().top(open.top).left(open.left))
                .child(ReportedPopup {
                    child: SuggestionMenu {
                        rows,
                        selected: open.selected,
                        theme: theme.clone(),
                        on_pick,
                    }
                    .into_element(),
                })
        });

        // Tags are placed from the last layout; reading the width renders
        // again once there is one.
        let tags = if carets.is_empty() || width() <= 0. {
            Vec::new()
        } else {
            caret_tags(&holder.read(), &carets, shared.theme.px(10.5))
        };
        let painter_holder = holder.read().clone();
        let painter_decor = decor;
        let background = has_decor.then(|| {
            rect()
                .background(RenderCallback::new(move |context| {
                    paint_decor(context, &painter_holder, &painter_decor.borrow());
                    paint_carets(context, &painter_holder, &carets);
                }))
                .key(decor_key)
                .position(Position::new_absolute().top(0.).left(0.))
                .width(Size::fill())
                .height(Size::fill())
        });

        // The popup is a sibling of the editable area, not a child, so
        // presses and drags inside it never reach the text editor handlers.
        rect()
            .width(Size::fill())
            .child(
                rect()
                    .a11y_id(a11y_id)
                    .a11y_focusable(!read_only)
                    // A text input role makes the platform raise its input
                    // method; read only text is a plain paragraph.
                    .a11y_role(if read_only {
                        AccessibilityRole::Paragraph
                    } else {
                        AccessibilityRole::MultilineTextInput
                    })
                    .width(Size::fill())
                    .cursor(if read_only {
                        CursorIcon::Default
                    } else {
                        CursorIcon::Text
                    })
                    .on_key_down(on_key_down)
                    .on_key_up(on_key_up)
                    .on_ime_preedit(on_ime_preedit)
                    .on_ime_edit(on_ime_edit)
                    .on_focus_press(on_focus_press)
                    .on_touch_start(on_touch_start)
                    .on_global_pointer_move(on_global_pointer_move)
                    .on_global_pointer_up(on_global_pointer_up)
                    .maybe_child(background)
                    .child(paragraph.layer(Layer::Relative(1)))
                    .children(tags),
            )
            .maybe_child(
                abbreviation
                    .read()
                    .clone()
                    .filter(|_| read_only || is_focused)
                    .map(|(title, x, y)| abbreviation_bubble(&shared.theme, title, x, y)),
            )
            .maybe_child(popup)
            .maybe_child((is_focused && !read_only).then(|| {
                TextSelectionControls::new(editable, holder, rope_text.clone(), on_toolbar_action)
                    .paragraph_area(area())
                    .visible_area(area())
                    .text_align(self.look.align)
            }))
    }
}

/// Bring the rope in line with the model (undo, commands, autoformat) and
/// with the markdown source revealed around the caret. Returns the reveal
/// now shown.
fn sync_rope(
    editable: &mut freya_edit::UseEditable,
    content: &Inline,
    reveal: &RefCell<Option<Reveal>>,
    request: Option<CaretRequest>,
    revealing: bool,
) -> Option<Reveal> {
    let (anchor, head, has_preedit, rope_text) = {
        let editor = editable.editor().peek();
        let (anchor, head) = match editor.selection() {
            TextSelection::Cursor(position) => (*position, *position),
            TextSelection::Range { from, to } => (*from, *to),
        };
        (anchor, head, editor.has_preedit(), editor.committed_text())
    };
    let previous = reveal.borrow().clone();
    if has_preedit {
        return previous;
    }
    let to_model = |display: usize| previous.as_ref().map_or(display, |r| r.to_model(display));
    let len = content.len();
    let (model_anchor, model_head) = request.map_or_else(
        || (to_model(anchor), to_model(head)),
        |request| (request.selection.anchor, request.selection.head),
    );
    let (model_anchor, model_head) = (model_anchor.min(len), model_head.min(len));

    let keep = request.is_none()
        && previous
            .as_ref()
            .is_some_and(|r| r.is_current(content) && r.contains(anchor) && r.contains(head));
    let next = if !revealing {
        None
    } else if keep {
        previous.clone()
    } else if model_anchor == model_head {
        Reveal::open(content, model_head)
    } else {
        None
    };
    let expected = next
        .as_ref()
        .map_or_else(|| content.text().to_owned(), |r| r.display_text(content));
    let stale = rope_text != expected;
    let overflow = head.max(anchor) > utf16_len(&expected);
    if stale || overflow || request.is_some() || next != previous {
        let to_display = |model: usize| next.as_ref().map_or(model, |r| r.to_display(model));
        let mut editor = editable.editor_mut().write();
        if stale {
            editor.set(&expected);
            editor.editor_history_mut().clear();
        }
        let (anchor, head) = (to_display(model_anchor), to_display(model_head));
        if anchor == head {
            editor.clear_selection();
            editor.move_cursor_to(head);
        } else {
            editor.set_selection((anchor, head));
        }
    }
    reveal.borrow_mut().clone_from(&next);
    next
}

/// Markdown delimiter typed by a formatting shortcut while syntax is shown.
fn syntax_delimiter(key: &str, shift: bool, alt: bool) -> Option<&'static str> {
    Some(match (key, shift, alt) {
        ("b", false, false) => "**",
        ("i", false, false) => "*",
        ("e", false, false) => "`",
        ("x" | "s", true, false) => "~~",
        ("h", true, false) => "==",
        (".", false, false) => "^",
        (",", false, false) => "~",
        _ => return None,
    })
}

/// Formatting shortcut with a bare caret while syntax is shown: type the
/// delimiter pair around the caret, or remove an empty pair it sits in.
/// A fresh pair only lives on screen until something is typed inside it.
fn type_delimiters(
    editable: &mut freya_edit::UseEditable,
    controller: crate::EditorController,
    target: TextTarget,
    reveal: &RefCell<Option<Reveal>>,
    delimiter: &str,
) {
    let (text, cursor) = {
        let editor = editable.editor().peek();
        (editor.committed_text(), editor.cursor_pos())
    };
    let at = byte_index(&text, cursor);
    let width = utf16_len(delimiter);
    let inside_empty_pair = text[..at].ends_with(delimiter) && text[at..].starts_with(delimiter);
    let active = reveal.borrow().clone();

    if active
        .as_ref()
        .is_some_and(|active| active.contains(cursor))
    {
        let (edited, caret) = if inside_empty_pair {
            let start = at - delimiter.len();
            let end = at + delimiter.len();
            (
                format!("{}{}", &text[..start], &text[end..]),
                cursor - width,
            )
        } else {
            (
                format!("{}{delimiter}{delimiter}{}", &text[..at], &text[at..]),
                cursor + width,
            )
        };
        let hint = if inside_empty_pair {
            cursor - width
        } else {
            cursor
        };
        let mut editor = editable.editor_mut().write();
        editor.set(&edited);
        editor.move_cursor_to(caret);
        drop(editor);
        commit_rope(editable, controller, target, hint, reveal);
        return;
    }

    let Some(content) = controller.peek_inline(target) else {
        return;
    };
    let model = active.map_or(cursor, |active| active.to_model(cursor));
    let pair = Reveal::delimiters(model.min(content.len()), delimiter);
    let shown = pair.display_text(&content);
    *reveal.borrow_mut() = Some(pair);
    let mut editor = editable.editor_mut().write();
    editor.set(&shown);
    editor.move_cursor_to(model + width);
}

/// Model offset of a position in the rope.
fn model_offset(reveal: &RefCell<Option<Reveal>>, display: usize) -> usize {
    reveal
        .borrow()
        .as_ref()
        .map_or(display, |reveal| reveal.to_model(display))
}

fn rope_selection(
    editable: &freya_edit::UseEditable,
    target: TextTarget,
    reveal: Option<&Reveal>,
) -> Selection {
    let to_model = |display: usize| reveal.map_or(display, |reveal| reveal.to_model(display));
    let editor = editable.editor().peek();
    match editor.selection() {
        TextSelection::Cursor(position) => Selection::caret(target, to_model(*position)),
        TextSelection::Range { from, to } => Selection {
            target,
            anchor: to_model(*from),
            head: to_model(*to),
        },
    }
}

/// Push the rope text and selection into the controller, re-parsing the
/// revealed markdown source when the edit touched it.
fn commit_rope(
    editable: &freya_edit::UseEditable,
    controller: crate::EditorController,
    target: TextTarget,
    hint: usize,
    reveal: &RefCell<Option<Reveal>>,
) {
    let text = editable.editor().peek().committed_text();
    let active = reveal.borrow().clone();
    let Some(active) = active else {
        let selection = rope_selection(editable, target, None);
        controller.edit_text(target, &text, hint, selection);
        return;
    };
    let Some(content) = controller.peek_inline(target) else {
        return;
    };
    if !active.is_current(&content) {
        *reveal.borrow_mut() = None;
        return;
    }
    match active.edit(&content, &text, hint, &controller.parse_options()) {
        None => controller.set_selection(rope_selection(editable, target, Some(&active))),
        Some(RevealEdit::Outside {
            model_text,
            reveal: moved,
        }) => {
            let selection = rope_selection(editable, target, Some(&moved));
            *reveal.borrow_mut() = Some(moved);
            controller.edit_text(target, &model_text, active.to_model(hint), selection);
        }
        Some(RevealEdit::Source {
            content,
            reveal: next,
        }) => {
            let selection = rope_selection(editable, target, Some(&next));
            *reveal.borrow_mut() = Some(next);
            controller.replace_inline(target, content, selection);
        }
    }
}

/// Text typed after the trigger symbol, or `None` when the menu must close.
fn menu_query(text: &str, at: usize, cursor: usize) -> Option<String> {
    if cursor <= at {
        return None;
    }
    let start = byte_index(text, at);
    let end = byte_index(text, cursor);
    let mut chars = text.get(start..end)?.chars();
    chars.next()?;
    let query: String = chars.collect();
    let closes = query.contains('\n') || query.contains("  ") || query.chars().count() > 48;
    (!closes).then_some(query)
}

/// Soft keyboards commit the line break as text instead of pressing Enter.
fn enter_from_text(key: Key) -> Key {
    match key {
        Key::Character(text) if matches!(text.as_str(), "\n" | "\r" | "\r\n") => {
            Key::Named(NamedKey::Enter)
        }
        key => key,
    }
}

fn heading_shortcut(digit: &str) -> Option<BlockType> {
    Some(match digit {
        "0" => BlockType::Paragraph,
        "1" => BlockType::Heading(HeadingLevel::H1),
        "2" => BlockType::Heading(HeadingLevel::H2),
        "3" => BlockType::Heading(HeadingLevel::H3),
        "4" => BlockType::Heading(HeadingLevel::H4),
        "5" => BlockType::Heading(HeadingLevel::H5),
        "6" => BlockType::Heading(HeadingLevel::H6),
        "7" => BlockType::OrderedList,
        "8" => BlockType::BulletList,
        "9" => BlockType::TaskList,
        _ => return None,
    })
}

/// Ctrl+K: link the selection to the URL in the clipboard, or to itself when
/// the selected text already is a URL.
fn link_from_clipboard(shared: &Shared, content: &Inline, selection: Option<(usize, usize)>) {
    let looks_like_url = |text: &str| {
        let text = text.trim();
        !text.contains(char::is_whitespace) && (text.contains("://") || text.starts_with("mailto:"))
    };
    let selected =
        selection.map(|(from, to)| content.slice(from.min(to)..from.max(to)).plain_text());
    let url = Clipboard::get()
        .ok()
        .filter(|text| looks_like_url(text))
        .or_else(|| selected.filter(|text| looks_like_url(text)));
    if let Some(url) = url {
        shared.controller.set_link(Some(url.trim().to_owned()));
    }
}

/// Ctrl+V. Returns whether the paste was handled here (rich content) or
/// should fall through to the plain text rope.
fn paste(shared: &Shared, role: TextRole, place: TextPlace) -> bool {
    if role != TextRole::Rich {
        return false;
    }
    let Ok(text) = Clipboard::get() else {
        return true;
    };
    if let Some(rich) = shared.controller.recall_copy(&text) {
        shared.controller.insert_inline(rich);
        return true;
    }
    paste_markdown(shared, place, &text)
}

/// Insert `text` as parsed markdown when it looks like markdown.
fn paste_markdown(shared: &Shared, place: TextPlace, text: &str) -> bool {
    let markdown_like = text.contains('\n')
        || ["**", "__", "`", "](", "~~", "==", "<span"]
            .iter()
            .any(|marker| text.contains(marker));
    if !markdown_like {
        return false;
    }
    if place == TextPlace::Cell {
        shared.controller.insert_markdown(&text.replace('\n', " "));
    } else {
        shared.controller.insert_markdown(text);
    }
    true
}

/// Soft keyboards paste by committing the whole clipboard as typed text.
fn is_committed_paste(text: &str) -> bool {
    let count = text.chars().count();
    (count > 1 && text.trim_end_matches(['\r', '\n']).contains('\n'))
        || (count > 4 && (text.contains("**") || text.contains("](")))
}

fn link_at(holder: &ParagraphHolder, location: CursorPoint, content: &Inline) -> Option<String> {
    match mark_at_point(holder, location, content, MarkFamily::Link)? {
        MarkKind::Link(link) => Some(link.url),
        _ => None,
    }
}

/// Title of the `<abbr>` under `location`.
fn abbreviation_at(
    holder: &ParagraphHolder,
    location: CursorPoint,
    content: &Inline,
) -> Option<String> {
    match mark_at_point(holder, location, content, MarkFamily::Abbreviation)? {
        MarkKind::Abbreviation(title) => Some(title).filter(|title| !title.is_empty()),
        _ => None,
    }
}

/// Mark of `family` covering the character under `location`.
fn mark_at_point(
    holder: &ParagraphHolder,
    location: CursorPoint,
    content: &Inline,
    family: MarkFamily,
) -> Option<MarkKind> {
    let location = holder.visible_location(location);
    let inner = holder.0.borrow();
    let ParagraphHolderInner {
        paragraph,
        scale_factor,
        ..
    } = inner.as_ref()?;
    let index =
        paragraph.cursor_index_at_point((location.x * scale_factor, location.y * scale_factor));
    [index, index.saturating_sub(1)]
        .into_iter()
        .find_map(|pos| content.mark_at(pos, family).map(|mark| mark.kind.clone()))
}

/// `(top, left)` in paragraph coordinates right under the caret at
/// `cursor`, used to place the suggestion menu once when it opens.
fn menu_anchor(holder: &ParagraphHolder, text: &str, cursor: usize) -> (f32, f32) {
    let inner = holder.0.borrow();
    inner.as_ref().map_or((24.0, 0.0), |inner| {
        #[allow(clippy::cast_possible_truncation, reason = "scale factors are small")]
        let scale = inner.scale_factor as f32;
        if scale <= 0.0 {
            return (24.0, 0.0);
        }
        let rect = inner.paragraph.cursor_rect(text, cursor, TextAlign::Left);
        (rect.bottom / scale + 4.0, rect.left / scale)
    })
}

/// What the input method should see of this field: the shown text and the
/// selection in byte offsets.
fn input_context(editable: &freya_edit::UseEditable) -> InputContext {
    let editor = editable.editor().peek();
    let text = editor.rope().to_string();
    let cursor = editor.cursor_pos();
    let anchor = editor
        .get_selection()
        .map_or(cursor, |(from, to)| if from == cursor { to } else { from });
    InputContext {
        cursor: byte_index(&text, cursor),
        anchor: byte_index(&text, anchor),
        text,
    }
}

/// Top and bottom edge of the caret at `cursor`, relative to the paragraph.
fn caret_rect(holder: &ParagraphHolder, text: &str, cursor: usize) -> Option<(f32, f32)> {
    let inner = holder.0.borrow();
    let inner = inner.as_ref()?;
    #[allow(clippy::cast_possible_truncation, reason = "scale factors are small")]
    let scale = inner.scale_factor as f32;
    if scale <= 0.0 {
        return None;
    }
    let rect = inner.paragraph.cursor_rect(text, cursor, TextAlign::Left);
    Some((rect.top / scale, rect.bottom / scale))
}

fn paint_decor(context: &mut FillRenderContext, holder: &ParagraphHolder, decor: &[Decor]) {
    let RenderTarget::Background { canvas, .. } = &context.target else {
        return;
    };
    let inner = holder.0.borrow();
    let Some(inner) = inner.as_ref() else {
        return;
    };
    #[allow(clippy::cast_possible_truncation, reason = "scale factors are small")]
    let scale = inner.scale_factor as f32;
    if scale <= 0.0 {
        return;
    }
    for item in decor {
        let boxes = inner.paragraph.get_rects_for_range(
            item.range.clone(),
            RectHeightStyle::Tight,
            RectWidthStyle::Tight,
        );
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        for text_box in boxes {
            let rect = text_box.rect;
            let (left, top, right, bottom) = (
                rect.left / scale,
                rect.top / scale,
                rect.right / scale,
                rect.bottom / scale,
            );
            match item.kind {
                DecorKind::Fill(color) => {
                    paint.set_style(PaintStyle::Fill);
                    paint.set_color(Color::from(color));
                    let area = SkRect::new(left - 1.5, top, right + 1.5, bottom);
                    canvas.draw_round_rect(area, 3.0, 3.0, &paint);
                }
                DecorKind::Underline(color) => {
                    paint.set_style(PaintStyle::Fill);
                    paint.set_color(Color::from(color));
                    let area = SkRect::new(left, bottom - 1.5, right, bottom - 0.5);
                    canvas.draw_rect(area, &paint);
                }
                DecorKind::Dotted(color) => {
                    paint.set_style(PaintStyle::Stroke);
                    paint.set_stroke_width(1.0);
                    paint.set_color(Color::from(color));
                    paint.set_path_effect(PathEffect::dash(&[1.5, 2.0], 0.0));
                    canvas.draw_line((left, bottom - 0.5), (right, bottom - 0.5), &paint);
                }
            }
        }
    }
}

fn atom_element(
    atom: Option<&Atom>,
    slot: AtomSlot,
    max_width: f32,
    shared: &Shared,
    base_size: f32,
) -> Element {
    let theme = &shared.theme;
    let colors = &theme.colors;
    let pill = |text: String, background: Color, color: Color| {
        rect()
            .height(Size::px((base_size + theme.px(6.0)).max(theme.px(18.0))))
            .padding((theme.px(1.), theme.px(7.)))
            .corner_radius(theme.px(6.))
            .background(background)
            .center()
            .child(
                label()
                    .font_size(base_size * 0.85)
                    .color(color)
                    .max_lines(1)
                    .text(text),
            )
            .into_element()
    };
    match atom {
        Some(Atom::Embed {
            url,
            label: text,
            title,
        }) => {
            let context = EmbedContext {
                url,
                label: text,
                title: title.as_deref(),
                read_only: shared.read_only,
                theme,
            };
            shared
                .decorators
                .render_embed(&context)
                .unwrap_or_else(|| pill(text.clone(), colors.popup_selected, colors.accent))
        }
        Some(Atom::Image { url, alt, title }) => ImageAtom {
            url: url.clone(),
            alt: alt.clone(),
            title: title.clone(),
            slot,
            max_width,
            shared: shared.clone(),
        }
        .into_element(),
        Some(Atom::FootnoteRef { label: name }) => rect()
            .height(Size::px(base_size))
            .child(
                label()
                    .font_size(base_size * 0.68)
                    .color(colors.accent)
                    .text(format!("[{name}]")),
            )
            .into_element(),
        Some(Atom::Math { tex }) => MathAtom {
            tex: tex.clone(),
            slot,
            font_size: base_size,
            max_width,
            shared: shared.clone(),
        }
        .into_element(),
        Some(Atom::Html { raw }) => match html_comment(raw) {
            Some(_) if shared.read_only => rect().into_element(),
            Some(note) => pill(
                format!("\u{1f4ac} {note}"),
                colors.muted.with_a(22),
                colors.muted,
            ),
            None => pill(raw.clone(), Color::TRANSPARENT, colors.muted),
        },
        None => pill(
            OBJECT_REPLACEMENT.to_string(),
            Color::TRANSPARENT,
            colors.muted,
        ),
    }
}

/// Text of an inline `<!-- ... -->` comment.
fn html_comment(raw: &str) -> Option<&str> {
    raw.trim()
        .strip_prefix("<!--")
        .and_then(|rest| rest.strip_suffix("-->"))
        .map(str::trim)
}

/// What an `<abbr>` stands for, shown under where it was pressed.
fn abbreviation_bubble(
    theme: &crate::theme::EditorTheme,
    title: String,
    x: f32,
    y: f32,
) -> Element {
    let colors = &theme.colors;
    rect()
        .layer(Layer::Overlay)
        .position(
            Position::new_absolute()
                .top(y + theme.px(14.))
                .left((x - theme.px(12.)).max(0.)),
        )
        .max_width(Size::px(theme.px(280.)))
        .padding((theme.px(6.), theme.px(10.)))
        .corner_radius(theme.px(8.))
        .background(colors.popup_background)
        .border(
            Border::new()
                .width(1.)
                .alignment(BorderAlignment::Inner)
                .fill(colors.border),
        )
        .shadow(
            Shadow::new()
                .y(4.)
                .blur(12.)
                .color(Color::from_argb(60, 0, 0, 0)),
        )
        .child(
            label()
                .font_size(theme.px(13.))
                .color(colors.text)
                .text(title),
        )
        .into_element()
}

/// Index into [`Inline::atoms`] of the image or formula drawn under
/// `location`.
fn atom_at(holder: &ParagraphHolder, location: CursorPoint, content: &Inline) -> Option<usize> {
    let location = holder.visible_location(location);
    let inner = holder.0.borrow();
    let ParagraphHolderInner {
        paragraph,
        scale_factor,
        ..
    } = inner.as_ref()?;
    #[allow(clippy::cast_possible_truncation, reason = "pixel coordinates")]
    let (x, y) = (
        (location.x * scale_factor) as f32,
        (location.y * scale_factor) as f32,
    );
    // Every atom is a placeholder, in order.
    paragraph
        .get_rects_for_placeholders()
        .iter()
        .position(|placeholder| {
            let rect = placeholder.rect;
            (rect.left..=rect.right).contains(&x) && (rect.top..=rect.bottom).contains(&y)
        })
        .filter(|index| {
            matches!(
                content.atoms().get(*index),
                Some(Atom::Image { .. } | Atom::Math { .. })
            )
        })
}

/// Someone else's caret in this text field.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RemotePaint {
    head: usize,
    color: u32,
    name: String,
}

/// Other people's carets: a bar in their color. Their names are
/// [`caret_tags`].
fn paint_carets(context: &mut FillRenderContext, holder: &ParagraphHolder, carets: &[RemotePaint]) {
    let RenderTarget::Background { canvas, .. } = &context.target else {
        return;
    };
    let inner = holder.0.borrow();
    let Some(inner) = inner.as_ref() else {
        return;
    };
    for caret in carets {
        let Some((x, top, bottom)) = caret_place(inner, caret.head) else {
            continue;
        };
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_style(PaintStyle::Fill);
        paint.set_color(Color::from(caret.color));
        canvas.draw_rect(SkRect::new(x - 1.0, top, x + 1.0, bottom), &paint);
    }
}

/// `(x, top, bottom)` in logical pixels of a caret at `head`, from the
/// paragraph's last layout.
fn caret_place(inner: &ParagraphHolderInner, head: usize) -> Option<(f32, f32, f32)> {
    #[allow(clippy::cast_possible_truncation, reason = "scale factors are small")]
    let scale = inner.scale_factor as f32;
    if scale <= 0.0 {
        return None;
    }
    let at = |range: std::ops::Range<usize>| {
        inner
            .paragraph
            .get_rects_for_range(range, RectHeightStyle::Max, RectWidthStyle::Tight)
            .first()
            .map(|text_box| text_box.rect)
    };
    // Before the character at `head`, else after the one before it (the
    // end of the text), else an empty field.
    let (x, top, bottom) = at(head..head + 1).map_or_else(
        || {
            head.checked_sub(1)
                .and_then(|before| at(before..head))
                .map_or_else(
                    || (0.0, 0.0, inner.paragraph.height()),
                    |rect| (rect.right, rect.top, rect.bottom),
                )
        },
        |rect| (rect.left, rect.top, rect.bottom),
    );
    Some((x / scale, top / scale, bottom / scale))
}

/// Name tags of other people's carets, above each caret. Elements rather
/// than paint, so they can rise above the text field.
fn caret_tags(holder: &ParagraphHolder, carets: &[RemotePaint], size: f32) -> Vec<Element> {
    let inner = holder.0.borrow();
    let Some(inner) = inner.as_ref() else {
        return Vec::new();
    };
    carets
        .iter()
        .filter(|caret| !caret.name.is_empty())
        .filter_map(|caret| {
            let (x, top, _) = caret_place(inner, caret.head)?;
            let height = size + 4.0;
            Some(
                rect()
                    .layer(Layer::Relative(2))
                    .position(Position::new_absolute().top(top - height).left(x - 1.0))
                    .height(Size::px(height))
                    .padding((0., 4.))
                    .corner_radius(3.)
                    .background(Color::from(caret.color))
                    .center()
                    .child(
                        label()
                            .font_size(size)
                            .font_weight(FontWeight::BOLD)
                            .color(Color::from_rgb(21, 21, 26))
                            .max_lines(1)
                            .text(caret.name.clone()),
                    )
                    .into_element(),
            )
        })
        .collect()
}

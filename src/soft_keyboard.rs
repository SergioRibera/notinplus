//! Soft keyboard on Android.
//!
//! - Note editors tell the keyboard the text around the caret
//!   ([`input_context`]), so it suggests, corrects and capitalizes from
//!   what is written instead of from an empty field.
//! - `GameActivity` opens a single-line keyboard whose action key is
//!   "Done"; while a note editor has the focus it is switched to a
//!   multi-line one so the return key types line breaks.
//! - Tapping the focused field brings back a keyboard the system hid.

use std::cell::{Cell, RefCell};

use freya::prelude::*;
use freya_markdown_editor::prelude::InputContext;

thread_local! {
    /// Note editors currently holding the focus.
    static NOTE_EDITORS: Cell<u32> = const { Cell::new(0) };
    /// Last text and selection a note editor reported.
    static NOTE_CONTEXT: RefCell<Option<InputContext>> = const { RefCell::new(None) };
}

/// Report that a note editor gained (`true`) or lost (`false`) the focus.
/// Calls must be paired.
pub fn note_editor_focus(focused: bool) {
    let (before, after) = NOTE_EDITORS.with(|count| {
        let before = count.get();
        let after = if focused {
            before + 1
        } else {
            before.saturating_sub(1)
        };
        count.set(after);
        (before, after)
    });
    if (before == 0) != (after == 0) {
        let multiline = after > 0;
        let context = if multiline {
            NOTE_CONTEXT.with(|context| context.borrow().clone())
        } else {
            NOTE_CONTEXT.with(|context| context.borrow_mut().take());
            None
        };
        android::set_layout(multiline, context.as_ref());
    }
}

/// Input context handler for note editors.
pub fn input_context(context: &InputContext) {
    NOTE_CONTEXT.with(|current| *current.borrow_mut() = Some(context.clone()));
    android::set_text(context);
}

/// Ask for the soft keyboard when a text field holds the focus but the
/// keyboard is hidden, e.g. after focusing an editor from code.
#[allow(
    clippy::missing_const_for_fn,
    reason = "only the desktop stub of the Android call is const"
)]
pub fn show() {
    android::reshow();
}

/// Pointer-up handler for the app root: show the keyboard again when a
/// text field has the focus but the keyboard was dismissed.
pub fn reshow_on_tap(_: Event<PointerEventData>) {
    let role = Platform::get().focused_accessibility_node.peek().role();
    if freya_core::accessibility::ime::is_ime_role(role) {
        android::reshow();
    }
}

#[cfg(target_os = "android")]
mod android {
    use freya_markdown_editor::prelude::InputContext;
    use winit::platform::android::activity::input::{ImeOptions, InputType, TextInputAction};
    use winit::platform::android::set_ime_surrounding_text;

    /// Switch between the multi-line note keyboard and the single-line
    /// one, then reload `context` (the field is unknown without one) so
    /// the restarted keyboard picks up the new layout.
    pub(super) fn set_layout(multiline: bool, context: Option<&InputContext>) {
        let Some(app) = istmo::android::android_app() else {
            return;
        };
        let text = InputType::TYPE_CLASS_TEXT | InputType::TYPE_TEXT_FLAG_CAP_SENTENCES;
        if multiline {
            app.set_ime_editor_info(
                text | InputType::TYPE_TEXT_FLAG_MULTI_LINE,
                TextInputAction::None,
                ImeOptions::IME_FLAG_NO_ENTER_ACTION | ImeOptions::IME_FLAG_NO_FULLSCREEN,
            );
        } else {
            app.set_ime_editor_info(
                text,
                TextInputAction::Done,
                ImeOptions::IME_FLAG_NO_FULLSCREEN,
            );
        }
        match context {
            Some(context) => {
                set_ime_surrounding_text(&app, &context.text, context.cursor, context.anchor);
            }
            None => set_ime_surrounding_text(&app, "", 0, 0),
        }
    }

    pub(super) fn set_text(context: &InputContext) {
        if let Some(app) = istmo::android::android_app() {
            set_ime_surrounding_text(&app, &context.text, context.cursor, context.anchor);
        }
    }

    pub(super) fn reshow() {
        let shown = istmo::plugins::SafeArea::acquire()
            .is_ok_and(|safe_area| safe_area.current_or_zero().ime.bottom > 0.0);
        if !shown && let Some(app) = istmo::android::android_app() {
            app.show_soft_input(true);
        }
    }
}

#[cfg(not(target_os = "android"))]
mod android {
    use freya_markdown_editor::prelude::InputContext;

    pub(super) const fn set_layout(_multiline: bool, _context: Option<&InputContext>) {}
    pub(super) const fn set_text(_context: &InputContext) {}
    pub(super) const fn reshow() {}
}

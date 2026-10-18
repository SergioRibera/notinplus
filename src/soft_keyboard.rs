//! Soft keyboard on Android.
//!
//! - `GameActivity` opens a single-line keyboard whose action key is
//!   "Done"; while a note editor has the focus it is switched to a
//!   multi-line one so the return key types line breaks.
//! - The keyboard buffer is emptied when the focus moves to another text
//!   field (winit only resets it when the keyboard turns on).
//! - Tapping the focused field brings back a keyboard the system hid.

use std::cell::Cell;

use freya::prelude::*;

thread_local! {
    /// Note editors currently holding the focus.
    static NOTE_EDITORS: Cell<u32> = const { Cell::new(0) };
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
        android::set_multiline(after > 0);
    }
}

/// Follow Freya's focus so every text field starts from an empty
/// keyboard buffer. Call once, from the app root.
pub fn use_text_field_focus() {
    let previous = use_hook(|| std::rc::Rc::new(Cell::new(None::<(AccessibilityId, bool)>)));
    use_side_effect(move || {
        let platform = Platform::get();
        let id = *platform.focused_accessibility_id.read();
        let typing = focused_text_field(&platform);
        // Turning the keyboard on already empties the buffer; only a move
        // between two text fields needs it.
        if let Some((before, was_typing)) = previous.replace(Some((id, typing)))
            && before != id
            && was_typing
            && typing
        {
            android::reset_text();
        }
    });
}

/// Pointer-up handler for the app root: show the keyboard again when a
/// text field has the focus but the keyboard was dismissed.
pub fn reshow_on_tap(_: Event<PointerEventData>) {
    if focused_text_field(&Platform::get()) {
        android::reshow();
    }
}

fn focused_text_field(platform: &Platform) -> bool {
    freya_core::accessibility::ime::is_ime_role(platform.focused_accessibility_node.peek().role())
}

#[cfg(target_os = "android")]
mod android {
    use winit::platform::android::activity::input::{ImeOptions, InputType, TextInputAction};

    pub(super) fn set_multiline(multiline: bool) {
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
        // Restarts the input connection, so an open keyboard picks up the
        // new layout. The buffer is empty right after a focus change.
        winit::platform::android::reset_ime_text(&app);
    }

    pub(super) fn reset_text() {
        if let Some(app) = istmo::android::android_app() {
            winit::platform::android::reset_ime_text(&app);
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
    pub(super) const fn set_multiline(_multiline: bool) {}
    pub(super) const fn reset_text() {}
    pub(super) const fn reshow() {}
}

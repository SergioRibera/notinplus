//! Soft keyboard on Android.
//!
//! Freya configures the keyboard from the focused field (note editors get a multi-line keyboard
//! whose return key types line breaks) and brings it back when the focused field is tapped. Note
//! editors tell it the text around the caret ([`input_context`]), so it suggests, corrects and
//! capitalizes from what is written instead of from an empty field.

use freya::prelude::*;
use freya_markdown_editor::prelude::InputContext;

/// Input context handler for note editors.
pub fn input_context(context: &InputContext) {
    Platform::get().set_ime_surrounding_text(context.text.clone(), context.cursor, context.anchor);
}

/// Ask for the soft keyboard when a text field holds the focus but the keyboard is hidden, e.g.
/// after focusing an editor from code.
#[allow(
    clippy::missing_const_for_fn,
    reason = "only the desktop stub of the Android call is const"
)]
pub fn show() {
    android::show();
}

#[cfg(target_os = "android")]
mod android {
    pub(super) fn show() {
        let shown = istmo::plugins::SafeArea::acquire()
            .is_ok_and(|safe_area| safe_area.current_or_zero().ime.bottom > 0.0);
        if !shown && let Some(app) = istmo::android::android_app() {
            app.show_soft_input(true);
        }
    }
}

#[cfg(not(target_os = "android"))]
mod android {
    pub(super) const fn show() {}
}

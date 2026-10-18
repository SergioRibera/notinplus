//! Soft keyboard flavour. `GameActivity` opens a single-line keyboard whose
//! action key is "Done"; while a note editor has the focus it is switched
//! to a multi-line one so the return key types line breaks.

use std::cell::Cell;

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
        apply(after > 0);
    }
}

#[cfg(target_os = "android")]
fn apply(multiline: bool) {
    use winit::platform::android::activity::input::{
        ImeOptions, InputType, TextInputAction, TextInputState,
    };

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
        // Restarts the input connection so an already open keyboard
        // picks up the new layout. The buffer is empty right after focus.
        app.set_text_input_state(TextInputState::default());
    } else {
        app.set_ime_editor_info(
            text,
            TextInputAction::Done,
            ImeOptions::IME_FLAG_NO_FULLSCREEN,
        );
    }
}

#[cfg(not(target_os = "android"))]
const fn apply(_multiline: bool) {}

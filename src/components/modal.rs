//! Shim — `Modal` + friends moved to `notein-ui-kit`.
//!
//! The host adds a thin wrapper installing the shell's safe-area insets and
//! pen-mask sink into `ModalChrome` context so modals inside the main app
//! behave identically while plugin-opened modals get the bare backdrop + card
//! by default.

use freya::prelude::*;

pub use notein_ui_kit::{Modal, ModalChrome, ModalController, ModalPlacement, ModalPortal};

use crate::hooks::use_safe_area_insets;
use crate::ui_mask::{UiRegion, publish_mask, set as set_mask};

/// Install the host's `ModalChrome` context so descendant modals pick
/// up the safe-area insets + the shell's pen-mask sink. Call once near
/// the app root (e.g. inside the modal portal mount).
pub fn provide_host_modal_chrome() {
    let insets = use_safe_area_insets();
    let bottom = use_memo(move || insets.read().bottom);
    let on_sized = Callback::<Area, ()>::new(|area: Area| {
        publish_mask(UiRegion::Modal, area);
    });
    let on_dismiss = Callback::<(), ()>::new(|()| {
        set_mask(UiRegion::Modal, None);
    });
    use_provide_context(move || ModalChrome {
        bottom_inset: Some(bottom),
        on_sized: Some(on_sized),
        on_dismiss: Some(on_dismiss),
    });
}

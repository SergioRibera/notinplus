//! `notinplus` — Crossplatform Note App.
//!
//! Same Rust code compiled three ways:
//!
//! - Desktop `bin`  — `src/main.rs`.
//! - Android `cdylib` linked from `NativeActivity` — `mobile_main` below.
//! - iOS `staticlib` linked into the `SwiftUI` shell — same `mobile_main`.
//!
//! The runtime is auto-wired by [`istmo::runtime`] using the plugin
//! manifests every dep's `build.rs` emits.

pub mod active_bible_sources;
pub mod active_boards;
pub mod active_pdf_sources;
pub mod app;
pub mod bible_face;
pub mod bible_install_modal;
pub mod bookmark;
pub mod bookmark_body;
pub mod bookmark_ui;
pub mod completion_catalog;
pub mod brush;
pub mod builtin;
pub mod bundle;
pub mod canvas;
pub mod components;
pub mod doc;
pub mod doc_linked;
pub mod doc_linked_prefs;
pub mod doc_op;
pub mod dynamic_plugins;
pub mod home;
pub mod hooks;
pub mod host_api;
pub mod host_overlay;
pub mod identity;
pub mod ids;
pub mod library;
pub mod linked_source;
pub mod op;
pub mod palette_popup;
pub mod pane_layout_prefs;
pub mod pdf_select_overlay;
pub mod pdf_viewer;
pub mod pen_pump;
pub mod plugin_settings;
pub mod plugins;
pub mod prefs;
pub mod render;
pub mod route;
pub mod shell_builtins;
pub mod source_ref_nav;
pub mod spatial;
pub mod ui_mask;
pub mod undo;
pub mod workspace_view;

#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
)))]
pub mod desktop;

#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
)))]
pub mod desktop_data_store;

/// Per-window identifier the app assigns to its single root window.
/// Matches [`istmo_pen::PenConfig::window_id`].
pub const WINDOW_ID: u64 = 1;

use istmo::plugins::SafeArea;
use istmo_data_store::DataStoreClient;
use istmo_file_picker::FilePickerClient;
use istmo_pen::PenClient;

istmo::runtime!(
    plugins: [PenClient, SafeArea, FilePickerClient, DataStoreClient],
);

#[cfg(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
))]
#[istmo::mobile_app]
fn mobile_main() {
    #[cfg(target_os = "android")]
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    log::info!("notinplus mobile entry point running");

    pen_pump::spawn(WINDOW_ID);
    app::run();
}

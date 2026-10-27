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

pub mod actions;
pub mod active_boards;
pub mod annotation;
#[cfg(target_os = "android")]
pub mod android_clipboard;
pub mod app;
pub mod bookmark;
pub mod brush;
pub mod builtin;
pub mod bundle;
pub mod canvas;
pub mod canvas_text;
pub mod creation;
pub mod components;
pub mod deep_link;
pub mod doc;
pub mod doc_op;
pub mod editor;
pub mod external;
pub mod files;
pub mod flows;
pub mod format;
pub mod hooks;
pub mod ink;
pub mod ids;
pub mod ink_glyph;
pub mod library;
pub mod notebooks;
pub mod pdf_select_overlay;
pub mod pdf_text;
pub mod pen_pump;
pub mod persist;
pub mod plugins;
pub mod prefs;
pub mod readable;
pub mod references;
pub mod render;
pub mod route;
pub mod soft_keyboard;
pub mod screens;
pub mod search;
pub mod selection;
pub mod session_end;
pub mod settings;
pub mod space;
pub mod spatial;
pub mod state;
pub mod scan;
pub mod sync;
pub mod text_box;
pub mod thumbnail;
pub mod ui_mask;
pub mod undo;

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

use istmo::plugins::{DeepLinks, SafeArea};
use istmo_data_store::DataStoreClient;
use istmo_file_picker::FilePickerClient;
use istmo_pen::PenClient;
use istmo_barcode_scanner::BarcodeScannerClient;
use istmo_opener::OpenerClient;
use istmo_share::ShareClient;

istmo::runtime!(
    plugins: [PenClient, SafeArea, DeepLinks, FilePickerClient, DataStoreClient, ShareClient, BarcodeScannerClient, OpenerClient],
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

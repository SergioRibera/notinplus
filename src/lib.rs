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

pub mod app;
pub mod brush;
pub mod canvas;
pub mod doc;
pub mod history;
pub mod home;
pub mod library;
pub mod palette_popup;
pub mod pen_pump;
pub mod render;
pub mod route;
pub mod spatial;
pub mod ui_mask;

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

    pen_pump::spawn(WINDOW_ID, canvas::Board::shared());
    app::run();
}

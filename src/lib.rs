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

/// Wraps a future so the profiler records it as an async span named
/// `$name`, plus one plain span per poll: shows which tasks wake, how
/// often, and when they end.
macro_rules! traced {
    ($name:expr, $fut:expr $(,)?) => {{
        static CALLSITE: ::sismo::Callsite = ::sismo::Callsite::new(
            ::sismo::__private::CallsiteKind::Span,
            $name,
            module_path!(),
            file!(),
            line!(),
        );
        ::sismo::Instrumented::new(&CALLSITE, $fut)
    }};
}

pub mod actions;
pub mod active_boards;
#[cfg(target_os = "android")]
pub mod android_clipboard;
pub mod annotation;
pub mod app;
pub mod bookmark;
pub mod brush;
pub mod builtin;
pub mod bundle;
pub mod canvas;
pub mod canvas_text;
pub mod components;
pub mod creation;
pub mod deep_link;
pub mod deferred;
pub mod diagnostics;
pub mod doc;
pub mod doc_op;
pub mod editor;
pub mod external;
pub(crate) mod failure;
pub mod files;
pub mod flows;
pub mod follow;
pub mod format;
pub mod hooks;
pub mod ids;
pub mod ink;
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
pub mod scan;
pub mod screens;
pub mod search;
pub mod selection;
pub mod session_end;
pub mod settings;
pub mod soft_keyboard;
pub mod space;
pub mod spatial;
pub mod state;
pub mod sync;
pub mod text_box;
pub mod thumbnail;
pub mod ui_mask;
pub mod undo;
pub mod update;

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

notein_i18n::catalog!();

use istmo::plugins::{DeepLinks, SafeArea, SystemInfo};
use istmo_camera::CameraClient;
use istmo_data_store::DataStoreClient;
use istmo_file_picker::FilePickerClient;
use istmo_installer::InstallerClient;
use istmo_opener::OpenerClient;
use istmo_pen::PenClient;
use istmo_share::ShareClient;

istmo::runtime!(
    plugins: [PenClient, SafeArea, DeepLinks, SystemInfo, FilePickerClient, DataStoreClient, ShareClient, CameraClient, OpenerClient, InstallerClient],
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
    diagnostics::init_logging(
        Some(Box::new(android_logger::AndroidLogger::new(
            android_logger::Config::default().with_max_level(log::LevelFilter::Info),
        ))),
        log::LevelFilter::Info,
    );
    #[cfg(not(target_os = "android"))]
    diagnostics::init_logging(None, log::LevelFilter::Off);
    log::info!("notinplus mobile entry point running");
    diagnostics::start_if_enabled();

    pen_pump::spawn(WINDOW_ID);
    app::run();
}

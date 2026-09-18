//! `notinplus` — Crossplatform Note App
//!
//! Same Rust code compiled three ways:
//!
//! - Desktop `bin`  — `src/main.rs`.
//! - Android `cdylib` linked from `NativeActivity` — `mobile_main` below.
//! - iOS `staticlib` linked into the SwiftUI shell — same `mobile_main`.

pub mod app;

#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
)))]
pub mod desktop;

// The macro auto-wires every plugin the app depends on:
//   - `istmo.toml` [[remote_override]] entries move plugins to :remote.
//   - `DEP_*_ISTMO_MANIFEST` env vars (set by each plugin's build.rs)
//     supply the fully-qualified `<Trait>Client` paths.
// Manual `plugins: [ClientA, ClientB, ...]` / `remote: [...]` sections
// still work for anything not carried by a plugin crate's manifest.
istmo::runtime!();

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

    // TODO: acquire your plugin clients here with the right config type.
    // Example (data-store):
    //
    //     use istmo_data_store::{DataStoreClient, DataStoreConfig};
    //     let cfg = DataStoreConfig::new("notinplus");
    //     let client = pollster::block_on(DataStoreClient::acquire_with(cfg))
    //         .expect("acquire data store client");

    
    // TODO: replace with the `freya` entry point of your choice.
    app::App::run_mobile();
    
}

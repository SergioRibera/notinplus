//! Desktop bootstrap: install the istmo runtime, register the pen
//! plugin host, and start the platform sample source.
//!
//! istmo-pen's `istmo.toml` sets `auto_register = false`, so the app is
//! responsible for wiring [`PenHost`] into the runtime. Every macro-
//! generated `PenClient::acquire_with` call afterwards resolves against
//! the host installed here.

use std::sync::Arc;

use istmo::{Runtime, RuntimeConfig, RuntimeInit};
use istmo_file_picker::{DesktopFilePicker, FilePickerHost};
use istmo_pen::PenHost;
use istmo_pen::backend::PenPublisherFactory;
use istmo_pen::publisher::PenPublisher;

use crate::WINDOW_ID;

/// Install the process-global runtime and the pen backend. Idempotent
/// callers should still invoke this exactly once — a second call fails
/// with `RuntimeAlreadyStarted`.
///
/// # Errors
///
/// Propagates any [`istmo::core::IstmoError`] surfaced during runtime
/// init.
pub fn install() -> Result<(), Box<dyn std::error::Error>> {
    let RuntimeInit { runtime, outbound } = Runtime::init(RuntimeConfig::inline())?;

    let publisher = PenPublisher::install(&runtime);
    publisher.register_window_id(WINDOW_ID);

    #[cfg(target_os = "linux")]
    if let Err(err) = publisher.install_libinput() {
        log::warn!(
            "libinput backend unavailable ({err}); the canvas still \
             accepts mouse input via freya event handlers"
        );
    }

    runtime.register_host(PenHost::new(PenPublisherFactory::new(Arc::clone(
        &publisher,
    ))));

    runtime.register_host(crate::desktop_data_store::host());

    let picker = DesktopFilePicker::new();
    picker.install_release_hook(&runtime);
    runtime.register_host(FilePickerHost::new(picker));

    log_storage_paths();

    // Local-hosted plugin calls short-circuit into dispatch_inbound, so
    // outbound only carries fire-and-forget frames the app doesn't emit
    // today. Drain defensively so a full channel never stalls dispatch.
    std::thread::spawn(move || while outbound.recv().is_ok() {});

    Ok(())
}

/// Dump the resolved on-disk locations to stdout + tracing so we can
/// find the persisted state without guessing. Runs once at boot.
///
/// The `istmo::path::data_dir` root is derived from
/// `ISTMO_APP_BUNDLE_ID` (baked by the app-side `istmo-build::emit`)
/// with a `CARGO_PKG_NAME` fallback that resolves to `istmo-core`
/// when the env var is not visible during path-crate compilation —
/// which is why the folder may currently read `istmo-core` instead
/// of `notinplus`. Logging the effective path avoids the mystery.
fn log_storage_paths() {
    let data_root = istmo::path::data_dir();
    let library_root = crate::library::bodies::root_dir();
    let data_store_file = data_root
        .join("data_store")
        .join(format!("{}.bin", crate::library::index::NAMESPACE));

    log::info!("istmo data_dir       : {}", data_root.display());
    log::info!("library docs / pdfs  : {}", library_root.display());
    log::info!("library index blob   : {}", data_store_file.display());
    println!("[notinplus] data_dir       : {}", data_root.display());
    println!("[notinplus] library bodies : {}", library_root.display());
    println!("[notinplus] index blob     : {}", data_store_file.display());
}

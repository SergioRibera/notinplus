//! Desktop bootstrap: install the istmo runtime, register the pen
//! plugin host, and start the platform sample source.
//!
//! istmo-pen's `istmo.toml` sets `auto_register = false`, so the app is
//! responsible for wiring [`PenHost`] into the runtime. Every macro-
//! generated `PenClient::acquire_with` call afterwards resolves against
//! the host installed here.

use std::sync::Arc;

use istmo::{Runtime, RuntimeConfig, RuntimeInit};
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

    // Local-hosted plugin calls short-circuit into dispatch_inbound, so
    // outbound only carries fire-and-forget frames the app doesn't emit
    // today. Drain defensively so a full channel never stalls dispatch.
    std::thread::spawn(move || while outbound.recv().is_ok() {});

    Ok(())
}

//! Remote diagnostics: the sismo recorder streaming spans, frames and
//! logs to a sismo viewer on the same network. Off unless the user turns
//! it on in Settings › About.

pub mod ui;

use std::path::PathBuf;

use log::{LevelFilter, Log};
use notein_i18n::tr;
use notein_plugin::UserMessage;

use crate::persist::Persisted;

/// Logs kept by the recorder: framework dumps are left out, and network
/// chatter too so streaming does not record itself.
const FILTER: &str = "info,freya_core=off,freya_winit::renderer=off,torin=off,ragnarok=off,zbus=warn,tracing=warn,zudp=warn,sismo=warn";

/// Workspace crates whose names would otherwise read as Freya (`freya*`)
/// or a third party, counted as the app in the viewer.
const APP_CRATES: [&str; 4] = [
    "notein_*",
    "freya_markdown_*",
    "freya_pdf",
    "freya_canvas_bg",
];

/// Whether a phone or tablet build: those only stream, never export.
pub const MOBILE: bool = cfg!(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
));

/// The Settings switch, kept apart from [`crate::settings::Settings`] so
/// it can be read before the interface starts.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DiagnosticsSettings {
    pub remote: bool,
}

impl Persisted for DiagnosticsSettings {
    const FILE: &'static str = "diagnostics.bin";
}

/// Why remote diagnostics could not start.
#[derive(Debug)]
pub struct StartFailure(sismo::InstallError);

impl std::fmt::Display for StartFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for StartFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl UserMessage for StartFailure {
    fn user_message(&self) -> String {
        match self.0 {
            sismo::InstallError::Net(_) => tr!("errors-diagnostics-network"),
            _ => tr!("errors-diagnostics-internal"),
        }
    }
}

/// Install the global logger: every record still reaches `inner`, and
/// the recorder keeps its own copy while remote diagnostics run.
pub fn init_logging(inner: Option<Box<dyn Log>>, inner_max: LevelFilter) {
    if let Err(err) = sismo::log::init(inner, inner_max) {
        eprintln!("logger already set: {err}");
    }
}

/// Start the recorder when the user left remote diagnostics on.
pub fn start_if_enabled() {
    if DiagnosticsSettings::load().remote
        && let Err(err) = start()
    {
        log::warn!("remote diagnostics: {err}");
    }
}

fn data_dir() -> PathBuf {
    crate::library::bodies::root_dir().join("diagnostics")
}

fn config() -> sismo::Config {
    let config = sismo::config!()
        .filter(FILTER)
        .app_crates(APP_CRATES)
        .live(sismo::LiveConfig {
            data_dir: Some(data_dir()),
            ..sismo::LiveConfig::default()
        });
    if MOBILE {
        config
    } else {
        config.dump_dir(data_dir())
    }
}

/// Start streaming; does nothing when already running.
///
/// # Errors
/// The network cannot be opened, or the recorder fails to start.
pub fn start() -> Result<(), StartFailure> {
    match sismo::install(config()) {
        Ok(_) | Err(sismo::InstallError::AlreadyInstalled) => Ok(()),
        Err(err) => Err(StartFailure(err)),
    }
}

/// Stop streaming and forget what was recorded.
pub fn stop() {
    if let Some(handle) = sismo::handle() {
        handle.shutdown();
    }
}

/// Code a viewer types the first time it pairs with this device.
#[must_use]
pub fn pairing_code() -> Option<String> {
    sismo::handle().and_then(|h| h.pairing_code())
}

/// Write what the recorder holds to a `.sismo` file in the data folder.
/// Desktop only.
///
/// # Errors
/// Nothing is recording, or the file cannot be written.
pub fn export() -> std::io::Result<PathBuf> {
    let handle = sismo::handle().ok_or_else(|| std::io::Error::other("not recording"))?;
    handle.dump_flight_recorder("export")
}

/// The folder exports land in.
#[must_use]
pub fn export_dir() -> PathBuf {
    data_dir()
}

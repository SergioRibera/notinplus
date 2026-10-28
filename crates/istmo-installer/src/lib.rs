//! Hand a downloaded installer to the system, for
//! [`istmo`](https://docs.rs/istmo).
//!
//! [`Installer::install`] takes the path of a file the app already
//! downloaded and starts the system's own installer for it:
//!
//! * Android — the package installer, through an `ACTION_VIEW` intent
//!   on a `FileProvider` URI. The file must live under the app's cache
//!   directory (`updates/`). Android asks the user to confirm; the first
//!   time it also asks to allow this app to install apps, and
//!   [`InstallError::NeedsPermission`] reports that the settings page
//!   was shown instead.
//! * Windows — [`DesktopInstaller`] runs a setup `.exe`, or an `.msi`
//!   through `msiexec`.
//! * Elsewhere — [`InstallError::Unsupported`]: the user installs the
//!   file by hand.
//!
//! The generated `IstmoPluginRegistry` registers the Android backend;
//! desktop apps register [`DesktopInstaller`] themselves.

use istmo_macros::{message, plugin};

/// Wire identifier for the installer plugin.
pub const INSTALLER_PLUGIN_ID: &str = "istmo.installer";

/// Why an installer did not start.
#[message(bincode = "::bincode", crate = "::istmo_core")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InstallError {
    /// The app may not install packages yet; the system page that
    /// grants it was opened instead. Try again once the user allows it.
    NeedsPermission,
    /// There is no file at that path.
    NotFound,
    /// This system has no installer the plugin can start.
    Unsupported,
    /// Any other platform failure, with its message.
    Backend(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsPermission => f.write_str("the app may not install packages yet"),
            Self::NotFound => f.write_str("installer file not found"),
            Self::Unsupported => f.write_str("no installer available on this system"),
            Self::Backend(msg) => write!(f, "installer backend error: {msg}"),
        }
    }
}

impl std::error::Error for InstallError {}

/// Recover the typed error from a failed [`InstallerClient::install`].
impl From<istmo_core::IstmoError> for InstallError {
    fn from(err: istmo_core::IstmoError) -> Self {
        match err {
            istmo_core::IstmoError::PluginError { bytes } => {
                bincode::decode_from_slice::<Self, _>(&bytes, bincode::config::standard())
                    .map_or_else(
                        |e| Self::Backend(format!("decode plugin error payload: {e}")),
                        |(err, _)| err,
                    )
            }
            other => Self::Backend(other.to_string()),
        }
    }
}

/// The plugin trait.
#[plugin(name = "istmo.installer", crate = "::istmo_core")]
pub trait Installer {
    /// Start the system installer for the file at `path`.
    async fn install(&self, path: String) -> Result<(), InstallError>;
}

/// Desktop backend: runs Windows setup programs, declines elsewhere.
#[derive(Debug, Clone, Copy, Default)]
pub struct DesktopInstaller;

impl DesktopInstaller {
    #[cfg(target_os = "windows")]
    fn start(path: &std::path::Path) -> Result<(), InstallError> {
        let msi = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("msi"));
        let mut cmd = if msi {
            let mut cmd = std::process::Command::new("msiexec");
            cmd.arg("/i").arg(path);
            cmd
        } else {
            std::process::Command::new(path)
        };
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(drop)
            .map_err(|e| InstallError::Backend(e.to_string()))
    }

    #[cfg(not(target_os = "windows"))]
    const fn start(_path: &std::path::Path) -> Result<(), InstallError> {
        Err(InstallError::Unsupported)
    }
}

impl Installer for DesktopInstaller {
    fn install(
        &self,
        path: String,
    ) -> impl std::future::Future<Output = Result<(), InstallError>> + Send + '_ {
        let path = std::path::PathBuf::from(path);
        let result = if path.is_file() {
            Self::start(&path)
        } else {
            Err(InstallError::NotFound)
        };
        std::future::ready(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_not_found() {
        let result = poll_ready(DesktopInstaller.install("/nonexistent/setup.exe".into()));
        assert_eq!(result, Err(InstallError::NotFound));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn only_windows_runs_installers() {
        let file = std::env::temp_dir().join("istmo-installer-test.AppImage");
        std::fs::write(&file, b"").expect("write temp file");
        let result = poll_ready(DesktopInstaller.install(file.display().to_string()));
        let _ = std::fs::remove_file(&file);
        assert_eq!(result, Err(InstallError::Unsupported));
    }

    /// The desktop future is always ready; poll it once.
    fn poll_ready<F: std::future::Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(value) => value,
            std::task::Poll::Pending => unreachable!("desktop installs never wait"),
        }
    }
}

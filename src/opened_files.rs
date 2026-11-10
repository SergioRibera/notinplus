//! Files the system asks the app to open: a double-click on a `.pdf`,
//! "Open with", a launch from another app, a later launch forwarded to
//! the running instance.
//!
//! They arrive on the istmo `Documents` queue (cold-start files
//! included) and are sent by type: a `.ntp` to the plugin review, a
//! library `.notin` to the bundle review, and documents to the import
//! sheet of the plugin that reads them. Nothing reaches the library
//! without the user confirming it.

use std::path::Path;

use freya::prelude::{Platform, WinitPlatformExt, spawn_in_window};
use istmo::plugins::{Documents, OpenedDocument};
use notein_i18n::tr;
use notein_plugin::UserMessage;
use notein_ui_kit::{Toast, ToastTone, Toasts};

use crate::files::Picked;

/// What an opened file is, by extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExternalKind {
    /// A `.ntp` plugin package.
    Plugin,
    /// A `.notin` bundle.
    Bundle,
    /// Anything a document plugin may import (PDF, Markdown, text…).
    Document,
}

impl From<&str> for ExternalKind {
    fn from(name: &str) -> Self {
        let ext = Path::new(name)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("ntp") => Self::Plugin,
            Some(ext) if ext == crate::bundle::BUNDLE_EXT => Self::Bundle,
            _ => Self::Document,
        }
    }
}

/// Listen for opened files and for launches that only want the window,
/// for as long as the app runs.
#[sismo::function("opened_files::listen")]
pub fn listen() {
    let documents = match Documents::acquire() {
        Ok(documents) => documents,
        Err(err) => {
            log::warn!("opened files unavailable: {err}");
            return;
        }
    };
    let opened = documents.stream();
    spawn_in_window(traced!("task::opened_files::listen", async move {
        loop {
            match opened.recv_async().await {
                Ok(document) => open(document).await,
                Err(istmo::IstmoError::ChannelClosed) => break,
                Err(err) => log::warn!("opened file: {err}"),
            }
        }
    }));
    let activations = documents.activations();
    spawn_in_window(traced!("task::opened_files::activations", async move {
        while activations.recv_async().await.is_ok() {
            Platform::get().focus_window(Platform::window_id());
        }
    }));
}

#[sismo::function("opened_files::open")]
async fn open(document: OpenedDocument) {
    let Some(name) = document.name() else {
        log::warn!("opened file without a name: {:?}", document.location);
        return;
    };
    let Some(path) = document.path().map(Path::to_path_buf) else {
        log::warn!("opened file {name} is not a local path");
        return;
    };
    let picked = match read(path).await {
        Ok(bytes) => Picked {
            name: name.clone(),
            bytes,
        },
        Err(err) => {
            log::warn!("read opened file {name}: {err}");
            failed(&name, &err.user_message());
            return;
        }
    };
    match ExternalKind::from(name.as_str()) {
        ExternalKind::Plugin => crate::screens::settings::review_package(&picked),
        ExternalKind::Bundle => crate::flows::transfer::review_bundle(picked).await,
        ExternalKind::Document => {
            let context = crate::flows::create::Context {
                folder: crate::screens::home::Browse::get().folder(),
                selection: Vec::new(),
                folders: Vec::new(),
            };
            if !crate::flows::create::import_file(picked, context) {
                failed(&name, &tr!("opened-files-unsupported"));
            }
        }
    }
}

/// Read a file off the UI thread.
async fn read(path: std::path::PathBuf) -> std::io::Result<Vec<u8>> {
    let (tx, rx) = flume::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send(std::fs::read(path));
    });
    rx.recv_async()
        .await
        .unwrap_or_else(|_| Err(std::io::Error::other("reader thread stopped")))
}

fn failed(name: &str, why: &str) {
    Toasts::get().show(
        Toast::new(tr!(
            "errors-headline-detail",
            "headline" => tr!("opened-files-failed", "name" => name.to_owned()),
            "detail" => why.to_owned()
        ))
        .tone(ToastTone::Error),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_by_extension() {
        assert_eq!(ExternalKind::from("Atlas.NTP"), ExternalKind::Plugin);
        assert_eq!(ExternalKind::from("estudio.notin"), ExternalKind::Bundle);
        assert_eq!(ExternalKind::from("Romanos.pdf"), ExternalKind::Document);
        assert_eq!(ExternalKind::from("apuntes.md"), ExternalKind::Document);
        assert_eq!(ExternalKind::from("sin-extension"), ExternalKind::Document);
    }
}

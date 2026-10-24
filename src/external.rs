//! Handing a URL to the operating system.

use istmo_opener::{OpenError, OpenerClient};
use notein_ui_kit::{Toast, ToastTone, Toasts};

/// Open `url` (web page, `mailto:`, `file://` folder) with the system's
/// default handler.
pub fn open_url(url: &str) {
    let url = url.to_owned();
    let client = match OpenerClient::acquire() {
        Ok(client) => client,
        Err(err) => {
            log::warn!("opener: {err}");
            return;
        }
    };
    freya::prelude::spawn_forever(async move {
        match client.open(url.clone()).await.map_err(OpenError::from) {
            Ok(()) => {}
            Err(OpenError::NoHandler) => {
                Toasts::get().show(Toast::new("Ninguna app de este dispositivo abre ese enlace").tone(ToastTone::Error));
            }
            Err(err) => log::warn!("open {url}: {err}"),
        }
    });
}

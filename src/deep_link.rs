//! `notein://` links the operating system hands to the app.
//!
//! The link format itself lives with [`SourceRef`] in the plugin
//! contract, since plugins write these links into their content.

use notein_plugin::SourceRef;
pub use notein_plugin::link::{SCHEME, scan_links};

/// Route a URL the operating system handed to the app.
#[sismo::function("deep_link::open")]
pub fn open(url: &str) {
    log::info!("deep link: {url}");
    if crate::sync::recognize_link(url).is_some() {
        crate::screens::collab::invitation(url.to_owned());
        return;
    }
    match SourceRef::from_url(url) {
        Some(src) => {
            crate::plugins::navigate::open(&src, notein_plugin::services::Placement::Preferred);
        }
        None => log::warn!("deep link ignored: {url}"),
    }
}

/// Listen for URLs delivered through the platform, cold-start intents
/// included, for as long as the app runs.
#[sismo::function("deep_link::listen")]
pub fn listen() {
    let stream = match istmo::plugins::DeepLinks::acquire() {
        Ok(plugin) => plugin.stream(),
        Err(err) => {
            log::warn!("deep links unavailable: {err}");
            return;
        }
    };
    freya::prelude::spawn_in_window(traced!("task::deep_link::listen", async move {
        loop {
            match stream.recv_async().await {
                Ok(link) => open(&link.uri),
                Err(istmo::IstmoError::ChannelClosed) => break,
                Err(err) => log::warn!("deep link: {err}"),
            }
        }
    }));
}

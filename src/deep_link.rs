//! `notein://` links the operating system hands to the app.
//!
//! The link format itself lives with [`SourceRef`] in the plugin
//! contract, since plugins write these links into their content. The
//! shell's own actions ([`AppLink`]) only arrive from the system, such
//! as the launcher shortcuts.

use notin_plugin::SourceRef;
pub use notin_plugin::link::{SCHEME, scan_links};

use crate::library::ItemId;
use crate::settings::EntryKey;

/// Documents offered in the launcher after the creation used most.
const RECENT_SHORTCUTS: usize = 3;

/// A shell action behind a link.
///
/// - `notein://create/<plugin id>/<entry id>`
/// - `notein://open/<item uuid>`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppLink {
    /// Run a creation entry, as its tile in the create menu does.
    Create(EntryKey),
    /// Open a document where the user is.
    Open(ItemId),
}

impl AppLink {
    /// The link URL of this action.
    #[must_use]
    pub fn to_url(&self) -> String {
        match self {
            Self::Create(key) => format!("{SCHEME}://create/{}/{}", key.plugin, key.entry),
            Self::Open(item) => format!("{SCHEME}://open/{item}"),
        }
    }

    /// `None` when `url` is not an action link.
    #[must_use]
    pub fn from_url(url: &str) -> Option<Self> {
        let rest = url.strip_prefix(SCHEME)?.strip_prefix("://")?;
        let (action, value) = rest.split_once('/')?;
        match action {
            "create" => {
                let (plugin, entry) = value.split_once('/')?;
                (!plugin.is_empty() && !entry.is_empty())
                    .then(|| Self::Create(EntryKey::new(plugin, entry)))
            }
            "open" => value.parse().ok().map(Self::Open),
            _ => None,
        }
    }

    #[sismo::function("deep_link::run_action")]
    fn run(self) {
        match self {
            Self::Create(key) => match crate::flows::create::entry_for(&key) {
                Some(entry) => {
                    crate::flows::create::run(entry, crate::flows::create::Context::default());
                }
                None => log::warn!("deep link: no creation entry {}/{}", key.plugin, key.entry),
            },
            Self::Open(item) => crate::flows::open(item),
        }
    }
}

/// Route a URL the operating system handed to the app.
#[sismo::function("deep_link::open")]
pub fn open(url: &str) {
    log::info!("deep link: {url}");
    if crate::sync::recognize_link(url).is_some() {
        crate::screens::collab::invitation(url.to_owned());
        return;
    }
    if let Some(action) = AppLink::from_url(url) {
        action.run();
        return;
    }
    match SourceRef::from_url(url) {
        Some(src) => {
            crate::plugins::navigate::open(&src, notin_plugin::services::Placement::Preferred);
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

/// Keep the launcher shortcuts (Android's long press on the app icon) on
/// the creation used most and the documents changed last, each opening
/// its [`AppLink`]. They follow the library and are pushed only when
/// they change.
#[sismo::function("deep_link::use_launcher_shortcuts")]
pub fn use_launcher_shortcuts() {
    use freya::prelude::{use_hook, use_side_effect};

    let published = use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(None)));
    use_side_effect(move || {
        // Only Android lists app shortcuts in its launcher.
        if !cfg!(target_os = "android") {
            return;
        }
        let Some(data) = crate::state::LibraryView::try_get().and_then(|view| view.data()) else {
            return;
        };
        let targets = launcher_shortcuts(&data);
        if published.borrow().as_ref() == Some(&targets) {
            return;
        }
        published.replace(Some(targets.clone()));
        publish_launcher_shortcuts(targets);
    });
}

#[sismo::function("deep_link::launcher_shortcuts")]
fn launcher_shortcuts(data: &crate::library::LibraryData) -> Vec<istmo_share::ShareTarget> {
    use istmo_share::ShareTarget;
    use notin_i18n::tr;

    let create = crate::flows::create::favorite_entry().map(|entry| {
        let link = AppLink::Create(EntryKey::new(entry.plugin, entry.entry.id.to_string()));
        let label = tr!("flows-create-shortcut", "label" => entry.entry.label.to_string());
        (link, label)
    });
    let recent = data.recent_items(RECENT_SHORTCUTS).into_iter().map(|item| {
        let name = if item.name.trim().is_empty() {
            tr!("common-untitled")
        } else {
            item.name.clone()
        };
        (AppLink::Open(item.id), name)
    });
    create
        .into_iter()
        .chain(recent)
        .map(|(link, label)| {
            let url = link.to_url();
            ShareTarget::new(url.clone(), label)
                .with_uri(url)
                .launcher_only()
        })
        .collect()
}

fn publish_launcher_shortcuts(targets: Vec<istmo_share::ShareTarget>) {
    freya::prelude::spawn_in_window(traced!("task::deep_link::launcher_shortcuts", async move {
        let result = match istmo_share::ShareClient::acquire() {
            Ok(client) => client
                .set_share_targets(targets)
                .await
                .map_err(istmo_share::ShareError::from),
            Err(err) => Err(istmo_share::ShareError::from(err)),
        };
        if let Err(err) = result {
            log::warn!("launcher shortcuts: {err}");
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::AppLink;
    use crate::library::ItemId;
    use crate::settings::EntryKey;

    #[test]
    fn actions_survive_their_url() {
        let links = [
            AppLink::Create(EntryKey::new("io.notein.notes", "note")),
            AppLink::Open(ItemId::from_bytes([7; 16])),
        ];
        for link in links {
            assert_eq!(AppLink::from_url(&link.to_url()), Some(link));
        }
    }

    #[test]
    fn references_are_not_actions() {
        assert_eq!(
            AppLink::from_url("notein://canvas/00000000-0000-0000-0000-000000000000"),
            None
        );
        assert_eq!(AppLink::from_url("notein://create/only-plugin"), None);
    }
}

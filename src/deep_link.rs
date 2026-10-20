//! `notein://` deep links for [`SourceRef`]s.
//!
//! Note bodies are markdown; references to other content are regular
//! markdown links whose URL uses this scheme, e.g.
//! `[Romans 8](notein://plugin/io.notein.bible/<item>/<anchor>)`. The note
//! editor renders those links as chips, and the same URLs can be handed to
//! the OS later to open the app on a given reference.
//!
//! - `notein://canvas/<bookmark uuid>`
//! - `notein://plugin/<plugin id>/<item uuid>/<anchor hex>`
//!
//! External references stay plain URLs.

use notein_plugin::{BookmarkId, SourceRef};

use crate::components::{JoinSheet, SettingsModal};
use crate::sync::LinkTarget;

pub const SCHEME: &str = "notein";
const PREFIX: &str = "notein://";

/// Conversion between a [`SourceRef`] and its link URL.
pub trait DeepLink: Sized {
    fn to_url(&self) -> String;

    /// `None` when `url` is not a well formed `notein://` link. Plain web
    /// URLs are not deep links; see [`SourceRef::from_link`].
    fn from_url(url: &str) -> Option<Self>;

    /// Any link URL as a reference: `notein://` links decode, everything
    /// else becomes [`SourceRef::External`].
    fn from_link(url: &str) -> Self;
}

impl DeepLink for SourceRef {
    fn to_url(&self) -> String {
        match self {
            Self::Canvas { bookmark } => {
                format!(
                    "{PREFIX}canvas/{}",
                    uuid::Uuid::from_bytes(*bookmark.as_bytes())
                )
            }
            Self::Plugin {
                plugin_id,
                item,
                anchor,
                ..
            } => format!(
                "{PREFIX}plugin/{plugin_id}/{}/{}",
                uuid::Uuid::from_bytes(*item),
                hex(anchor)
            ),
            Self::External { url } => url.clone(),
        }
    }

    fn from_url(url: &str) -> Option<Self> {
        let rest = url.strip_prefix(PREFIX)?;
        let (kind, value) = rest.split_once('/')?;
        match kind {
            "canvas" => {
                let id = uuid::Uuid::parse_str(value).ok()?;
                Some(Self::Canvas {
                    bookmark: BookmarkId::from_bytes(*id.as_bytes()),
                })
            }
            "plugin" => {
                let mut parts = value.rsplitn(3, '/');
                let anchor = unhex(parts.next()?)?;
                let item = uuid::Uuid::parse_str(parts.next()?).ok()?;
                let plugin_id = parts.next().filter(|id| !id.is_empty())?;
                Some(Self::Plugin {
                    plugin_id: plugin_id.to_owned(),
                    item: *item.as_bytes(),
                    anchor,
                    selection: None,
                })
            }
            _ => None,
        }
    }

    fn from_link(url: &str) -> Self {
        Self::from_url(url).unwrap_or_else(|| Self::External {
            url: url.to_owned(),
        })
    }
}

/// Every `notein://` link found anywhere in `text`, in order. Links end at
/// whitespace or at the characters markdown and HTML use to close a
/// link; anything that fails to parse is skipped.
pub fn scan_links(text: &str) -> impl Iterator<Item = SourceRef> + '_ {
    text.match_indices(PREFIX).filter_map(move |(start, _)| {
        let rest = &text[start..];
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '>' | '"' | '\''))
            .unwrap_or(rest.len());
        SourceRef::from_url(&rest[..end])
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_round_trips() {
        let source = SourceRef::Canvas {
            bookmark: BookmarkId::from_bytes([7; 16]),
        };
        let url = source.to_url();
        assert!(url.starts_with("notein://canvas/"));
        assert_eq!(SourceRef::from_url(&url), Some(source));
    }

    #[test]
    fn plugin_round_trips_with_dotted_id() {
        let source = SourceRef::Plugin {
            plugin_id: "io.notein.bible".into(),
            item: [1; 16],
            anchor: vec![0xde, 0xad, 0xbe, 0xef],
            selection: None,
        };
        let url = source.to_url();
        assert!(url.ends_with("/deadbeef"));
        assert_eq!(SourceRef::from_url(&url), Some(source));
    }

    #[test]
    fn web_links_are_external() {
        assert_eq!(SourceRef::from_url("https://x.dev"), None);
        assert_eq!(
            SourceRef::from_link("https://x.dev"),
            SourceRef::External {
                url: "https://x.dev".into()
            }
        );
    }

    #[test]
    fn malformed_links_are_rejected() {
        assert_eq!(SourceRef::from_url("notein://canvas/nope"), None);
        assert_eq!(SourceRef::from_url("notein://plugin/x/abc"), None);
        assert_eq!(SourceRef::from_url("notein://other/1"), None);
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    #[test]
    fn scan_finds_every_link_in_markdown() {
        let canvas = SourceRef::Canvas {
            bookmark: BookmarkId::from_bytes([3; 16]),
        };
        let plugin = SourceRef::Plugin {
            plugin_id: "io.notein.notes".into(),
            item: [4; 16],
            anchor: vec![1, 2],
            selection: None,
        };
        let text = format!(
            "see [pin]({}) and <{}> plus https://x.dev and notein://bogus",
            canvas.to_url(),
            plugin.to_url()
        );
        let found: Vec<SourceRef> = scan_links(&text).collect();
        assert_eq!(found, vec![canvas, plugin]);
    }
}

/// Route a URL the operating system handed to the app.
pub fn open(url: &str) {
    log::info!("deep link: {url}");
    match crate::sync::open_link(url) {
        Some(LinkTarget::Join {
            share,
            title,
            owner,
        }) => return JoinSheet::open(share, title, owner),
        Some(LinkTarget::Pair { .. }) => return SettingsModal::open(),
        None => {}
    }
    match SourceRef::from_url(url) {
        Some(src) => crate::source_ref_nav::navigate(&src),
        None => log::warn!("deep link ignored: {url}"),
    }
}

/// Listen for URLs delivered through the platform, cold-start intents
/// included, for as long as the app runs.
pub fn listen() {
    let stream = match istmo::plugins::DeepLinks::acquire() {
        Ok(plugin) => plugin.stream(),
        Err(err) => {
            log::warn!("deep links unavailable: {err}");
            return;
        }
    };
    freya::prelude::spawn_forever(async move {
        loop {
            match stream.recv_async().await {
                Ok(link) => open(&link.uri),
                Err(istmo::IstmoError::ChannelClosed) => break,
                Err(err) => log::warn!("deep link: {err}"),
            }
        }
    });
}

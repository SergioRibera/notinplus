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

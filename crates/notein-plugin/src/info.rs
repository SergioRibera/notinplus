//! Who a plugin is.

use std::borrow::Cow;

/// Identity and the user-facing description shown in Settings ›
/// Plugins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginInfo {
    /// Stable reverse-DNS id (`io.notein.bible`).
    pub id: Cow<'static, str>,
    /// Name shown to users.
    pub name: Cow<'static, str>,
    pub version: Cow<'static, str>,
    pub author: Cow<'static, str>,
    /// One sentence in plain language: what the plugin adds.
    pub description: Cow<'static, str>,
}

impl PluginInfo {
    #[must_use]
    pub fn new(id: impl Into<Cow<'static, str>>, name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: Cow::Borrowed(env!("CARGO_PKG_VERSION")),
            author: Cow::Borrowed("Notein"),
            description: Cow::Borrowed(""),
        }
    }

    #[must_use]
    pub fn version(mut self, version: impl Into<Cow<'static, str>>) -> Self {
        self.version = version.into();
        self
    }

    #[must_use]
    pub fn author(mut self, author: impl Into<Cow<'static, str>>) -> Self {
        self.author = author.into();
        self
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<Cow<'static, str>>) -> Self {
        self.description = description.into();
        self
    }
}

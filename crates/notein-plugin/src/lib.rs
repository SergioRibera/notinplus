//! Plugin surface for notinplus.
//!
//! A plugin adds document kinds to the library. Each kind is a
//! [`DocumentKind`]: it owns a typed content ([`Payload`]), an anchor
//! type that points inside that content, and the search hits it yields
//! through the `Hits<'a>` generic associated type, so a kind can borrow
//! its own index while it streams matches. A kind may come with a
//! [`Face`], the pane that shows its content, and with
//! [`CreationEntry`]s for the "new" menu.
//!
//! The shell talks to kinds through [`ErasedKind`] / [`ErasedFace`],
//! which the [`Registrar`] builds from the typed traits, and plugins
//! reach the shell through [`HostServices`] (toasts, progress,
//! annotations, jumps).
//!
//! [`PluginCatalog`] holds the installed plugins, built in or loaded
//! from a `.ntp` package by [`loader`]; [`ConfigSchema`] declares a
//! plugin's settings.
//!
//! Plugins translate their text with [`i18n`]: Fluent files under the
//! plugin crate's `locales/<lang>/*.ftl`, `notein-i18n` (feature
//! `build`) as a build-dependency whose `build::emit()` runs from
//! `build.rs`, `notein_plugin::i18n::catalog!();` at the crate root,
//! and [`tr!`] wherever text is shown. The host keeps every plugin on
//! the interface language, also inside a shared library
//! ([`export_plugin!`] exports the setter it calls).

pub mod bookmark;
pub mod catalog;
pub mod config;
pub mod create;
pub mod doc;
pub mod erased;
pub mod face;
pub mod failure;
pub mod ids;
pub mod info;
pub mod kind;
pub mod link;
pub mod loader;
pub mod manifest;
pub mod payload;
pub mod plugin;
pub mod services;
pub mod target;
pub mod text;
pub mod ui;

pub use bytes;
pub use notein_i18n as i18n;
pub use notein_i18n::tr;
pub use rkyv;

notein_i18n::catalog!();

pub use bookmark::{Bookmark, Rgba, SourceRef, StrokeAnchor, TimestampMs, WorldPoint};
pub use catalog::{CatalogEntry, CatalogError, PluginCatalog};
pub use config::{
    ConfigEnumOption, ConfigError, ConfigField, ConfigFieldKind, ConfigSchema, ConfigValue,
};
pub use create::{
    Created, CreationChoice, CreationEntry, CreationError, CreationFlow, CreationRequest,
    ImportPreview, ImportedFile, Paper,
};
pub use doc::{DocId, DocMeta};
pub use erased::ErasedKind;
pub use face::{
    ContentStore, CoverRequest, ErasedFace, Face, FaceContext, FaceRequest, HeaderAction, PaneKey,
};
pub use failure::{NetFailure, UserMessage};
pub use ids::{ActorId, BookmarkId, DeviceId, OpId, StrokeId, UserId};
pub use info::PluginInfo;
pub use kind::{
    ActionEffect, Attachment, ChipView, DocumentKind, ExportFile, Hit, IndexReport, KindAction,
    KindChoice, KindColor, KindInfo, Mark, OutlineIcon, OutlineNode, Quote, SearchQuery,
    SelectionSupport,
};
pub use loader::{LoaderError, SDK_ABI_VERSION};
pub use manifest::{ManifestError, ManifestLibrary, ManifestRequirement, PluginManifest};
pub use payload::{Payload, PayloadError};
pub use plugin::{Font, Plugin, PluginSettings, Registrar};
pub use services::{
    HeaderPress, Host, HostServices, HostToast, Jump, Peer, Presence, Progress, Records, Share,
    TextBody, ToastTone,
};
pub use target::Target;
pub use text::{Folded, fold};
pub use ui::{HostUi, SelectionMenu, Ui};

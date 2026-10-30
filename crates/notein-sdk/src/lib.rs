//! Author-facing SDK for notinplus plugins.
//!
//! Re-exports the whole [`notein_plugin`] surface. Most plugins only
//! need the [`prelude`]:
//!
//! ```ignore
//! use notein_sdk::prelude::*;
//!
//! struct Recipes;
//!
//! impl DocumentKind for Recipes {
//!     type Content = RecipeContent;
//!     type Anchor = Step;
//!     type Hits<'a> = std::vec::IntoIter<Hit<Step>>;
//!     // …
//! }
//!
//! impl Plugin for RecipesPlugin {
//!     fn info(&self) -> PluginInfo {
//!         PluginInfo::new("dev.recipes", tr!("recipes-plugin-name"))
//!     }
//!
//!     fn register(&self, registrar: &mut Registrar) {
//!         registrar.kind(Recipes);
//!     }
//! }
//! ```
//!
//! # Translations
//!
//! Text the plugin shows lives in Fluent files, one folder per
//! language (`locales/en/recipes.ftl`, `locales/es/recipes.ftl`), with
//! the same message ids in every language. Wire them with
//! `notein-i18n` as a build-dependency (feature `build`):
//!
//! ```ignore
//! // build.rs
//! fn main() {
//!     notein_i18n::build::emit();
//! }
//!
//! // lib.rs
//! notein_sdk::i18n::catalog!();
//! ```
//!
//! [`tr!`] then formats a message in the interface language, which the
//! host keeps current in the plugin. Since the language can change
//! while the app runs, build text when it is asked for (`info`,
//! `creation_entries`, `render`) rather than caching it.

pub use notein_plugin::*;

/// The traits and types a plugin implementing a kind uses.
pub mod prelude {
    pub use notein_plugin::{
        ChipView, ConfigSchema, ConfigValue, CoverRequest, Created, CreationEntry, CreationError,
        CreationRequest, DocId, DocMeta, DocumentKind, ExportFile, Face, FaceContext, Folded, Font,
        Hit, HostServices, HostUi, ImportPreview, ImportedFile, IndexReport, KindChoice, KindColor,
        KindInfo, Mark, OutlineIcon, OutlineNode, Payload, PayloadError, Plugin, PluginInfo,
        Registrar, SearchQuery, SelectionMenu, Share, SourceRef, TextBody, Ui, tr,
    };
}

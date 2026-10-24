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
//!         // id, name, version…
//!     }
//!
//!     fn register(&self, registrar: &mut Registrar) {
//!         registrar.kind(Recipes);
//!     }
//! }
//! ```

pub use notein_plugin::*;

/// The traits and types a plugin implementing a kind uses.
pub mod prelude {
    pub use notein_plugin::{
        ChipView, ConfigSchema, ConfigValue, Created, CreationEntry, CreationError, CreationRequest,
        DocId, DocMeta, DocumentKind, Face, FaceContext, Folded, Hit, HostServices, IndexReport,
        KindColor, KindInfo, Mark, OutlineIcon, OutlineNode, Payload, PayloadError, Plugin,
        PluginInfo, Registrar, SearchQuery,
    };
}

//! Shell-owned creation entries that have no plugin home yet
//! (`new-folder`, `import-pdf`).
//!
//! The three canvas entries (`new-canvas`, `new-ephemeral`,
//! `quick-sketch`) migrated to [`crate::builtin::stroke_canvas`]; their
//! stable entry ids are re-exported here so persisted FAB shortcuts and
//! `home.rs` dispatch can keep referencing one source of truth. PDF
//! import moves next.
//!
//! Dispatch (what each entry actually *does*) lives in `home.rs`
//! because it needs the live `LibHandle` + `RouterContext` — the shell
//! only models the entries here.

use notein_plugin::{CreationCategory, CreationEntry, CreationKind};

/// Pseudo-plugin id every shell-owned entry carries. Lets persisted
/// shortcut references distinguish them from real plugins without a
/// separate "source" tag.
pub const BUILTIN_PLUGIN_ID: &str = "io.notein.builtin";

/// Entry ids owned by the stroke canvas plugin — re-exported so
/// `home.rs` dispatch and the shortcut migration can refer to one
/// source of truth.
pub use crate::builtin::stroke_canvas::{NEW_CANVAS, NEW_EPHEMERAL, QUICK_SKETCH};

pub const NEW_FOLDER: &str = "new-folder";
pub const IMPORT_PDF: &str = "import-pdf";

/// Every shell-owned entry. Ordered by the natural menu grouping
/// (sources, then other) so a caller that drops the `category`
/// grouping still sees a sensible order.
#[must_use]
pub fn entries() -> Vec<CreationEntry> {
    vec![
        CreationEntry {
            id: IMPORT_PDF.to_owned(),
            label: "Importar PDF".to_owned(),
            icon: "📄".to_owned(),
            category: CreationCategory::Source,
            kind: CreationKind::Import {
                dialog_title: "Elegí un PDF".to_owned(),
                extensions: vec!["pdf".to_owned()],
                mime_types: vec!["application/pdf".to_owned()],
            },
        },
        CreationEntry {
            id: NEW_FOLDER.to_owned(),
            label: "Nueva carpeta".to_owned(),
            icon: "🗂".to_owned(),
            category: CreationCategory::Other,
            kind: CreationKind::Blank { prompt_name: true },
        },
    ]
}

/// Look up a shell builtin by id. `None` if the id is not one of the
/// shell's own.
#[must_use]
pub fn find(entry_id: &str) -> Option<CreationEntry> {
    entries().into_iter().find(|e| e.id == entry_id)
}

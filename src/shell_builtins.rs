//! Shell-owned creation entries — the four flows the home has shipped
//! since v1 (new canvas / incognito / quick sketch / new folder /
//! import PDF).
//!
//! These behave *like* plugin-provided [`CreationEntry`] values —
//! same shape, same ids, same `(plugin_id, entry_id)` referencing — so
//! the FAB `+` menu, the shortcut picker, and the settings surface can
//! treat them uniformly with real plugin entries. Phase 2.6 / 2.7
//! promote the canvas and PDF ones into real plugins; the entries move
//! verbatim with their ids intact, so persisted shortcut references
//! survive the refactor.
//!
//! Dispatch (what each entry actually *does*) lives in `home.rs`
//! because it needs the live `LibHandle` + `RouterContext` — the shell
//! only models the entries here.

use notein_plugin::{CreationCategory, CreationEntry, CreationKind};

/// Pseudo-plugin id every shell-owned entry carries. Lets persisted
/// shortcut references distinguish them from real plugins without a
/// separate "source" tag.
pub const BUILTIN_PLUGIN_ID: &str = "io.notein.builtin";

/// Static shell builtins. Entry ids are stable across releases so
/// persisted shortcut picks keep resolving.
pub const NEW_CANVAS: &str = "new-canvas";
pub const NEW_EPHEMERAL: &str = "new-ephemeral";
pub const QUICK_SKETCH: &str = "quick-sketch";
pub const NEW_FOLDER: &str = "new-folder";
pub const IMPORT_PDF: &str = "import-pdf";

/// Every shell-owned entry. Ordered by the natural menu grouping
/// (canvases first, then sources, then other) so a caller that drops
/// the `category` grouping still sees a sensible order.
#[must_use]
pub fn entries() -> Vec<CreationEntry> {
    vec![
        CreationEntry {
            id: NEW_CANVAS.to_owned(),
            label: "Lienzo infinito".to_owned(),
            icon: "▢".to_owned(),
            category: CreationCategory::Canvas,
            kind: CreationKind::Blank { prompt_name: true },
        },
        CreationEntry {
            id: NEW_EPHEMERAL.to_owned(),
            label: "Lienzo incógnito".to_owned(),
            icon: "🕶".to_owned(),
            category: CreationCategory::Canvas,
            kind: CreationKind::Blank { prompt_name: false },
        },
        CreationEntry {
            id: QUICK_SKETCH.to_owned(),
            label: "Borrador rápido".to_owned(),
            icon: "✎".to_owned(),
            category: CreationCategory::Canvas,
            kind: CreationKind::Blank { prompt_name: false },
        },
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

//! The plugin entry point and what it registers.

use std::sync::Arc;

use crate::config::{ConfigError, ConfigSchema, ConfigValue};
use crate::erased::{ErasedKind, KindAdapter};
use crate::face::{ErasedFace, Face, FaceAdapter};
use crate::info::PluginInfo;
use crate::kind::DocumentKind;
use crate::services::Host;

/// Settings a plugin exposes; the shell draws the form from the
/// schema (one fixed control per field kind).
pub trait PluginSettings: Send + Sync {
    fn schema(&self) -> ConfigSchema;
    fn get(&self, key: &str) -> Option<ConfigValue>;

    /// # Errors
    /// [`ConfigError`] for an unknown key or a value of the wrong type
    /// or out of range.
    fn set(&self, key: &str, value: ConfigValue) -> Result<(), ConfigError>;
}

/// A plugin.
///
/// ```rust,ignore
/// impl Plugin for Music {
///     fn info(&self) -> PluginInfo {
///         PluginInfo::new("dev.music", "Partituras").description("Escribí partituras a mano")
///     }
///
///     fn register(&self, r: &mut Registrar) {
///         let kind = r.kind(ScoreKind::default());
///         r.face(ScoreFace, kind);
///     }
/// }
/// ```
pub trait Plugin: Send + Sync + 'static {
    fn info(&self) -> PluginInfo;

    /// Declare kinds, faces and settings. Called once at load.
    fn register(&self, registrar: &mut Registrar);

    /// Receive the host handle. Called once, after `register`.
    fn install(&self, host: &Host) {
        let _ = host;
    }
}

/// Collects what a plugin contributes.
#[derive(Default)]
pub struct Registrar {
    pub(crate) kinds: Vec<Arc<dyn ErasedKind>>,
    pub(crate) faces: Vec<Arc<dyn ErasedFace>>,
    pub(crate) settings: Option<Arc<dyn PluginSettings>>,
}

impl std::fmt::Debug for Registrar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registrar")
            .field("kinds", &self.kinds.len())
            .field("faces", &self.faces.len())
            .finish_non_exhaustive()
    }
}

impl Registrar {
    /// Register a document kind; returns the shared handle to pass to
    /// its face.
    pub fn kind<K: DocumentKind>(&mut self, kind: impl Into<Arc<K>>) -> Arc<K> {
        let kind = kind.into();
        self.kinds.push(Arc::new(KindAdapter(Arc::clone(&kind))));
        kind
    }

    /// Register the face that renders documents of `kind`.
    pub fn face<F: Face>(&mut self, face: F, kind: Arc<F::Kind>) {
        self.faces.push(Arc::new(FaceAdapter { face, kind }));
    }

    pub fn settings(&mut self, settings: impl PluginSettings + 'static) {
        self.settings = Some(Arc::new(settings));
    }
}

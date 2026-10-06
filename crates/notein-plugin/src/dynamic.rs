//! Dynamic plugin loading (dlopen + Rust-ABI entry fn).
//!
//! A dynamic plugin ships three artefacts:
//!
//! 1. A `plugin.ntp.kdl` manifest describing the plugin ([`crate::manifest`]).
//! 2. A shared library (`.so` / `.dylib` / `.dll`) built with the
//!    **same rustc toolchain and `notein-plugin` version** as the host.
//! 3. An exported entry function `notein_plugin_entry` whose signature
//!    matches [`PluginEntryFn`]. The function returns a boxed
//!    [`PluginBundle`] producing the trait objects the host registers.
//!
//! Trust model: v1 is unsandboxed. The user installed the `.so`, so we
//! load it. Compatibility is enforced by two cheap guards:
//!
//! - [`SDK_ABI_VERSION`]: monotonic integer the host and plugins agree
//!   on. A mismatch (plugin built against a different plugin-trait
//!   shape) is rejected before any trait objects are touched.
//! - Entry symbol lookup errors are surfaced to the loader instead of
//!   crashing — a `.so` that forgot `notein_plugin_entry` is
//!   diagnosable from logs.
//!
//! The entry function uses the **Rust ABI** (not `extern "C"`): the
//! plugin trait objects involve `Vec`, `Arc<dyn Trait>` and other
//! Rust-only shapes. Cross-rustc compatibility is out of scope for v1
//! (plan §10 risks — "trust-only, user instala manual"); a future
//! stabby wrap replaces this with an ABI-stable surface.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::Library;

use crate::canvas::CanvasKind;
use crate::manifest::{ManifestError, PluginManifest};
use crate::source::Source;

/// ABI version shared by host and plugins. **Bump when the
/// `Source` / `CanvasKind` / `Registrations` / `PluginBundle` shapes
/// change in a non-additive way.** Plugins return this via
/// [`PluginBundle::sdk_abi_version`] so the loader can reject
/// mismatched builds before touching any trait objects.
pub const SDK_ABI_VERSION: u32 = 1;

/// Name of the exported entry symbol every dynamic plugin must
/// provide.
pub const ENTRY_SYMBOL: &[u8] = b"notein_plugin_entry\0";

/// The trait objects a plugin wants registered.
///
/// Returned by [`PluginBundle::take_registrations`] and consumed by
/// the host registry.
#[derive(Default)]
pub struct Registrations {
    pub canvases: Vec<Arc<dyn CanvasKind>>,
    pub sources: Vec<Arc<dyn Source>>,
}

impl std::fmt::Debug for Registrations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registrations")
            .field(
                "canvases",
                &self.canvases.iter().map(|c| c.kind_id()).collect::<Vec<_>>(),
            )
            .field(
                "sources",
                &self.sources.iter().map(|s| s.kind_id()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Trait a plugin's root object implements. Produced by the plugin's
/// entry function; drained once by the host.
pub trait PluginBundle: Send + Sync {
    /// Must return [`SDK_ABI_VERSION`] of the SDK version the plugin
    /// was built against. Any other value aborts the load.
    fn sdk_abi_version(&self) -> u32;
    /// Consume the bundle and hand its trait objects to the host.
    fn take_registrations(self: Box<Self>) -> Registrations;
}

/// Signature every dynamic plugin exports as `notein_plugin_entry`.
///
/// The plugin crate declares this with:
///
/// ```ignore
/// #[unsafe(no_mangle)]
/// pub fn notein_plugin_entry() -> Box<dyn notein_plugin::PluginBundle> {
///     Box::new(MyBundle::default())
/// }
/// ```
pub type PluginEntryFn = fn() -> Box<dyn PluginBundle>;

#[derive(Debug)]
pub enum LoaderError {
    Manifest(ManifestError),
    MissingLibrary { manifest_path: PathBuf },
    Dlopen { library_path: PathBuf, error: libloading::Error },
    MissingSymbol { library_path: PathBuf, symbol: &'static str, error: libloading::Error },
    AbiMismatch { plugin_id: String, expected: u32, found: u32 },
}

impl std::fmt::Display for LoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manifest(e) => write!(f, "manifest: {e}"),
            Self::MissingLibrary { manifest_path } => write!(
                f,
                "manifest at {} has no `library` entry and no default .so/.dylib/.dll alongside it",
                manifest_path.display()
            ),
            Self::Dlopen { library_path, error } => {
                write!(f, "dlopen {}: {error}", library_path.display())
            }
            Self::MissingSymbol { library_path, symbol, error } => write!(
                f,
                "symbol `{symbol}` in {}: {error}",
                library_path.display()
            ),
            Self::AbiMismatch { plugin_id, expected, found } => write!(
                f,
                "plugin `{plugin_id}` built against SDK ABI v{found}, host expects v{expected}"
            ),
        }
    }
}

impl std::error::Error for LoaderError {}

impl From<ManifestError> for LoaderError {
    fn from(err: ManifestError) -> Self {
        Self::Manifest(err)
    }
}

/// A loaded dynamic plugin: parsed manifest, open shared library,
/// and the trait objects extracted from the plugin's entry function.
///
/// Drop order is deliberate: the `Library` is the **last** field so
/// Rust drops it after every `Arc<dyn Source>` / `Arc<dyn CanvasKind>`
/// has gone away. Dropping the library while a trait vtable is still
/// resident unloads the plugin's code segment and segfaults the next
/// virtual call.
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub manifest_path: PathBuf,
    pub library_path: PathBuf,
    pub registrations: Registrations,
    _library: Library,
}

impl std::fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedPlugin")
            .field("manifest_id", &self.manifest.id)
            .field("library_path", &self.library_path)
            .field("registrations", &self.registrations)
            .finish_non_exhaustive()
    }
}

/// Load a dynamic plugin from its `plugin.ntp.kdl` path.
///
/// Steps: parse manifest → resolve library path (manifest `library`
/// field; else platform-default alongside the manifest) → `dlopen` →
/// resolve the `notein_plugin_entry` symbol → call it → check the
/// bundle's SDK ABI version → drain its registrations.
pub fn load_from_manifest(manifest_path: &Path) -> Result<LoadedPlugin, LoaderError> {
    let manifest = PluginManifest::from_path(manifest_path)?;

    let library_path = match manifest.library_path(manifest_path) {
        Some(p) => p,
        None => default_library_path(manifest_path, &manifest.id).ok_or_else(|| {
            LoaderError::MissingLibrary {
                manifest_path: manifest_path.to_path_buf(),
            }
        })?,
    };

    // SAFETY: the host assumes a trusted plugin. The library runs
    // with the host's full privilege; a malicious or buggy plugin
    // can crash or corrupt the process. v1 trust model.
    let library = unsafe { Library::new(&library_path) }.map_err(|error| LoaderError::Dlopen {
        library_path: library_path.clone(),
        error,
    })?;

    let registrations = {
        // SAFETY: entry symbol must match [`PluginEntryFn`]. Mismatch
        // = UB when the fn pointer is called below. Plugin authors
        // follow the documented signature via `notein-plugin`'s
        // re-exports; a `#[unsafe(no_mangle)]` typo surfaces here as
        // a signature mismatch at best, a crash at worst.
        let entry: libloading::Symbol<PluginEntryFn> =
            unsafe { library.get(ENTRY_SYMBOL) }.map_err(|error| LoaderError::MissingSymbol {
                library_path: library_path.clone(),
                symbol: "notein_plugin_entry",
                error,
            })?;
        let bundle = entry();
        if bundle.sdk_abi_version() != SDK_ABI_VERSION {
            return Err(LoaderError::AbiMismatch {
                plugin_id: manifest.id.clone(),
                expected: SDK_ABI_VERSION,
                found: bundle.sdk_abi_version(),
            });
        }
        bundle.take_registrations()
    };

    Ok(LoadedPlugin {
        manifest,
        manifest_path: manifest_path.to_path_buf(),
        library_path,
        registrations,
        _library: library,
    })
}

fn default_library_path(manifest_path: &Path, id: &str) -> Option<PathBuf> {
    let parent = manifest_path.parent()?;
    let base = sanitize_id_for_filename(id);
    #[cfg(target_os = "windows")]
    let file = format!("{base}.dll");
    #[cfg(target_os = "macos")]
    let file = format!("lib{base}.dylib");
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let file = format!("lib{base}.so");
    Some(parent.join(file))
}

fn sanitize_id_for_filename(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_dots_and_dashes() {
        assert_eq!(sanitize_id_for_filename("org.user.quran"), "org_user_quran");
        assert_eq!(sanitize_id_for_filename("io.notein-pdf"), "io_notein_pdf");
    }
}

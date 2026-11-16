//! The app's folders, from istmo's platform paths.
//!
//! Every folder goes through here instead of `istmo::path` directly, so
//! tests resolve them into a scratch directory of their own: istmo keeps
//! the first paths it hands out, and whichever test asked first used to
//! decide whether the rest wrote into the person's real library.

use std::path::Path;

/// Persistent, roaming application data.
#[must_use]
pub fn data_dir() -> &'static Path {
    sandbox();
    #[expect(clippy::disallowed_methods, reason = "the one place that reads it")]
    istmo::path::data_dir()
}

/// Disposable cache.
#[must_use]
pub fn cache_dir() -> &'static Path {
    sandbox();
    #[expect(clippy::disallowed_methods, reason = "the one place that reads it")]
    istmo::path::cache_dir()
}

/// Point every folder at a scratch directory for this test process.
///
/// # Panics
/// When istmo handed out its paths before this ran: a test would be
/// about to write into the real data.
#[cfg(test)]
pub(crate) fn sandbox() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let root = std::env::temp_dir().join(format!("notinplus-test-{}", std::process::id()));
        let dir = |name: &str| root.join(name);
        istmo::path::install(istmo::path::PlatformPaths {
            data_dir: dir("data"),
            data_local_dir: dir("data-local"),
            config_dir: dir("config"),
            config_local_dir: dir("config-local"),
            cache_dir: dir("cache"),
            state_dir: dir("state"),
            preference_dir: dir("preferences"),
            documents_dir: dir("documents"),
            runtime_dir: dir("runtime"),
            temp_dir: dir("tmp"),
        });
        #[expect(clippy::disallowed_methods, reason = "checks what istmo kept")]
        let data = istmo::path::data_dir();
        assert!(
            data.starts_with(&root),
            "istmo resolved its paths before the test sandbox ({}); a test would write into real data",
            data.display()
        );
    });
}

#[cfg(not(test))]
const fn sandbox() {}

//! The platforms a plugin library is built for.
//!
//! A plugin may ship one shared library per platform. Each one names
//! its target as `<os>-<arch>` (`linux-x86_64`, `android-aarch64`) or
//! as an operating system alone (`macos`, for a universal binary). The
//! names are Rust's own, [`std::env::consts::OS`] and
//! [`std::env::consts::ARCH`]; the usual aliases (`arm64`, `amd64`,
//! `x64`, `darwin`…) are accepted and folded into them.

use std::fmt;

/// An operating system and, optionally, a CPU architecture.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub os: String,
    /// `None` fits every architecture of `os`.
    pub arch: Option<String>,
}

/// How well a library fits the device, best last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fit {
    /// The library names no target: it is meant for any device.
    Any,
    /// Same operating system, any architecture.
    System,
    /// Same operating system and architecture.
    Exact,
}

impl Target {
    /// The device the app runs on.
    #[must_use]
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS.to_owned(),
            arch: Some(std::env::consts::ARCH.to_owned()),
        }
    }

    /// Read `linux-x86_64`, `macos` or an alias of either; `None` when
    /// the text names no known system.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        let (os, arch) = text
            .split_once('-')
            .map_or((text.as_str(), None), |(os, arch)| (os, Some(arch)));
        let os = canonical_os(os)?;
        let arch = match arch {
            Some(arch) => Some(canonical_arch(arch)?),
            None => None,
        };
        Some(Self {
            os: os.to_owned(),
            arch: arch.map(str::to_owned),
        })
    }

    /// How a library built for `self` fits `device`; `None` when it
    /// does not run there.
    #[must_use]
    pub fn fit(&self, device: &Self) -> Option<Fit> {
        if self.os != device.os {
            return None;
        }
        match (&self.arch, &device.arch) {
            (None, _) => Some(Fit::System),
            (Some(arch), Some(device)) if arch == device => Some(Fit::Exact),
            (Some(_), _) => None,
        }
    }

    /// File name a library of plugin `id` has on this target's system
    /// when the manifest names none (`liborg_user_quran.so`).
    #[must_use]
    pub fn default_library(&self, id: &str) -> String {
        let base: String = id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        match self.os.as_str() {
            "windows" => format!("{base}.dll"),
            "macos" | "ios" => format!("lib{base}.dylib"),
            _ => format!("lib{base}.so"),
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.arch {
            Some(arch) => write!(f, "{}-{arch}", self.os),
            None => f.write_str(&self.os),
        }
    }
}

fn canonical_os(os: &str) -> Option<&'static str> {
    Some(match os {
        "linux" => "linux",
        "android" => "android",
        "macos" | "darwin" | "osx" | "mac" => "macos",
        "ios" => "ios",
        "windows" | "win" | "win32" | "win64" => "windows",
        "freebsd" => "freebsd",
        _ => return None,
    })
}

fn canonical_arch(arch: &str) -> Option<&'static str> {
    Some(match arch {
        "x86_64" | "x64" | "amd64" => "x86_64",
        "aarch64" | "arm64" | "arm64-v8a" => "aarch64",
        "arm" | "armv7" | "armeabi-v7a" => "arm",
        "x86" | "i386" | "i686" => "x86",
        "riscv64" => "riscv64",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(text: &str) -> Target {
        Target::parse(text).expect("known target")
    }

    #[test]
    fn aliases_fold_into_rust_names() {
        assert_eq!(target("Darwin-ARM64"), target("macos-aarch64"));
        assert_eq!(target("windows-amd64").to_string(), "windows-x86_64");
        assert_eq!(target("android-arm64-v8a").arch.as_deref(), Some("aarch64"));
        assert_eq!(Target::parse("plan9-x86_64"), None);
        assert_eq!(Target::parse("linux-sparc"), None);
    }

    #[test]
    fn an_exact_target_fits_better_than_its_system() {
        let device = target("linux-x86_64");
        assert_eq!(target("linux-x86_64").fit(&device), Some(Fit::Exact));
        assert_eq!(target("linux").fit(&device), Some(Fit::System));
        assert_eq!(target("linux-aarch64").fit(&device), None);
        assert_eq!(target("windows-x86_64").fit(&device), None);
        assert!(Fit::Exact > Fit::System && Fit::System > Fit::Any);
    }

    #[test]
    fn default_library_names_follow_the_system() {
        assert_eq!(
            target("linux-x86_64").default_library("org.user.quran"),
            "liborg_user_quran.so"
        );
        assert_eq!(target("windows-x86_64").default_library("a.b"), "a_b.dll");
        assert_eq!(target("macos").default_library("a.b"), "liba_b.dylib");
    }
}

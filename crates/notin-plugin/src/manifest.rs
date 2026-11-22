//! `plugin.ntp.kdl` manifest format + parser.
//!
//! The manifest sits next to the plugin's `.so` / `.dll` / `.dylib`
//! and tells the host:
//!
//! - Plugin identity (`id`, `name`, `version`, `author`, optional
//!   `description`), shown before anything is loaded.
//! - What traits it provides (`provides` lines — informational).
//! - Which `notin-sdk` ABI it was built against (`requires` lines).
//! - Which library file to load on each platform (`library` lines,
//!   relative to the manifest, see [`PluginManifest::library_for`]).
//!
//! The format is a KDL subset — single-value entries only, no children
//! on the top-level nodes we care about. A hand-rolled line parser
//! keeps the plugin crate dep-light; swapping to a full KDL crate is
//! a drop-in later if needed.
//!
//! ```kdl
//! id      "org.user.quran"
//! name    "Quran Source"
//! version "1.0.0"
//! author  "Alice"
//! description "Read the Quran next to your notes."
//! library "linux/x86_64/libquran.so"   target="linux-x86_64"
//! library "linux/aarch64/libquran.so"  target="linux-aarch64"
//! library "android/arm64/libquran.so"  target="android-aarch64"
//! library "macos/libquran.dylib"       target="macos"
//! library "windows/x86_64/quran.dll"   target="windows-x86_64"
//!
//! provides "io.notein.source"
//! requires "io.notein.sdk" min-version="1.0"
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use notin_i18n::tr;

use crate::failure::UserMessage;
use crate::target::{Fit, Target};

mod declared;

pub use declared::{Checksum, Declared, DeclaredEntry, DeclaredKind, Mismatch, quote};

/// Parsed `plugin.ntp.kdl` manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    /// One sentence for the install review.
    pub description: Option<String>,
    /// The shared libraries, relative to the manifest, each for the
    /// platform its `target` names. Empty: the loader looks for the
    /// platform's default file name, see [`PluginManifest::library_for`].
    pub libraries: Vec<ManifestLibrary>,
    pub provides: Vec<String>,
    pub requires: Vec<ManifestRequirement>,
    /// What the plugin says it adds; `None` when the manifest says
    /// nothing (written by hand, or by an older `notin-pack`).
    pub declared: Option<Declared>,
    /// SHA-256 of its library files, checked before one is loaded.
    pub checksums: Vec<Checksum>,
}

/// One `library` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestLibrary {
    /// Path from the manifest's folder.
    pub path: String,
    /// The platform it is built for; `None` runs anywhere.
    pub target: Option<Target>,
}

impl ManifestLibrary {
    fn fit(&self, device: &Target) -> Option<Fit> {
        self.target
            .as_ref()
            .map_or(Some(Fit::Any), |target| target.fit(device))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestRequirement {
    pub id: String,
    pub min_version: Option<String>,
}

#[derive(Debug)]
pub enum ManifestError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Parse {
        path: PathBuf,
        line: usize,
        message: String,
    },
    MissingField {
        path: PathBuf,
        field: &'static str,
    },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "read {}: {}", path.display(), error),
            Self::Parse {
                path,
                line,
                message,
            } => {
                write!(f, "{}:{}: {}", path.display(), line, message)
            }
            Self::MissingField { path, field } => {
                write!(f, "{}: missing required field `{}`", path.display(), field)
            }
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { error, .. } => Some(error),
            Self::Parse { .. } | Self::MissingField { .. } => None,
        }
    }
}

impl UserMessage for ManifestError {
    fn user_message(&self) -> String {
        match self {
            Self::Io { error, .. } => error.user_message(),
            Self::Parse { .. } | Self::MissingField { .. } => tr!("plugin-load-damaged"),
        }
    }
}

impl PluginManifest {
    #[cfg_attr(feature = "sismo", sismo::function("plugin::manifest_from_path"))]
    pub fn from_path(path: &Path) -> Result<Self, ManifestError> {
        let text = fs::read_to_string(path).map_err(|error| ManifestError::Io {
            path: path.to_path_buf(),
            error,
        })?;
        Self::parse(path, &text)
    }

    #[cfg_attr(feature = "sismo", sismo::function("plugin::manifest_parse"))]
    pub fn parse(path: &Path, text: &str) -> Result<Self, ManifestError> {
        let mut id = None;
        let mut name = None;
        let mut version = None;
        let mut author = None;
        let mut description = None;
        let mut libraries = Vec::new();
        let mut provides = Vec::new();
        let mut requires = Vec::new();
        let mut declared = Declared::default();
        let mut any_declared = false;
        let mut checksums = Vec::new();

        for (idx, raw_line) in text.lines().enumerate() {
            let line_no = idx + 1;
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            let (node, rest) = split_node(line);
            match node {
                "id" => id = Some(expect_string(path, line_no, rest)?),
                "name" => name = Some(expect_string(path, line_no, rest)?),
                "version" => version = Some(expect_string(path, line_no, rest)?),
                "author" => author = Some(expect_string(path, line_no, rest)?),
                "description" => description = Some(expect_string(path, line_no, rest)?),
                "library" => libraries.push(parse_library(path, line_no, rest)?),
                "provides" => provides.push(expect_string(path, line_no, rest)?),
                "requires" => requires.push(parse_requires(path, line_no, rest)?),
                "kind" => {
                    let (id, attrs) = parse_attrs(path, line_no, rest)?;
                    declared.kinds.push(DeclaredKind {
                        label: attr(&attrs, "label").unwrap_or_else(|| id.clone()),
                        id,
                    });
                    any_declared = true;
                }
                "create" => {
                    let (id, attrs) = parse_attrs(path, line_no, rest)?;
                    let kind = attr(&attrs, "kind").ok_or_else(|| ManifestError::Parse {
                        path: path.to_path_buf(),
                        line: line_no,
                        message: "`create` needs `kind=`".into(),
                    })?;
                    declared.entries.push(DeclaredEntry {
                        label: attr(&attrs, "label").unwrap_or_else(|| id.clone()),
                        id,
                        kind,
                    });
                    any_declared = true;
                }
                "settings" => {
                    declared.settings = rest.trim().parse().map_err(|_| ManifestError::Parse {
                        path: path.to_path_buf(),
                        line: line_no,
                        message: format!("`settings` takes a number, got `{rest}`"),
                    })?;
                    any_declared = true;
                }
                "checksum" => {
                    let (file, attrs) = parse_attrs(path, line_no, rest)?;
                    let sha256 = attr(&attrs, "sha256")
                        .map(|hex| hex.to_ascii_lowercase())
                        .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
                        .ok_or_else(|| ManifestError::Parse {
                            path: path.to_path_buf(),
                            line: line_no,
                            message: "`checksum` needs `sha256=` with 64 hex digits".into(),
                        })?;
                    checksums.push(Checksum { path: file, sha256 });
                }
                // Unknown nodes are tolerated — forward-compat with
                // richer manifests. Logged by the loader if desired.
                _ => {}
            }
        }

        Ok(Self {
            id: id.ok_or_else(|| ManifestError::MissingField {
                path: path.to_path_buf(),
                field: "id",
            })?,
            name: name.ok_or_else(|| ManifestError::MissingField {
                path: path.to_path_buf(),
                field: "name",
            })?,
            version: version.ok_or_else(|| ManifestError::MissingField {
                path: path.to_path_buf(),
                field: "version",
            })?,
            author,
            description,
            libraries,
            provides,
            requires,
            declared: any_declared.then_some(declared),
            checksums,
        })
    }

    /// The library this device loads.
    ///
    /// With `library` lines, the one whose target fits the device best:
    /// its system and architecture, then its system alone, then one
    /// naming no target. Without any, the first that exists of
    /// `<os>-<arch>/<file>`, `<os>/<file>` and `<file>` next to the
    /// manifest, where `<file>` is the system's default name for the
    /// plugin ([`Target::default_library`]).
    #[must_use]
    pub fn library_for(&self, manifest_path: &Path, device: &Target) -> Option<PathBuf> {
        let dir = manifest_path.parent()?;
        if !self.libraries.is_empty() {
            return self
                .libraries
                .iter()
                .filter_map(|library| library.fit(device).map(|fit| (fit, library)))
                .max_by_key(|(fit, _)| *fit)
                .map(|(_, library)| dir.join(&library.path));
        }
        let file = device.default_library(&self.id);
        let mut candidates = Vec::with_capacity(3);
        if let Some(arch) = &device.arch {
            candidates.push(dir.join(format!("{}-{arch}", device.os)).join(&file));
        }
        candidates.push(dir.join(&device.os).join(&file));
        candidates.push(dir.join(&file));
        candidates.into_iter().find(|path| path.exists())
    }

    /// The platforms the `library` lines are built for.
    #[must_use]
    pub fn targets(&self) -> Vec<String> {
        self.libraries
            .iter()
            .map(|library| {
                library
                    .target
                    .as_ref()
                    .map_or_else(|| tr!("plugin-manifest-any-target"), ToString::to_string)
            })
            .collect()
    }
}

fn strip_comment(line: &str) -> &str {
    line.find("//").map_or(line, |idx| &line[..idx])
}

fn split_node(line: &str) -> (&str, &str) {
    let bytes = line.as_bytes();
    let split = bytes
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(bytes.len());
    let (node, rest) = line.split_at(split);
    (node, rest.trim_start())
}

fn expect_string(path: &Path, line: usize, rest: &str) -> Result<String, ManifestError> {
    let (value, _) = parse_quoted(path, line, rest)?;
    Ok(value)
}

fn parse_library(path: &Path, line: usize, rest: &str) -> Result<ManifestLibrary, ManifestError> {
    let (library, after) = parse_quoted(path, line, rest)?;
    let after = after.trim_start();
    let target = if let Some(remainder) = after.strip_prefix("target=") {
        let (text, _) = parse_quoted(path, line, remainder)?;
        Some(Target::parse(&text).ok_or_else(|| ManifestError::Parse {
            path: path.to_path_buf(),
            line,
            message: format!("unknown target `{text}`; use `<os>-<arch>` or `<os>`"),
        })?)
    } else if after.is_empty() {
        None
    } else {
        return Err(ManifestError::Parse {
            path: path.to_path_buf(),
            line,
            message: format!("unexpected trailing tokens: `{after}`"),
        });
    };
    Ok(ManifestLibrary {
        path: library,
        target,
    })
}

/// `"value" key="v" …`: the value and every attribute, unknown ones
/// included (newer `notin-pack` versions may add some).
fn parse_attrs(
    path: &Path,
    line: usize,
    rest: &str,
) -> Result<(String, Vec<(String, String)>), ManifestError> {
    let (value, mut after) = parse_quoted(path, line, rest)?;
    let mut attrs = Vec::new();
    loop {
        after = after.trim_start();
        if after.is_empty() {
            return Ok((value, attrs));
        }
        let Some((key, remainder)) = after.split_once('=') else {
            return Err(ManifestError::Parse {
                path: path.to_path_buf(),
                line,
                message: format!("unexpected trailing tokens: `{after}`"),
            });
        };
        let (text, next) = parse_quoted(path, line, remainder)?;
        attrs.push((key.trim().to_owned(), text));
        after = next;
    }
}

fn attr(attrs: &[(String, String)], key: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
}

fn parse_requires(
    path: &Path,
    line: usize,
    rest: &str,
) -> Result<ManifestRequirement, ManifestError> {
    let (id, after) = parse_quoted(path, line, rest)?;
    let after = after.trim_start();
    let min_version = if let Some(remainder) = after.strip_prefix("min-version=") {
        let (ver, _) = parse_quoted(path, line, remainder)?;
        Some(ver)
    } else if after.is_empty() {
        None
    } else {
        return Err(ManifestError::Parse {
            path: path.to_path_buf(),
            line,
            message: format!("unexpected trailing tokens: `{after}`"),
        });
    };
    Ok(ManifestRequirement { id, min_version })
}

/// Parse a `"..."` string literal. Returns the unescaped content and
/// the slice of `input` after the closing quote.
fn parse_quoted<'a>(
    path: &Path,
    line: usize,
    input: &'a str,
) -> Result<(String, &'a str), ManifestError> {
    let trimmed = input.trim_start();
    let mut chars = trimmed.char_indices();
    let Some((_, '"')) = chars.next() else {
        return Err(ManifestError::Parse {
            path: path.to_path_buf(),
            line,
            message: format!("expected string, got `{trimmed}`"),
        });
    };
    let mut out = String::new();
    while let Some((idx, c)) = chars.next() {
        match c {
            '"' => {
                let rest = &trimmed[idx + c.len_utf8()..];
                return Ok((out, rest));
            }
            '\\' => match chars.next() {
                Some((_, '"')) => out.push('"'),
                Some((_, '\\')) => out.push('\\'),
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, other)) => out.push(other),
                None => {
                    return Err(ManifestError::Parse {
                        path: path.to_path_buf(),
                        line,
                        message: "trailing backslash in string".into(),
                    });
                }
            },
            _ => out.push(c),
        }
    }
    Err(ManifestError::Parse {
        path: path.to_path_buf(),
        line,
        message: "unterminated string literal".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(text: &str) -> Result<PluginManifest, ManifestError> {
        PluginManifest::parse(Path::new("plugin.ntp.kdl"), text)
    }

    #[test]
    fn parses_minimal_manifest() {
        let m = mk(r#"
id      "org.user.quran"
name    "Quran Source"
version "1.0.0"
"#)
        .unwrap();
        assert_eq!(m.id, "org.user.quran");
        assert_eq!(m.name, "Quran Source");
        assert_eq!(m.version, "1.0.0");
        assert!(m.author.is_none());
        assert!(m.libraries.is_empty());
        assert!(m.provides.is_empty());
        assert!(m.requires.is_empty());
    }

    #[test]
    fn declarations_round_trip() {
        let declared = Declared {
            kinds: vec![DeclaredKind {
                id: "q.reading".into(),
                label: "Lectura \"diaria\"".into(),
            }],
            entries: vec![DeclaredEntry {
                id: "new".into(),
                kind: "q.reading".into(),
                label: "Nueva".into(),
            }],
            settings: 3,
        };
        let checksum = Checksum {
            path: "linux/libq.so".into(),
            sha256: "AB".repeat(32),
        };
        let text = format!(
            "id \"q\"\nname \"Q\"\nversion \"1\"\n{}{}future \"x\" key=\"v\"\n",
            declared.to_kdl(),
            checksum.to_kdl()
        );
        let m = mk(&text).unwrap();
        assert_eq!(m.declared, Some(declared));
        assert_eq!(m.checksums.len(), 1);
        assert_eq!(m.checksums[0].sha256, "ab".repeat(32), "stored lowercase");
        assert_eq!(
            mk("id \"q\"\nname \"Q\"\nversion \"1\"\n")
                .unwrap()
                .declared,
            None
        );
        assert!(mk("id \"q\"\nname \"Q\"\nversion \"1\"\nchecksum \"a\" sha256=\"zz\"\n").is_err());
    }

    #[test]
    fn parses_full_manifest() {
        let m = mk(r#"
// top-level comment
id       "org.user.quran"
name     "Quran Source"
version  "1.0.0"
author   "Alice"
description "Read the \"Quran\"."
library  "libquran.so"

provides "io.notein.source"
requires "io.notein.sdk" min-version="1.0"
requires "io.notein.pdf"
"#)
        .unwrap();
        assert_eq!(m.author.as_deref(), Some("Alice"));
        assert_eq!(m.description.as_deref(), Some("Read the \"Quran\"."));
        assert_eq!(
            m.libraries,
            vec![ManifestLibrary {
                path: "libquran.so".into(),
                target: None,
            }]
        );
        assert_eq!(m.provides, vec!["io.notein.source".to_owned()]);
        assert_eq!(m.requires.len(), 2);
        assert_eq!(m.requires[0].id, "io.notein.sdk");
        assert_eq!(m.requires[0].min_version.as_deref(), Some("1.0"));
        assert_eq!(m.requires[1].min_version, None);
    }

    #[test]
    fn rejects_missing_id() {
        let err = mk(r#"name "x"\nversion "1""#).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::MissingField { field: "id", .. }
        ));
    }

    #[test]
    fn rejects_unterminated_string() {
        let err = mk(r#"id "org.user"#).unwrap_err();
        assert!(matches!(err, ManifestError::Parse { .. }));
    }

    fn quran(libraries: &[(&str, Option<&str>)]) -> PluginManifest {
        PluginManifest {
            id: "org.user.quran".into(),
            name: "Corán".into(),
            version: "1".into(),
            author: None,
            description: None,
            libraries: libraries
                .iter()
                .map(|(path, target)| ManifestLibrary {
                    path: (*path).to_owned(),
                    target: target.map(|t| Target::parse(t).expect("target")),
                })
                .collect(),
            provides: Vec::new(),
            requires: Vec::new(),
            declared: None,
            checksums: Vec::new(),
        }
    }

    #[test]
    fn parses_targeted_libraries() {
        let m = mk(r#"
id      "org.user.quran"
name    "Quran Source"
version "1.0.0"
library "linux/x86_64/libquran.so" target="linux-x86_64"
library "macos/libquran.dylib" target="darwin"
"#)
        .unwrap();
        assert_eq!(m.targets(), ["linux-x86_64", "macos"]);
        let err = mk("id \"x\"\nname \"x\"\nversion \"1\"\nlibrary \"x.so\" target=\"plan9\"")
            .unwrap_err();
        assert!(matches!(err, ManifestError::Parse { line: 4, .. }));
    }

    #[test]
    fn the_device_loads_its_best_fitting_library() {
        let m = quran(&[
            ("any/libquran.so", None),
            ("linux/libquran.so", Some("linux")),
            ("linux/x86_64/libquran.so", Some("linux-x86_64")),
            ("linux/aarch64/libquran.so", Some("linux-aarch64")),
        ]);
        let manifest = Path::new("/opt/plugins/quran/plugin.ntp.kdl");
        let pick = |device: &str| m.library_for(manifest, &Target::parse(device).expect("device"));
        assert_eq!(
            pick("linux-x86_64"),
            Some(PathBuf::from("/opt/plugins/quran/linux/x86_64/libquran.so"))
        );
        assert_eq!(
            pick("linux-riscv64"),
            Some(PathBuf::from("/opt/plugins/quran/linux/libquran.so"))
        );
        assert_eq!(
            pick("windows-x86_64"),
            Some(PathBuf::from("/opt/plugins/quran/any/libquran.so"))
        );
        let only_linux = quran(&[("linux/x86_64/libquran.so", Some("linux-x86_64"))]);
        assert_eq!(
            only_linux.library_for(manifest, &Target::parse("android-aarch64").expect("device")),
            None
        );
    }

    #[test]
    fn without_library_lines_the_device_folder_comes_first() {
        let dir = std::env::temp_dir().join(format!("notein-manifest-{}", std::process::id()));
        let device = Target::parse("linux-aarch64").expect("device");
        let file = device.default_library("org.user.quran");
        std::fs::create_dir_all(dir.join("linux-aarch64")).expect("dir");
        std::fs::write(dir.join(&file), b"").expect("root library");
        let manifest = dir.join("plugin.ntp.kdl");
        let m = quran(&[]);
        assert_eq!(m.library_for(&manifest, &device), Some(dir.join(&file)));
        std::fs::write(dir.join("linux-aarch64").join(&file), b"").expect("device library");
        assert_eq!(
            m.library_for(&manifest, &device),
            Some(dir.join("linux-aarch64").join(&file))
        );
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}

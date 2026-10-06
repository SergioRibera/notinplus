//! `plugin.ntp.kdl` manifest format + parser.
//!
//! The manifest sits next to the plugin's `.so` / `.dll` / `.dylib`
//! and tells the host:
//!
//! - Plugin identity (`id`, `name`, `version`, `author`).
//! - What traits it provides (`provides` lines — informational).
//! - Which `notein-sdk` ABI it was built against (`requires` lines).
//! - Which library file to load (`library` — relative to the manifest).
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
//! library "libquran.so"
//!
//! provides "io.notein.source"
//! requires "io.notein.sdk" min-version="1.0"
//! ```

use std::fs;
use std::path::{Path, PathBuf};

/// Parsed `plugin.ntp.kdl` manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    /// Relative path (from the manifest dir) to the shared library
    /// file. If absent the loader falls back to platform defaults
    /// (`lib<id>.so` on linux, etc.).
    pub library: Option<String>,
    pub provides: Vec<String>,
    pub requires: Vec<ManifestRequirement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestRequirement {
    pub id: String,
    pub min_version: Option<String>,
}

#[derive(Debug)]
pub enum ManifestError {
    Io { path: PathBuf, error: std::io::Error },
    Parse { path: PathBuf, line: usize, message: String },
    MissingField { path: PathBuf, field: &'static str },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "read {}: {}", path.display(), error),
            Self::Parse { path, line, message } => {
                write!(f, "{}:{}: {}", path.display(), line, message)
            }
            Self::MissingField { path, field } => {
                write!(f, "{}: missing required field `{}`", path.display(), field)
            }
        }
    }
}

impl std::error::Error for ManifestError {}

impl PluginManifest {
    pub fn from_path(path: &Path) -> Result<Self, ManifestError> {
        let text = fs::read_to_string(path).map_err(|error| ManifestError::Io {
            path: path.to_path_buf(),
            error,
        })?;
        Self::parse(path, &text)
    }

    pub fn parse(path: &Path, text: &str) -> Result<Self, ManifestError> {
        let mut id = None;
        let mut name = None;
        let mut version = None;
        let mut author = None;
        let mut library = None;
        let mut provides = Vec::new();
        let mut requires = Vec::new();

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
                "library" => library = Some(expect_string(path, line_no, rest)?),
                "provides" => provides.push(expect_string(path, line_no, rest)?),
                "requires" => requires.push(parse_requires(path, line_no, rest)?),
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
            library,
            provides,
            requires,
        })
    }

    /// Absolute path to the plugin's library file, resolved against
    /// the manifest's own directory. Returns `None` when the manifest
    /// has no `library` field and the caller must apply a default.
    #[must_use]
    pub fn library_path(&self, manifest_path: &Path) -> Option<PathBuf> {
        let parent = manifest_path.parent()?;
        self.library.as_ref().map(|lib| parent.join(lib))
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find("//") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn split_node(line: &str) -> (&str, &str) {
    let bytes = line.as_bytes();
    let split = bytes
        .iter()
        .position(|b| b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let (node, rest) = line.split_at(split);
    (node, rest.trim_start())
}

fn expect_string(path: &Path, line: usize, rest: &str) -> Result<String, ManifestError> {
    let (value, _) = parse_quoted(path, line, rest)?;
    Ok(value)
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
"#).unwrap();
        assert_eq!(m.id, "org.user.quran");
        assert_eq!(m.name, "Quran Source");
        assert_eq!(m.version, "1.0.0");
        assert!(m.author.is_none());
        assert!(m.library.is_none());
        assert!(m.provides.is_empty());
        assert!(m.requires.is_empty());
    }

    #[test]
    fn parses_full_manifest() {
        let m = mk(r#"
// top-level comment
id       "org.user.quran"
name     "Quran Source"
version  "1.0.0"
author   "Alice"
library  "libquran.so"

provides "io.notein.source"
requires "io.notein.sdk" min-version="1.0"
requires "io.notein.pdf"
"#).unwrap();
        assert_eq!(m.author.as_deref(), Some("Alice"));
        assert_eq!(m.library.as_deref(), Some("libquran.so"));
        assert_eq!(m.provides, vec!["io.notein.source".to_owned()]);
        assert_eq!(m.requires.len(), 2);
        assert_eq!(m.requires[0].id, "io.notein.sdk");
        assert_eq!(m.requires[0].min_version.as_deref(), Some("1.0"));
        assert_eq!(m.requires[1].min_version, None);
    }

    #[test]
    fn rejects_missing_id() {
        let err = mk(r#"name "x"\nversion "1""#).unwrap_err();
        assert!(matches!(err, ManifestError::MissingField { field: "id", .. }));
    }

    #[test]
    fn rejects_unterminated_string() {
        let err = mk(r#"id "org.user"#).unwrap_err();
        assert!(matches!(err, ManifestError::Parse { .. }));
    }

    #[test]
    fn library_path_resolves_relative_to_manifest() {
        let m = PluginManifest {
            id: "x".into(),
            name: "x".into(),
            version: "1".into(),
            author: None,
            library: Some("libx.so".into()),
            provides: Vec::new(),
            requires: Vec::new(),
        };
        let manifest = Path::new("/opt/plugins/x/plugin.ntp.kdl");
        assert_eq!(
            m.library_path(manifest).unwrap(),
            PathBuf::from("/opt/plugins/x/libx.so")
        );
    }
}

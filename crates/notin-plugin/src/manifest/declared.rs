//! What a plugin says it adds, and the checksums of its libraries.
//!
//! Both are written into `plugin.ntp.kdl` by `notin-pack`, so the app
//! can show them before any of the plugin's code runs:
//!
//! ```kdl
//! kind     "org.user.quran.reading" label="Reading"
//! create   "new" kind="org.user.quran.reading" label="New reading"
//! settings 3
//! checksum "linux/x86_64/libquran.so" sha256="9f86d0…"
//! ```
//!
//! Older apps skip these nodes (unknown nodes are ignored), so a
//! manifest carrying them still installs there.

use std::collections::BTreeSet;
use std::fmt::Write as _;

/// The document types, Create menu entries and settings a plugin adds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declared {
    pub kinds: Vec<DeclaredKind>,
    pub entries: Vec<DeclaredEntry>,
    /// Number of settings fields.
    pub settings: usize,
}

/// A document type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredKind {
    pub id: String,
    /// In the language it was packed in; only shown, never compared.
    pub label: String,
}

/// A Create menu entry of a document type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredEntry {
    pub id: String,
    /// Id of the document type it creates.
    pub kind: String,
    pub label: String,
}

/// The SHA-256 of one library file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checksum {
    /// Path from the manifest's folder, as in its `library` line.
    pub path: String,
    /// Lowercase hex.
    pub sha256: String,
}

/// How what a plugin registered differs from what it declared.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mismatch {
    /// Registered but not declared.
    pub undeclared: Vec<String>,
    /// Declared but not registered.
    pub missing: Vec<String>,
}

impl Declared {
    /// Whether nothing is declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty() && self.entries.is_empty() && self.settings == 0
    }

    /// Compare by ids (labels depend on the language), `None` when the
    /// plugin registered exactly what it declared.
    #[must_use]
    pub fn mismatch(&self, registered: &Self) -> Option<Mismatch> {
        let ids = |declared: &Self| -> BTreeSet<String> {
            declared
                .kinds
                .iter()
                .map(|kind| format!("kind {}", kind.id))
                .chain(
                    declared
                        .entries
                        .iter()
                        .map(|entry| format!("create {}/{}", entry.kind, entry.id)),
                )
                .chain(std::iter::once(format!("settings {}", declared.settings)))
                .collect()
        };
        let (said, did) = (ids(self), ids(registered));
        let mismatch = Mismatch {
            undeclared: did.difference(&said).cloned().collect(),
            missing: said.difference(&did).cloned().collect(),
        };
        (!mismatch.undeclared.is_empty() || !mismatch.missing.is_empty()).then_some(mismatch)
    }

    /// The manifest lines for `self`.
    #[must_use]
    pub fn to_kdl(&self) -> String {
        let mut out = String::new();
        for kind in &self.kinds {
            let _ = writeln!(
                out,
                "kind     {} label={}",
                quote(&kind.id),
                quote(&kind.label)
            );
        }
        for entry in &self.entries {
            let _ = writeln!(
                out,
                "create   {} kind={} label={}",
                quote(&entry.id),
                quote(&entry.kind),
                quote(&entry.label)
            );
        }
        let _ = writeln!(out, "settings {}", self.settings);
        out
    }
}

impl Checksum {
    /// The manifest line for `self`.
    #[must_use]
    pub fn to_kdl(&self) -> String {
        format!(
            "checksum {} sha256={}\n",
            quote(&self.path),
            quote(&self.sha256)
        )
    }
}

/// `text` as a manifest string literal.
#[must_use]
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            // One line per node: the parser reads no escapes for these.
            '\n' | '\r' | '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quran() -> Declared {
        Declared {
            kinds: vec![DeclaredKind {
                id: "q.reading".into(),
                label: "Reading".into(),
            }],
            entries: vec![DeclaredEntry {
                id: "new".into(),
                kind: "q.reading".into(),
                label: "New reading".into(),
            }],
            settings: 2,
        }
    }

    #[test]
    fn labels_do_not_count() {
        let mut spanish = quran();
        spanish.kinds[0].label = "Lectura".into();
        assert_eq!(quran().mismatch(&spanish), None);
    }

    #[test]
    fn undeclared_and_missing_are_named() {
        let mut registered = quran();
        registered.kinds[0].id = "q.other".into();
        registered.settings = 3;
        let mismatch = quran().mismatch(&registered).expect("differs");
        assert_eq!(mismatch.undeclared, ["kind q.other", "settings 3"]);
        assert_eq!(mismatch.missing, ["kind q.reading", "settings 2"]);
    }

    #[test]
    fn quoting_escapes() {
        assert_eq!(quote(r#"a "b" \c"#), r#""a \"b\" \\c""#);
    }
}

//! Build-script half: find, check and list a crate's Fluent files.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use fluent_syntax::ast::Entry;

use crate::Lang;

/// Folder, relative to the crate manifest, holding `<lang>/*.ftl`.
const LOCALES_DIR: &str = "locales";
/// File `catalog!` includes from `OUT_DIR`.
const OUTPUT: &str = "notein_i18n_locales.rs";

/// Check `locales/<lang>/*.ftl` and write the include list
/// [`crate::catalog!`] reads. Call from `build.rs`.
///
/// # Panics
///
/// Panics (failing the build) when a file does not parse, a folder is
/// not a supported [`Lang`], a message is defined twice, or a language
/// misses or adds messages compared to [`Lang::FALLBACK`].
pub fn emit() {
    let manifest =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let out = std::env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo");
    let root = Path::new(&manifest).join(LOCALES_DIR);
    println!("cargo::rerun-if-changed={}", root.display());

    let locales = Locale::discover(&root);
    Locale::check_parity(&locales);

    let mut code = String::from("&[");
    for locale in &locales {
        let _ = write!(code, "({:?}, &[", locale.lang.tag());
        for file in &locale.files {
            let _ = write!(code, "include_str!({:?}),", file.display().to_string());
        }
        code.push_str("] as &[&str]),");
    }
    code.push(']');
    std::fs::write(Path::new(&out).join(OUTPUT), code).expect("OUT_DIR is writable");
}

/// One language's files and the message ids they define.
struct Locale {
    lang: Lang,
    files: Vec<PathBuf>,
    ids: BTreeSet<String>,
}

impl Locale {
    /// Every supported language, with no files when its folder is
    /// missing so parity reports it.
    fn discover(root: &Path) -> Vec<Self> {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                assert!(
                    !entry.path().is_dir() || Lang::ALL.iter().any(|l| l.tag() == name),
                    "{}: `{name}` is not a supported language; add it to notein_i18n::Lang",
                    root.display()
                );
            }
        }
        if !root.is_dir() {
            return Vec::new();
        }
        Lang::ALL
            .into_iter()
            .map(|lang| Self::load(lang, &root.join(lang.tag())))
            .collect()
    }

    fn load(lang: Lang, dir: &Path) -> Self {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|ext| ext == "ftl"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();

        let mut ids = BTreeSet::new();
        for file in &files {
            let source = std::fs::read_to_string(file)
                .unwrap_or_else(|err| panic!("{}: {err}", file.display()));
            let resource = fluent_syntax::parser::parse(source.as_str())
                .unwrap_or_else(|(_, errors)| panic!("{}: {errors:#?}", file.display()));
            for entry in resource.body {
                let id = match entry {
                    Entry::Message(message) => message.id.name.to_owned(),
                    Entry::Term(term) => format!("-{}", term.id.name),
                    _ => continue,
                };
                assert!(
                    ids.insert(id.clone()),
                    "{}: `{id}` is defined twice",
                    file.display()
                );
            }
        }
        Self { lang, files, ids }
    }

    fn check_parity(locales: &[Self]) {
        let Some(reference) = locales.iter().find(|l| l.lang == Lang::FALLBACK) else {
            return;
        };
        let mut report = String::new();
        for locale in locales.iter().filter(|l| l.lang != Lang::FALLBACK) {
            let missing: Vec<_> = reference.ids.difference(&locale.ids).collect();
            let extra: Vec<_> = locale.ids.difference(&reference.ids).collect();
            if !missing.is_empty() {
                let _ = writeln!(report, "{} misses: {missing:?}", locale.lang.tag());
            }
            if !extra.is_empty() {
                let _ = writeln!(
                    report,
                    "{} has keys {} lacks: {extra:?}",
                    locale.lang.tag(),
                    reference.lang.tag()
                );
            }
        }
        assert!(report.is_empty(), "translations out of sync:\n{report}");
    }
}

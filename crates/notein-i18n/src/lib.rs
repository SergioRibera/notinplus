//! Interface languages for notinplus and its plugins.
//!
//! Every crate with user-facing text keeps its strings as Fluent files
//! under `locales/<lang>/*.ftl` (one folder per [`Lang`]) and wires them
//! in two lines:
//!
//! ```ignore
//! // build.rs
//! fn main() {
//!     notein_i18n::build::emit();
//! }
//!
//! // lib.rs
//! notein_i18n::catalog!();
//! ```
//!
//! Then `tr!("home-title")` or `tr!("toast-renamed", "name" => name)`
//! anywhere in that crate returns the text in the active language.
//! The build script refuses files that do not parse and languages that
//! miss or add keys, so a string can never be shown untranslated.
//!
//! The active language is process-global ([`Lang::current`]); the
//! shell sets it from the user's setting or the device locale and
//! remounts its views when it changes.

#[cfg(feature = "build")]
pub mod build;

use std::borrow::Cow;
use std::sync::atomic::{AtomicU8, Ordering};

use fluent::concurrent::FluentBundle;
pub use fluent::{FluentArgs, FluentResource, FluentValue};

/// A language the interface is translated to. Adding one means adding
/// a variant here and a `locales/<tag>/` folder to every crate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, bincode::Encode, bincode::Decode)]
pub enum Lang {
    #[default]
    En,
    Es,
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

impl Lang {
    /// Every supported language, in the order pickers list them.
    pub const ALL: [Self; 2] = [Self::En, Self::Es];

    /// Language a missing translation falls back to, and the one used
    /// when the device asks for an unsupported language.
    pub const FALLBACK: Self = Self::En;

    /// BCP-47 primary language subtag, also the `locales/` folder name.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Es => "es",
        }
    }

    /// The language's name written in itself, for language pickers.
    #[must_use]
    pub const fn native_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Es => "Español",
        }
    }

    /// Supported language matching a locale tag such as `es-AR`,
    /// `en_US.UTF-8` or `es`; only the primary subtag counts.
    #[must_use]
    pub fn from_locale(tag: &str) -> Option<Self> {
        let primary = tag.split(['-', '_', '.', '@']).next()?;
        Self::ALL
            .into_iter()
            .find(|lang| lang.tag().eq_ignore_ascii_case(primary))
    }

    /// Language the interface is showing.
    #[must_use]
    pub fn current() -> Self {
        let index = usize::from(CURRENT.load(Ordering::Relaxed));
        Self::ALL.get(index).copied().unwrap_or(Self::FALLBACK)
    }

    /// Make `self` the language every catalog formats in.
    pub fn make_current(self) {
        CURRENT.store(self.index(), Ordering::Relaxed);
    }

    const fn index(self) -> u8 {
        match self {
            Self::En => 0,
            Self::Es => 1,
        }
    }
}

type Bundle = FluentBundle<FluentResource>;

/// One crate's strings in every [`Lang`]. Built once by [`catalog!`].
pub struct Catalog {
    bundles: Vec<(Lang, Bundle)>,
}

impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Catalog")
            .field(
                "langs",
                &self
                    .bundles
                    .iter()
                    .map(|(lang, _)| *lang)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Catalog {
    /// Catalog from Fluent sources grouped by language tag, the shape
    /// [`build::emit`] writes. Unknown tags and broken files are logged
    /// and skipped; the build script already rejected them.
    #[must_use]
    pub fn new(sources: &[(&str, &[&str])]) -> Self {
        let bundles = sources
            .iter()
            .filter_map(|(tag, files)| {
                let Some(lang) = Lang::from_locale(tag) else {
                    log::warn!(target: "i18n", "unknown locale folder {tag}");
                    return None;
                };
                Some((lang, bundle(lang, files)))
            })
            .collect();
        Self { bundles }
    }

    /// Message `id` in the active language.
    #[must_use]
    pub fn text(&self, id: &str) -> String {
        self.resolve(id, None)
    }

    /// Message `id` in the active language, with its `{ $name }`
    /// placeables filled from `args`.
    #[must_use]
    pub fn text_with(&self, id: &str, args: &FluentArgs<'_>) -> String {
        self.resolve(id, Some(args))
    }

    fn resolve(&self, id: &str, args: Option<&FluentArgs<'_>>) -> String {
        let lang = Lang::current();
        self.format(lang, id, args)
            .or_else(|| (lang != Lang::FALLBACK).then(|| self.format(Lang::FALLBACK, id, args))?)
            .map_or_else(
                || {
                    log::warn!(target: "i18n", "missing message {id}");
                    id.to_owned()
                },
                Cow::into_owned,
            )
    }

    fn format<'a>(
        &'a self,
        lang: Lang,
        id: &str,
        args: Option<&'a FluentArgs<'_>>,
    ) -> Option<Cow<'a, str>> {
        let (_, bundle) = self.bundles.iter().find(|(l, _)| *l == lang)?;
        let pattern = bundle.get_message(id)?.value()?;
        let mut errors = Vec::new();
        let text = bundle.format_pattern(pattern, args, &mut errors);
        if !errors.is_empty() {
            log::warn!(target: "i18n", "formatting {id} in {}: {errors:?}", lang.tag());
        }
        Some(text)
    }
}

fn bundle(lang: Lang, files: &[&str]) -> Bundle {
    let mut bundle = Bundle::new_concurrent(lang.tag().parse().into_iter().collect());
    // Unicode isolation marks around placeables render as stray glyphs
    // in the canvas text engine; every supported language is LTR.
    bundle.set_use_isolating(false);
    for source in files {
        let resource =
            FluentResource::try_new((*source).to_owned()).unwrap_or_else(|(res, errors)| {
                log::warn!(target: "i18n", "parsing {} locale: {errors:?}", lang.tag());
                res
            });
        if let Err(errors) = bundle.add_resource(resource) {
            log::warn!(target: "i18n", "loading {} locale: {errors:?}", lang.tag());
        }
    }
    bundle
}

/// Declare this crate's `LOCALES` catalog from the files
/// [`build::emit`] found. Invoke once at the crate root.
#[macro_export]
macro_rules! catalog {
    () => {
        /// This crate's interface strings in every language.
        pub(crate) static LOCALES: ::std::sync::LazyLock<$crate::Catalog> =
            ::std::sync::LazyLock::new(|| {
                $crate::Catalog::new(include!(concat!(
                    env!("OUT_DIR"),
                    "/notein_i18n_locales.rs"
                )))
            });
    };
}

/// Text of a message from the invoking crate's [`catalog!`], in the
/// active language: `tr!("id")` or `tr!("id", "arg" => value, …)`.
// `crate::LOCALES` is the invoking crate's catalog on purpose.
#[allow(clippy::crate_in_macro_def)]
#[macro_export]
macro_rules! tr {
    ($id:literal $(,)?) => {
        crate::LOCALES.text($id)
    };
    ($id:literal, $($key:literal => $value:expr),+ $(,)?) => {{
        let mut args = $crate::FluentArgs::new();
        $(args.set($key, $value);)+
        crate::LOCALES.text_with($id, &args)
    }};
}

#[cfg(test)]
mod tests {
    use super::{Catalog, FluentArgs, Lang};

    const EN: &str = "greet = Hello, { $name }!\nfiles = { $n ->\n [one] One file\n *[other] { $n } files\n}\nonly-en = Only here\n";
    const ES: &str = "greet = ¡Hola, { $name }!\nfiles = { $n ->\n [one] Un archivo\n *[other] { $n } archivos\n}\n";

    fn catalog() -> Catalog {
        Catalog::new(&[("en", &[EN]), ("es", &[ES])])
    }

    #[test]
    fn locale_tags_map_by_primary_subtag() {
        assert_eq!(Lang::from_locale("es-AR"), Some(Lang::Es));
        assert_eq!(Lang::from_locale("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(Lang::from_locale("ES"), Some(Lang::Es));
        assert_eq!(Lang::from_locale("de-DE"), None);
        assert_eq!(Lang::from_locale(""), None);
    }

    // Single test so the process-global language is not raced by a
    // sibling test.
    #[test]
    fn formats_in_the_current_language_with_fallback() {
        let catalog = catalog();
        let mut args = FluentArgs::new();
        args.set("name", "Ana");
        let mut one = FluentArgs::new();
        one.set("n", 1);
        let mut many = FluentArgs::new();
        many.set("n", 3);

        Lang::Es.make_current();
        assert_eq!(Lang::current(), Lang::Es);
        assert_eq!(catalog.text_with("greet", &args), "¡Hola, Ana!");
        assert_eq!(catalog.text_with("files", &one), "Un archivo");
        assert_eq!(catalog.text_with("files", &many), "3 archivos");
        assert_eq!(catalog.text("only-en"), "Only here");
        assert_eq!(catalog.text("nowhere"), "nowhere");

        Lang::En.make_current();
        assert_eq!(catalog.text_with("greet", &args), "Hello, Ana!");
        assert_eq!(catalog.text_with("files", &many), "3 files");
    }
}

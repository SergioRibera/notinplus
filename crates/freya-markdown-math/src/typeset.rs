//! LaTeX to SVG: `MiTeX` turns the TeX into Typst math, Typst lays it out.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::sync::LazyLock;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, Source};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

/// Typst sources defining what `MiTeX`'s output calls, by path.
const SCOPE: &[(&str, &str)] = &[
    ("mitex/mod.typ", include_str!("../assets/mitex/mod.typ")),
    (
        "mitex/prelude.typ",
        include_str!("../assets/mitex/prelude.typ"),
    ),
    (
        "mitex/latex/standard.typ",
        include_str!("../assets/mitex/latex/standard.typ"),
    ),
];

/// A math font and a text font for `\text{}`; nothing else is embedded.
const FONTS: [&[u8]; 2] = [
    include_bytes!("../assets/fonts/NewCMMath-Regular.otf"),
    include_bytes!("../assets/fonts/LibertinusSerif-Regular.otf"),
];

/// Formulas kept typeset, so redrawing a note costs nothing.
const CACHE_LIMIT: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MathError {
    /// The TeX could not be read.
    Convert(String),
    /// Typst could not lay it out.
    Typeset(String),
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Convert(message) | Self::Typeset(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for MathError {}

/// A typeset formula.
#[derive(Clone, Debug, PartialEq)]
pub struct Formula {
    pub svg: freya::prelude::Bytes,
    /// Size in logical pixels.
    pub width: f32,
    pub height: f32,
}

/// What a formula is typeset with.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Style {
    pub display: bool,
    /// Font size in logical pixels, as `f32` bits.
    size: u32,
    /// `RRGGBB`.
    color: String,
}

impl Style {
    #[must_use]
    pub fn new(display: bool, size: f32, color: [u8; 3]) -> Self {
        let [r, g, b] = color;
        Self {
            display,
            size: size.to_bits(),
            color: format!("{r:02x}{g:02x}{b:02x}"),
        }
    }

    const fn size(&self) -> f32 {
        f32::from_bits(self.size)
    }
}

thread_local! {
    static TYPESET: RefCell<HashMap<(String, Style), Result<Formula, MathError>>> =
        RefCell::new(HashMap::new());
}

/// `tex` typeset with `style`, from the cache when it was before.
///
/// # Errors
/// [`MathError`] when the TeX cannot be read or laid out.
pub fn typeset(tex: &str, style: &Style) -> Result<Formula, MathError> {
    let key = (tex.to_owned(), style.clone());
    if let Some(done) = TYPESET.with(|cache| cache.borrow().get(&key).cloned()) {
        return done;
    }
    let done = typeset_now(tex, style);
    TYPESET.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, done.clone());
    });
    done
}

fn typeset_now(tex: &str, style: &Style) -> Result<Formula, MathError> {
    let math = mitex::convert_math(tex, None).map_err(|message| {
        MathError::Convert(
            message
                .strip_prefix("error: ")
                .unwrap_or(&message)
                .to_owned(),
        )
    })?;
    let literal = math
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    // Typst points are taken as logical pixels, so a formula matches the
    // text around it.
    let main = format!(
        "#import \"/mitex/mod.typ\": mitex-scope\n\
         #set page(width: auto, height: auto, margin: 1pt, fill: none)\n\
         #set text(size: {size}pt, fill: rgb(\"#{color}\"), font: \"Libertinus Serif\")\n\
         #show math.equation: set text(font: \"New Computer Modern Math\")\n\
         #math.equation(block: {display}, eval(\"$\" + \"{literal}\" + \"$\", scope: mitex-scope))\n",
        size = style.size(),
        color = style.color,
        display = style.display,
    );
    let world = MathWorld {
        base: &BASE,
        main: Source::detached(main),
    };
    let document = typst::compile::<PagedDocument>(&world).output;
    // Typst memoizes across runs; keep that bounded.
    typst::comemo::evict(30);
    let document = document.map_err(|errors| {
        MathError::Typeset(
            errors
                .first()
                .map_or_else(String::new, |error| error.message.to_string()),
        )
    })?;
    let page = document
        .pages()
        .first()
        .ok_or_else(|| MathError::Typeset(String::new()))?;
    let svg = inline_symbols(&typst_svg::svg(page, &typst_svg::SvgOptions::default()));
    let (width, height) = svg_size(&svg).ok_or_else(|| MathError::Typeset(String::new()))?;
    Ok(Formula {
        svg: freya::prelude::Bytes::from(svg.into_bytes()),
        width,
        height,
    })
}

/// `svg` with every `<use>` of a `<symbol>` replaced by a copy of it.
///
/// Typst draws each glyph once as a symbol and places it with `<use>`;
/// Skia's SVG renderer draws no symbols.
fn inline_symbols(svg: &str) -> String {
    let mut symbols = HashMap::new();
    let mut rest = String::with_capacity(svg.len());
    let mut cursor = svg;
    while let Some(start) = cursor.find("<symbol") {
        rest.push_str(&cursor[..start]);
        let tail = &cursor[start..];
        let Some(end) = tail.find("</symbol>") else {
            break;
        };
        let element = &tail[..end];
        if let (Some(id), Some(open)) = (attribute(element, "id"), element.find('>')) {
            symbols.insert(id.to_owned(), element[open + 1..].to_owned());
        }
        cursor = &tail[end + "</symbol>".len()..];
    }
    rest.push_str(cursor);

    let mut out = String::with_capacity(rest.len() * 2);
    let mut cursor = rest.as_str();
    while let Some(start) = cursor.find("<use") {
        out.push_str(&cursor[..start]);
        let tail = &cursor[start..];
        let Some(end) = tail.find("/>") else {
            break;
        };
        let element = &tail[..end];
        let target = attribute(element, "xlink:href")
            .or_else(|| attribute(element, "href"))
            .and_then(|href| href.strip_prefix('#'))
            .and_then(|id| symbols.get(id));
        if let Some(inner) = target {
            let x = attribute(element, "x").unwrap_or("0");
            let y = attribute(element, "y").unwrap_or("0");
            let paint = ["fill", "fill-rule", "stroke", "stroke-width"]
                .into_iter()
                .filter_map(|name| {
                    attribute(element, name).map(|value| format!(" {name}=\"{value}\""))
                })
                .collect::<String>();
            let _ = write!(out, "<g transform=\"translate({x} {y})\"{paint}>{inner}</g>");
        } else {
            out.push_str(&tail[..end + 2]);
        }
        cursor = &tail[end + 2..];
    }
    out.push_str(cursor);
    out
}

/// Value of the attribute `name` in the start tag `element`.
fn attribute<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let head = &element[..element.find('>').unwrap_or(element.len())];
    let start = head.find(&format!(" {name}=\""))? + name.len() + 3;
    let value = &head[start..];
    Some(&value[..value.find('"')?])
}

/// `width` and `height` of the root `<svg>`, in points.
fn svg_size(svg: &str) -> Option<(f32, f32)> {
    let head = &svg[..svg.find('>')?];
    let attribute = |name: &str| -> Option<f32> {
        let start = head.find(&format!(" {name}=\""))? + name.len() + 3;
        let value = &head[start..];
        value[..value.find('"')?]
            .trim_end_matches("pt")
            .parse()
            .ok()
    };
    Some((attribute("width")?, attribute("height")?))
}

/// What every compilation shares: the standard library and the fonts.
struct Base {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

static BASE: LazyLock<Base> = LazyLock::new(|| {
    let fonts: Vec<Font> = FONTS
        .into_iter()
        .flat_map(|data| Font::iter(Bytes::new(data)))
        .collect();
    Base {
        library: LazyHash::new(Library::builder().build()),
        book: LazyHash::new(FontBook::from_fonts(&fonts)),
        fonts,
    }
});

/// One formula as a Typst document.
struct MathWorld<'a> {
    base: &'a Base,
    main: Source,
}

impl MathWorld<'_> {
    fn path(id: FileId) -> String {
        id.get().vpath().get_without_slash().to_owned()
    }
}

impl World for MathWorld<'_> {
    fn library(&self) -> &LazyHash<Library> {
        &self.base.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.base.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            return Ok(self.main.clone());
        }
        let path = Self::path(id);
        SCOPE
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, text)| Source::new(id, (*text).to_owned()))
            .ok_or_else(|| FileError::NotFound(path.into()))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        Err(FileError::NotFound(Self::path(id).into()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.base.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{MathError, Style, typeset};

    fn style() -> Style {
        Style::new(true, 16., [228, 230, 235])
    }

    #[test]
    fn typesets_common_latex() {
        for tex in [
            r"\frac{a}{b}",
            r"\sqrt{x^2 + y^2}",
            r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2}",
            r"\int_0^1 f(x)\,dx \xrightarrow{n} \mathbb{R}",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            r"\alpha + \beta \text{ texto }",
        ] {
            let formula = typeset(tex, &style()).unwrap_or_else(|err| panic!("{tex}: {err}"));
            assert!(formula.width > 0. && formula.height > 0., "{tex}");
        }
    }

    #[test]
    fn broken_latex_is_an_error() {
        assert!(matches!(
            typeset(r"\unknowncmd{x}", &style()),
            Err(MathError::Convert(message)) if message == r"unknown command: \unknowncmd"
        ));
        assert!(matches!(
            typeset(r"\left( x", &style()),
            Err(MathError::Typeset(_))
        ));
    }
}

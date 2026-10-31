//! Mermaid diagrams for [`freya_markdown_editor`].
//!
//! Add [`MermaidDiagrams`] as a decorator and every ` ```mermaid ` block is
//! drawn under its source while editing and instead of it while reading.
//! Layout and SVG come from `mermaid-svg`, pure Rust with no fonts or
//! network needed, colored after the editor theme.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use freya::prelude::*;
use freya_markdown_editor::{CodeContext, Decorator, EditorColors};
use mermaid_svg::Theme;

/// Diagrams kept drawn, so redrawing a note costs nothing.
const CACHE_LIMIT: usize = 64;

/// Pixels drawn per logical pixel, so diagrams stay sharp on dense screens.
const DENSITY: f32 = 2.0;

/// The only font diagrams are drawn with; `mermaid-svg` sizes labels for it.
const FONT_FAMILY: &str = "DejaVu Sans";

static FONTS: LazyLock<Arc<resvg::usvg::fontdb::Database>> = LazyLock::new(|| {
    #[cfg(feature = "sismo")]
    sismo::scope!("diagram::load_fonts");
    let mut fonts = resvg::usvg::fontdb::Database::new();
    fonts.load_font_data(include_bytes!("../assets/fonts/DejaVuSans.ttf").to_vec());
    fonts.load_font_data(include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf").to_vec());
    fonts.set_sans_serif_family(FONT_FAMILY);
    Arc::new(fonts)
});

/// A drawn diagram: a PNG and its size in logical pixels.
#[derive(Clone, Debug, PartialEq)]
struct Drawing {
    key: u64,
    png: Bytes,
    width: f32,
    height: f32,
}

/// A diagram source and the colors it was drawn with.
type Key = (String, [u32; 4]);

thread_local! {
    static DRAWN: RefCell<HashMap<Key, Result<Drawing, String>>> =
        RefCell::new(HashMap::new());
}

/// Draws ` ```mermaid ` blocks. See the [crate] docs.
#[derive(Clone, Copy, Debug, Default)]
pub struct MermaidDiagrams;

impl Decorator for MermaidDiagrams {
    #[cfg_attr(feature = "sismo", sismo::function("diagram::render"))]
    fn render_preview(&self, code: &CodeContext<'_>) -> Option<Element> {
        if !code.language.trim().eq_ignore_ascii_case("mermaid") || code.code.trim().is_empty() {
            return None;
        }
        let colors = &code.theme.colors;
        Some(match draw(code.code, colors) {
            Ok(drawing) => {
                // Shrink what does not fit rather than overflow the note.
                let scale = if code.max_width > 0. && drawing.width > code.max_width {
                    code.max_width / drawing.width
                } else {
                    1.
                };
                #[cfg(feature = "sismo")]
                sismo::scope!("diagram::image_viewer");
                ImageViewer::new((drawing.key, drawing.png))
                    .width(Size::px(drawing.width * scale))
                    .height(Size::px(drawing.height * scale))
                    .into_element()
            }
            Err(error) => label()
                .font_size(code.theme.px(12.5))
                .color(colors.caution)
                .text(format!("⚠ {error}"))
                .into_element(),
        })
    }
}

#[cfg_attr(feature = "sismo", sismo::function("diagram::draw"))]
fn draw(source: &str, colors: &EditorColors) -> Result<Drawing, String> {
    let key = (
        source.to_owned(),
        [
            colors.text,
            colors.surface,
            colors.muted,
            colors.code_background,
        ]
        .map(u32::from),
    );
    if let Some(done) = DRAWN.with(|cache| cache.borrow().get(&key).cloned()) {
        #[cfg(feature = "sismo")]
        sismo::instant!("diagram::cache_hit");
        return done;
    }
    #[cfg(feature = "sismo")]
    sismo::instant!("diagram::cache_miss");
    let svg = {
        #[cfg(feature = "sismo")]
        sismo::scope!("diagram::mermaid_svg");
        mermaid_svg::render_with(source, &theme(colors))
    };
    let done = svg
        .map_err(|error| error.to_string())
        .and_then(|svg| rasterize(&svg));
    DRAWN.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= CACHE_LIMIT {
            #[cfg(feature = "sismo")]
            sismo::scope!("diagram::cache_clear");
            cache.clear();
        }
        cache.insert(key, done.clone());
        #[cfg(feature = "sismo")]
        sismo::counter!("diagram.cache_len", cache.len());
    });
    done
}

fn hex(color: Color) -> Cow<'static, str> {
    Cow::Owned(format!(
        "#{:02x}{:02x}{:02x}",
        color.r(),
        color.g(),
        color.b()
    ))
}

/// Mermaid colors taken from the editor's, so diagrams follow light and
/// dark themes.
#[cfg_attr(feature = "sismo", sismo::function("diagram::theme"))]
fn theme(colors: &EditorColors) -> Theme {
    let surface = colors.surface;
    let dark = u16::from(surface.r()) + u16::from(surface.g()) + u16::from(surface.b()) < 384;
    let (node, line) = (hex(colors.code_background), hex(colors.muted));
    let palette: Vec<Cow<'static, str>> = [
        colors.note,
        colors.tip,
        colors.warning,
        colors.important,
        colors.caution,
        colors.accent,
    ]
    .into_iter()
    .map(hex)
    .collect();
    Theme {
        fg: hex(colors.text),
        fg_muted: line.clone(),
        bg: Cow::Borrowed("transparent"),
        actor_fill: node.clone(),
        actor_stroke: line.clone(),
        lifeline: line.clone(),
        arrow_stroke: line.clone(),
        note_fill: hex(colors.popup_background),
        note_stroke: hex(colors.border),
        activation_fill: node.clone(),
        activation_stroke: line.clone(),
        frame_label_fill: node.clone(),
        flow_node_fill: node,
        flow_node_stroke: line.clone(),
        flow_edge_stroke: line,
        flow_label_bg: hex(colors.surface),
        flow_cluster_fill: hex(colors.surface),
        flow_cluster_stroke: hex(colors.border),
        pie_palette: Cow::Owned(palette.clone()),
        xychart_palette: Cow::Owned(palette),
        font_family: Cow::Borrowed(FONT_FAMILY),
        responsive: false,
        ..if dark {
            Theme::dark()
        } else {
            Theme::default()
        }
    }
}

/// `svg` drawn as a PNG at [`DENSITY`].
#[cfg_attr(feature = "sismo", sismo::function("diagram::rasterize"))]
fn rasterize(svg: &str) -> Result<Drawing, String> {
    use resvg::tiny_skia::{Pixmap, Transform};
    let options = resvg::usvg::Options {
        font_family: FONT_FAMILY.to_owned(),
        fontdb: FONTS.clone(),
        ..resvg::usvg::Options::default()
    };
    let tree = {
        #[cfg(feature = "sismo")]
        sismo::scope!("diagram::usvg_parse");
        resvg::usvg::Tree::from_str(svg, &options).map_err(|error| error.to_string())?
    };
    let size = tree.size();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "diagram sizes are small and positive"
    )]
    let (width, height) = (
        (size.width() * DENSITY).ceil() as u32,
        (size.height() * DENSITY).ceil() as u32,
    );
    let mut pixmap = Pixmap::new(width.max(1), height.max(1)).ok_or("empty diagram")?;
    {
        #[cfg(feature = "sismo")]
        sismo::scope!("diagram::resvg_render");
        resvg::render(
            &tree,
            Transform::from_scale(DENSITY, DENSITY),
            &mut pixmap.as_mut(),
        );
    }
    #[cfg(feature = "sismo")]
    sismo::counter!("diagram.pixels", u64::from(width) * u64::from(height));
    let png = {
        #[cfg(feature = "sismo")]
        sismo::scope!("diagram::encode_png");
        pixmap.encode_png().map_err(|error| error.to_string())?
    };
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(svg, &mut hasher);
    Ok(Drawing {
        key: std::hash::Hasher::finish(&hasher),
        png: Bytes::from(png),
        width: size.width(),
        height: size.height(),
    })
}

#[cfg(test)]
mod tests {
    use freya_markdown_editor::EditorColors;

    use super::draw;

    #[test]
    fn draws_and_reports_errors() {
        let colors = EditorColors::dark();
        let drawing = draw(
            "flowchart TD\n  A[Start] --> B{Ok?}\n  B -->|Yes| C[Done]",
            &colors,
        )
        .expect("flowchart");
        assert!(drawing.width > 0. && drawing.height > 0.);
        assert!(draw("flowchart TD\n  A[Start --> B", &colors).is_err());
    }
}

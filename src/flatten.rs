//! A canvas doc painted onto a Skia canvas without the UI: card covers
//! and the PDFs of "read in other apps".
//!
//! Draws what the board paints from world units itself (ink and images)
//! and the text boxes, laid out by [`MarkdownPicture`], which follows the
//! note editor's look without its UI.

use freya::prelude::Color;
use freya_canvas_bg::{
    BgPaintCtx, CanvasBackground, DotGridBackground, GridBackground, LinedBackground,
    Rect as WorldRect, RedrawHandle,
};
use freya_engine::prelude::{
    Canvas, Color as SkColor, FontCollection, FontMgr, Paint, Rect, SkData, SkImage,
    TypefaceFontProvider,
};
use freya_markdown_editor::{EditorTheme, MarkdownPicture};
use notin_plugin::create::Paper;

use crate::canvas_text::{BOX_PADDING, FONT_BYTES, FONT_FAMILY};
use crate::doc::Doc;
use crate::image_box::ImageBox;
use crate::library::{ItemId, bodies};
use crate::render::BrushRegistry;
use crate::text_box::{TEXT_BASE_SIZE, TextBox};

/// World bounds `(min_x, min_y, max_x, max_y)` of everything [`paint`]
/// draws, `None` when there is nothing.
#[must_use]
#[sismo::function("flatten::bounds")]
pub fn bounds(doc: &Doc) -> Option<(f32, f32, f32, f32)> {
    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    let mut grow = |x0: f32, y0: f32, x1: f32, y1: f32| {
        bounds = Some(bounds.map_or((x0, y0, x1, y1), |(a, b, c, d)| {
            (a.min(x0), b.min(y0), c.max(x1), d.max(y1))
        }));
    };
    for layer in doc.layers.iter().filter(|l| l.visible) {
        for p in layer
            .strokes
            .iter()
            .flat_map(|s| s.display_points().into_owned())
        {
            grow(p.x, p.y, p.x, p.y);
        }
        for image in &layer.images {
            let (x0, y0, x1, y1) = image.frame.bounds(image.height);
            grow(x0, y0, x1, y1);
        }
        for text in layer.texts.iter().filter(|t| !t.is_blank()) {
            // The height does not depend on the colour.
            let height = LaidText::new(text, [0; 4]).height;
            let (x0, y0, x1, y1) = text.frame.bounds(height);
            grow(x0, y0, x1, y1);
        }
    }
    bounds
}

/// Draw the lines, grid or dots of `paper` over the world rect `area`
/// (`min_x, min_y, max_x, max_y`), the way the board paints them under the
/// ink. Blank paper draws nothing.
#[sismo::function("flatten::paint_paper")]
pub fn paint_paper(canvas: &Canvas, paper: Paper, area: (f32, f32, f32, f32)) {
    let background: Box<dyn CanvasBackground> = match paper {
        Paper::Blank => return,
        Paper::Lines => Box::new(LinedBackground::new(SkColor::TRANSPARENT)),
        Paper::Grid => Box::new(GridBackground::new(SkColor::TRANSPARENT)),
        Paper::Dots => Box::new(DotGridBackground::new(SkColor::TRANSPARENT)),
    };
    let mut cx = BgPaintCtx {
        canvas,
        visible: WorldRect::from_corners((area.0, area.1), (area.2, area.3)),
        scale: 1.0,
        redraw: RedrawHandle::new(|| {}),
    };
    background.paint(&mut cx);
}

/// Draw `doc` in world units under the canvas's current transform. Images
/// come from `item`'s `assets/` folder; one that cannot be read is skipped.
/// Text boxes that follow the theme are dark on light `paper` (RGBA) and
/// light on dark.
#[sismo::function("flatten::paint")]
pub fn paint(canvas: &Canvas, doc: &Doc, item: ItemId, paper: [u8; 4]) {
    let registry = BrushRegistry::default();
    for layer in doc.layers.iter().filter(|l| l.visible) {
        let opacity = layer.opacity.clamp(0.0, 1.0);
        // Images under the ink of their layer, as the board paints them.
        for image in &layer.images {
            if let Some(decoded) = load_image(item, image) {
                draw_image(canvas, &decoded, image, opacity);
            }
        }
        {
            sismo::scope!("flatten::strokes");
            for stroke in &layer.strokes {
                let Some(preset) = doc.preset(stroke.brush) else {
                    continue;
                };
                let path = registry.stroke_path(preset, stroke);
                let paint: Paint = registry.brush(preset.kind).paint(preset, stroke.color);
                canvas.draw_path(&path, &paint);
            }
        }
        let texts: Vec<&TextBox> = layer.texts.iter().filter(|t| !t.is_blank()).collect();
        if texts.is_empty() {
            continue;
        }
        sismo::scope!("flatten::texts");
        let faded = opacity < 0.999;
        if faded {
            canvas.save_layer_alpha_f(None, opacity);
        }
        for text in texts {
            LaidText::new(text, paper).paint(canvas, text);
        }
        if faded {
            canvas.restore();
        }
    }
}

/// Fonts text boxes are laid out with: the canvas hand, every plugin's
/// fonts and the system's.
fn text_fonts() -> FontCollection {
    let mut provider = TypefaceFontProvider::new();
    let loader = FontMgr::custom_empty().unwrap_or_default();
    let mut add = |family: &str, bytes: &[u8]| {
        if let Some(typeface) = loader.new_from_data(SkData::new_copy(bytes), None) {
            provider.register_typeface(typeface, Some(family));
        } else {
            tracing::warn!("flatten: font {family} does not load");
        }
    };
    add(FONT_FAMILY, FONT_BYTES);
    let plugin_fonts = crate::plugins::catalog().fonts();
    for font in plugin_fonts {
        add(font.family, font.bytes);
    }
    let mut fonts = FontCollection::new();
    fonts.set_default_font_manager(FontMgr::default(), None);
    fonts.set_dynamic_font_manager(FontMgr::from(provider));
    fonts
}

thread_local! {
    static TEXT_FONTS: FontCollection = text_fonts();
}

/// A text box laid out as the canvas shows it at 100%.
struct LaidText {
    picture: MarkdownPicture,
    /// Height of the box, padding included, in world units.
    height: f32,
}

impl LaidText {
    /// Lay `text` out over a backdrop of colour `paper` (RGBA), whose
    /// lightness picks the text colour of boxes that follow the theme.
    #[sismo::function("flatten::lay_text")]
    fn new(text: &TextBox, paper: [u8; 4]) -> Self {
        let base = if on_dark(paper) {
            EditorTheme::dark()
        } else {
            EditorTheme::light()
        };
        let mut theme = base.clone().zoomed(TEXT_BASE_SIZE / base.body_size);
        theme.fonts.insert(0, FONT_FAMILY.into());
        if let Some([r, g, b, a]) = text.custom_color() {
            theme.colors.text = Color::from_argb(a, r, g, b);
        }
        let (pad_y, pad_x) = BOX_PADDING;
        let width = pad_x.mul_add(-2.0, text.frame.width);
        let picture =
            TEXT_FONTS.with(|fonts| MarkdownPicture::layout(&text.markdown, &theme, width, fonts));
        let height = pad_y.mul_add(2.0, picture.height());
        Self { picture, height }
    }

    /// Draw it in `text`'s frame, turned around the frame's centre.
    fn paint(&self, canvas: &Canvas, text: &TextBox) {
        let (cx, cy) = text.frame.center(self.height);
        let (pad_y, pad_x) = BOX_PADDING;
        canvas.save();
        canvas.translate((cx, cy));
        canvas.rotate(text.frame.rotation, None);
        self.picture.paint(
            canvas,
            (
                text.frame.width.mul_add(-0.5, pad_x),
                self.height.mul_add(-0.5, pad_y),
            ),
        );
        canvas.restore();
    }
}

/// Whether `paper` (RGBA) is dark enough to need light text. Transparent
/// paper is read as white, the page most exports land on.
fn on_dark([r, g, b, a]: [u8; 4]) -> bool {
    let luma = 0.0722f32.mul_add(
        f32::from(b),
        0.2126f32.mul_add(f32::from(r), 0.7152 * f32::from(g)),
    );
    a > 0 && luma < 128.0
}

#[sismo::function("flatten::load_image")]
fn load_image(item: ItemId, image: &ImageBox) -> Option<SkImage> {
    let bytes = bodies::read_asset(item, &image.asset)
        .inspect_err(|err| tracing::warn!("flatten: image {}: {err}", image.asset))
        .ok()?;
    if !crate::editor::images::decodable(&bytes) {
        tracing::warn!("flatten: image {} is too big to draw", image.asset);
        return None;
    }
    SkImage::from_encoded(SkData::new_copy(&bytes))
}

/// Draw `decoded` stretched over `image`'s frame, turned around the
/// frame's centre.
#[sismo::function("flatten::draw_image")]
fn draw_image(canvas: &Canvas, decoded: &SkImage, image: &ImageBox, opacity: f32) {
    let (width, height) = (image.frame.width, image.height);
    let (cx, cy) = image.frame.center(height);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_alpha_f(opacity);
    canvas.save();
    canvas.translate((cx, cy));
    canvas.rotate(image.frame.rotation, None);
    canvas.draw_image_rect(
        decoded,
        None,
        Rect::from_xywh(-width * 0.5, -height * 0.5, width, height),
        &paint,
    );
    canvas.restore();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc_op::DocOp;
    use crate::image_box::ImageBoxId;
    use crate::text_box::TextFrame;

    #[test]
    fn bounds_cover_images_on_visible_layers() {
        let mut doc = Doc::default();
        doc.emit(DocOp::AddImage {
            layer_id: 0,
            image: ImageBox {
                id: ImageBoxId::new_v4(),
                frame: TextFrame {
                    x: 10.0,
                    y: 20.0,
                    width: 100.0,
                    rotation: 0.0,
                },
                height: 50.0,
                asset: "a.png".into(),
                source: None,
            },
        });
        assert_eq!(bounds(&doc), Some((10.0, 20.0, 110.0, 70.0)));
        doc.set_layer_visible(0, false);
        assert_eq!(bounds(&doc), None);
    }

    /// RGBA pixels of `doc` painted on white, 100 × 100 world units.
    fn render(doc: &Doc) -> Vec<u8> {
        let mut surface = freya_engine::prelude::raster_n32_premul((100, 100)).expect("raster");
        surface
            .canvas()
            .clear(freya_engine::prelude::SkColor::WHITE);
        paint(surface.canvas(), doc, ItemId::default(), [255; 4]);
        let image = surface.image_snapshot();
        let pixmap = image.peek_pixels().expect("raster pixels");
        pixmap.bytes().expect("pixel bytes").to_vec()
    }

    /// Whether the pixel at `(x, y)` is inked (darker than paper).
    fn inked(pixels: &[u8], x: usize, y: usize) -> bool {
        let at = (y * 100 + x) * 4;
        pixels[at..at + 3].iter().any(|c| *c < 200)
    }

    fn line_doc() -> (Doc, crate::ids::StrokeId) {
        let mut doc = Doc::default();
        let brush = doc
            .register_brush(crate::brush::BrushPreset::pen())
            .expect("brush");
        let points = (0..=16u8)
            .map(|i| crate::brush::InkPoint::new(f32::from(i).mul_add(5.0, 10.0), 20.0, 200, 0, 0))
            .collect();
        let id = doc.push_stroke(brush, [10, 10, 10, 255], points);
        (doc, id)
    }

    fn text_doc(markdown: &str, color: [u8; 4]) -> Doc {
        let mut doc = Doc::default();
        doc.emit(DocOp::AddText {
            layer_id: 0,
            text: TextBox {
                id: crate::text_box::TextBoxId::new_v4(),
                frame: TextFrame {
                    x: 20.0,
                    y: 20.0,
                    width: 260.0,
                    rotation: 0.0,
                },
                markdown: markdown.to_owned(),
                color,
            },
        });
        doc
    }

    /// RGBA pixels of `doc` painted on `paper`, 300 × 200 world units.
    /// Also written as `<name>.png` under `$FLATTEN_SNAPSHOT_DIR` when set.
    fn render_text(doc: &Doc, paper: [u8; 4], name: &str) -> Vec<u8> {
        crate::paths::sandbox();
        let mut surface = freya_engine::prelude::raster_n32_premul((300, 200)).expect("raster");
        let [r, g, b, a] = paper;
        surface.canvas().clear(SkColor::from_argb(a, r, g, b));
        paint(surface.canvas(), doc, ItemId::default(), paper);
        let image = surface.image_snapshot();
        if let Some(dir) = std::env::var_os("FLATTEN_SNAPSHOT_DIR") {
            let png = image
                .encode(None, freya_engine::prelude::EncodedImageFormat::PNG, None)
                .expect("png");
            std::fs::write(
                std::path::Path::new(&dir).join(format!("{name}.png")),
                png.as_bytes(),
            )
            .expect("write snapshot");
        }
        let pixmap = image.peek_pixels().expect("raster pixels");
        pixmap.bytes().expect("pixel bytes").to_vec()
    }

    /// Pixels of `pixels` (300 wide) inside rows `rows` that differ from
    /// `paper`.
    fn marked(pixels: &[u8], rows: std::ops::Range<usize>, paper: [u8; 4]) -> usize {
        rows.flat_map(|y| (0..300).map(move |x| (y * 300 + x) * 4))
            .filter(|&at| {
                pixels[at..at + 3]
                    .iter()
                    .zip(&paper)
                    .any(|(c, p)| c.abs_diff(*p) > 60)
            })
            .count()
    }

    #[test]
    fn text_boxes_paint_and_count_towards_the_bounds() {
        crate::paths::sandbox();
        let doc = text_doc(
            "# Título\n\nUn **texto** con *estilo*.\n\n- uno\n- dos",
            crate::text_box::AUTO_TEXT_COLOR,
        );
        let (x0, y0, x1, y1) = bounds(&doc).expect("text has bounds");
        assert!((x0 - 20.0).abs() < 0.5 && (y0 - 20.0).abs() < 0.5);
        assert!((x1 - 280.0).abs() < 0.5);
        assert!(y1 > 100.0, "heading, paragraph and list stack up: {y1}");

        let white = [255; 4];
        let pixels = render_text(&doc, white, "text_on_white");
        assert!(marked(&pixels, 20..60, white) > 50, "heading drawn");
        assert_eq!(marked(&pixels, 0..18, white), 0, "nothing above the box");
    }

    #[test]
    fn theme_text_turns_light_on_dark_paper() {
        let doc = text_doc("Hola", crate::text_box::AUTO_TEXT_COLOR);
        let dark = [30, 30, 36, 255];
        let pixels = render_text(&doc, dark, "text_on_dark");
        let brightest = (20..60)
            .flat_map(|y| (0..300).map(move |x| (y * 300 + x) * 4))
            .map(|at| pixels[at])
            .max();
        assert!(brightest > Some(150), "light text: {brightest:?}");
    }

    #[test]
    fn corrected_ink_paints_where_it_is_shown() {
        let (mut doc, id) = line_doc();
        let raw = render(&doc);
        assert!(inked(&raw, 50, 20));
        assert!(!inked(&raw, 50, 60));
        doc.emit(DocOp::SetStrokeFix {
            id,
            fix: Some(notin_beautify::StrokeFix {
                affine: notin_beautify::Affine::translation(0.0, 40.0),
                smooth: notin_beautify::Smooth::Normal,
            }),
        });
        let fixed = render(&doc);
        assert!(!inked(&fixed, 50, 20));
        assert!(inked(&fixed, 50, 60));
    }

    #[test]
    fn a_synced_correction_paints_the_same() {
        let (mut doc, id) = line_doc();
        let mut replica =
            Doc::from_snapshot(&doc.snapshot_bytes().expect("snapshot")).expect("load");
        let since = replica.version();
        doc.emit(DocOp::SetStrokeShape {
            id,
            shape: Some(notin_beautify::StrokeShape::plain(
                notin_beautify::ShapeGeom::Ellipse {
                    center: notin_beautify::Pt::new(50.0, 50.0),
                    rx: 30.0,
                    ry: 20.0,
                    rotation: 0.0,
                },
                false,
            )),
        });
        replica
            .import_updates(&doc.updates_since(&since).expect("updates"))
            .expect("merge");
        assert_eq!(render(&doc), render(&replica));
        assert!(inked(&render(&replica), 80, 50));
    }
}

//! A canvas doc painted onto a Skia canvas without the UI: card covers
//! and the PDFs of "read in other apps".
//!
//! Draws what the board paints from world units itself: ink and images.
//! Text boxes are laid out by the note editor and have no painter here
//! yet, so they neither draw nor count towards the bounds.

use freya_engine::prelude::{Canvas, Paint, Rect, SkData, SkImage};

use crate::doc::Doc;
use crate::image_box::ImageBox;
use crate::library::{ItemId, bodies};
use crate::render::BrushRegistry;

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
        for p in layer.strokes.iter().flat_map(|s| s.points.iter()) {
            grow(p.x, p.y, p.x, p.y);
        }
        for image in &layer.images {
            let (x0, y0, x1, y1) = image.frame.bounds(image.height);
            grow(x0, y0, x1, y1);
        }
    }
    bounds
}

/// Draw `doc` in world units under the canvas's current transform. Images
/// come from `item`'s `assets/` folder; one that cannot be read is skipped.
#[sismo::function("flatten::paint")]
pub fn paint(canvas: &Canvas, doc: &Doc, item: ItemId) {
    let registry = BrushRegistry::default();
    for layer in doc.layers.iter().filter(|l| l.visible) {
        sismo::scope!("flatten::strokes");
        for stroke in &layer.strokes {
            let Some(preset) = doc.preset(stroke.brush) else {
                continue;
            };
            let brush = registry.brush(preset.kind);
            let path = brush.build_path(preset, stroke, registry.caps());
            let paint: Paint = brush.paint(preset, stroke.color);
            canvas.draw_path(&path, &paint);
        }
        for image in &layer.images {
            if let Some(decoded) = load_image(item, image) {
                draw_image(canvas, &decoded, image, layer.opacity.clamp(0.0, 1.0));
            }
        }
    }
}

#[sismo::function("flatten::load_image")]
fn load_image(item: ItemId, image: &ImageBox) -> Option<SkImage> {
    let bytes = bodies::read_asset(item, &image.asset)
        .inspect_err(|err| log::warn!("flatten: image {}: {err}", image.asset))
        .ok()?;
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
            },
        });
        assert_eq!(bounds(&doc), Some((10.0, 20.0, 110.0, 70.0)));
        doc.set_layer_visible(0, false);
        assert_eq!(bounds(&doc), None);
    }
}

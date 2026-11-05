//! Card covers: small PNG previews of a canvas's ink and a PDF's first
//! page, stored on the item so Home shows real content.

use freya_engine::prelude::{
    AlphaType, ColorType, EncodedImageFormat, ImageInfo, SkColor, SkData, SkImage,
    raster_n32_premul,
};

use crate::doc::Doc;
use crate::library::ItemId;

/// Cover size in pixels (card aspect, twice the card's logical size).
pub const WIDTH: i32 = 312;
pub const HEIGHT: i32 = 400;
const MARGIN: f32 = 24.0;

/// PNG of `doc`'s ink and images fitted on `paper`; `None` when it has
/// neither. Images are read from `item`'s files.
#[must_use]
#[sismo::function("thumbnail::canvas")]
pub fn canvas(doc: &Doc, item: ItemId, paper: [u8; 4]) -> Option<Vec<u8>> {
    let (x0, y0, x1, y1) = crate::flatten::bounds(doc)?;
    #[allow(clippy::cast_precision_loss)] // small fixed sizes
    let (w, h) = (WIDTH as f32, HEIGHT as f32);
    let content = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let scale = (MARGIN.mul_add(-2.0, w) / content.0)
        .min(MARGIN.mul_add(-2.0, h) / content.1)
        .clamp(0.05, 2.0);
    let mut surface = raster_n32_premul((WIDTH, HEIGHT))?;
    let sk = surface.canvas();
    sk.clear(SkColor::from_argb(paper[3], paper[0], paper[1], paper[2]));
    sk.save();
    sk.translate((x0.mul_add(-scale, MARGIN), y0.mul_add(-scale, MARGIN)));
    sk.scale((scale, scale));
    sismo::counter!("thumbnail.strokes", doc.stroke_count());
    crate::flatten::paint(sk, doc, item);
    sk.restore();
    let image = {
        sismo::scope!("thumbnail::image_snapshot");
        surface.image_snapshot()
    };
    encode(&image)
}

/// PNG of `pdf`'s first page; `None` when it cannot be rendered.
#[must_use]
#[sismo::function("thumbnail::pdf")]
pub fn pdf(pdf: &freya_pdf::PdfDocument) -> Option<Vec<u8>> {
    let (page_w, page_h) = pdf.page_size(0).ok()?;
    #[allow(clippy::cast_precision_loss)]
    let scale = (WIDTH as f32 / page_w.max(1.0)).min(HEIGHT as f32 / page_h.max(1.0));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // bounded by the cover size
    let (w, h) = (
        (page_w * scale).round() as u32,
        (page_h * scale).round() as u32,
    );
    let (rgba, w, h) = pdf.render_page_rgba(0, w.max(1), h.max(1)).ok()?;
    let info = ImageInfo::new(
        (i32::try_from(w).ok()?, i32::try_from(h).ok()?),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let row = usize::try_from(w).ok()? * 4;
    let image = freya_engine::prelude::raster_from_data(&info, SkData::new_copy(&rgba), row)?;
    encode(&image)
}

#[sismo::function("thumbnail::encode")]
fn encode(image: &SkImage) -> Option<Vec<u8>> {
    image
        .encode(None, EncodedImageFormat::PNG, None)
        .map(|data| data.as_bytes().to_vec())
}

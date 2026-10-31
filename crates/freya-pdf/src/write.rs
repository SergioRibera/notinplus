//! Writing PDFs made of page images (exports with ink flattened on).

use pdfium_render::prelude::{PdfPageObjectsCommon, PdfPagePaperSize, PdfPoints};

use crate::doc::pdfium;
use crate::error::PdfError;

/// One page of an image PDF.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageImage {
    /// RGBA pixels, row major.
    pub rgba: Vec<u8>,
    /// Image width in pixels.
    pub width_px: u32,
    /// Image height in pixels.
    pub height_px: u32,
    /// Page width in points.
    pub width_pt: u32,
    /// Page height in points.
    pub height_pt: u32,
}

/// Build a PDF whose pages are `pages`, each image covering its page.
///
/// # Errors
/// [`PdfError`] when `PDFium` is unavailable or rejects an image.
#[cfg_attr(feature = "sismo", sismo::function("pdf::export_image_pdf"))]
pub fn image_pdf(pages: &[PageImage]) -> Result<Vec<u8>, PdfError> {
    let pdfium = pdfium()?;
    let mut doc = pdfium
        .create_new_pdf()
        .map_err(|e| PdfError::Backend(e.to_string()))?;
    for page in pages {
        #[cfg(feature = "sismo")]
        sismo::scope!("pdf::export_page");
        let Some(buffer) =
            image::RgbaImage::from_raw(page.width_px, page.height_px, page.rgba.clone())
        else {
            return Err(PdfError::Backend(
                "page image size does not match its pixels".to_owned(),
            ));
        };
        #[allow(clippy::cast_precision_loss)] // page sizes in points are small
        let (w, h) = (page.width_pt as f32, page.height_pt as f32);
        let mut target = doc
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::Custom(
                PdfPoints::new(w),
                PdfPoints::new(h),
            ))
            .map_err(|e| PdfError::Backend(e.to_string()))?;
        target
            .objects_mut()
            .create_image_object(
                PdfPoints::new(0.0),
                PdfPoints::new(0.0),
                &image::DynamicImage::ImageRgba8(buffer),
                Some(PdfPoints::new(w)),
                Some(PdfPoints::new(h)),
            )
            .map_err(|e| PdfError::Backend(e.to_string()))?;
    }
    #[cfg(feature = "sismo")]
    sismo::counter!("pdf.export_pages", pages.len());
    #[cfg(feature = "sismo")]
    sismo::scope!("pdf::export_save");
    doc.save_to_bytes()
        .map_err(|e| PdfError::Backend(e.to_string()))
}

/// World-space rectangle `(x, y, width, height)` of every page as the
/// canvas lays them out: centred on x = 0, stacked from y = 0 with
/// `gap` between pages.
#[must_use]
pub fn page_world_rects(page_sizes: &[(f32, f32)], gap: f32) -> Vec<(f32, f32, f32, f32)> {
    let mut y = 0.0;
    page_sizes
        .iter()
        .map(|&(w, h)| {
            let rect = (-(w * 0.5), y, w, h);
            y += h + gap;
            rect
        })
        .collect()
}

/// Gap between pages used by [`crate::PdfBackground::new`].
pub const PAGE_GAP: f32 = 12.0;

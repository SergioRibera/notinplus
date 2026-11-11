//! "Leer en otras apps": a zip other programs open.
//!
//! Each kind exports its documents as a common file (PDFs with the ink
//! drawn on them, canvases as one-page PDFs, notes as markdown), and the
//! highlights, annotations and bookmarks of any document become a
//! markdown list beside it.

use std::io::{Cursor, Write};

use freya_engine::prelude::{
    AlphaType, ColorType, EncodedImageFormat, ImageInfo, SkData, raster_from_data,
    raster_n32_premul,
};
use freya_pdf::write::{PAGE_GAP, PageImage, image_pdf, page_world_rects};
use notin_i18n::tr;
use notin_plugin::UserMessage;
use notin_plugin::create::Paper;

use crate::doc::Doc;
use crate::library::{ItemId, LibraryData, bodies};

/// Pixels per PDF point in flattened pages.
pub(crate) const PAGE_SCALE: f32 = 2.0;
/// Widest or tallest a rendered page may get, in pixels.
const MAX_SIDE_PX: f32 = 8000.0;
/// Margin around a canvas's content in its page, in world units.
const CANVAS_MARGIN: f32 = 36.0;

/// Pixel formats "Exportar como…" writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Png,
    Jpg,
}

impl ImageFormat {
    pub(crate) const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpg => "jpg",
        }
    }

    pub(crate) const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpg => "image/jpeg",
        }
    }
}

/// How a canvas turns into pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CanvasRender {
    /// Paper colour and pattern under the ink; `None` leaves the page
    /// transparent.
    pub paper: Option<([u8; 4], Paper)>,
    /// Pixels per world unit.
    pub scale: f32,
}

/// `page` encoded as `format`. JPEG drops transparency: what was see
/// through turns white.
#[sismo::function("readable::encode_page")]
pub(crate) fn encode_page(page: &PageImage, format: ImageFormat) -> Option<Vec<u8>> {
    let (w, h) = (
        i32::try_from(page.width_px).ok()?,
        i32::try_from(page.height_px).ok()?,
    );
    let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let row = usize::try_from(page.width_px).ok()? * 4;
    let image = match format {
        ImageFormat::Png => raster_from_data(&info, SkData::new_copy(&page.rgba), row)?,
        ImageFormat::Jpg => {
            let mut surface = raster_n32_premul((w, h))?;
            surface.canvas().clear(freya_engine::prelude::Color::WHITE);
            let source = raster_from_data(&info, SkData::new_copy(&page.rgba), row)?;
            surface.canvas().draw_image(&source, (0, 0), None);
            surface.image_snapshot()
        }
    };
    let (kind, quality) = match format {
        ImageFormat::Png => (EncodedImageFormat::PNG, None),
        ImageFormat::Jpg => (EncodedImageFormat::JPEG, Some(90)),
    };
    image
        .encode(None, kind, quality)
        .map(|data| data.as_bytes().to_vec())
}

/// `pages` stacked top to bottom into one image, left aligned on the
/// widest, the gaps white.
#[sismo::function("readable::stack_pages")]
pub(crate) fn stack_pages(pages: &[PageImage]) -> Option<PageImage> {
    let width = pages.iter().map(|p| p.width_px).max()?;
    let height: u32 = pages.iter().map(|p| p.height_px).sum();
    let row = usize::try_from(width).ok()? * 4;
    let mut rgba = vec![255u8; row * usize::try_from(height).ok()?];
    let mut top = 0usize;
    for page in pages {
        let page_row = usize::try_from(page.width_px).ok()? * 4;
        for (y, line) in page.rgba.chunks(page_row).enumerate() {
            let start = (top + y) * row;
            rgba[start..start + line.len()].copy_from_slice(line);
        }
        top += usize::try_from(page.height_px).ok()?;
    }
    Some(PageImage {
        rgba,
        width_px: width,
        height_px: height,
        width_pt: pages.iter().map(|p| p.width_pt).max().unwrap_or(0),
        height_pt: pages.iter().map(|p| p.height_pt).sum(),
    })
}

/// Why a readable export failed.
#[derive(Debug)]
pub enum ReadableError {
    Zip(zip::result::ZipError),
    Io(std::io::Error),
}

impl std::fmt::Display for ReadableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Zip(err) => write!(f, "zip: {err}"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ReadableError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Zip(err) => Some(err),
            Self::Io(err) => Some(err),
        }
    }
}

impl UserMessage for ReadableError {
    fn user_message(&self) -> String {
        match self {
            Self::Zip(zip::result::ZipError::Io(err)) | Self::Io(err) => err.user_message(),
            Self::Zip(_) => tr!("errors-readable-zip"),
        }
    }
}

impl From<zip::result::ZipError> for ReadableError {
    fn from(err: zip::result::ZipError) -> Self {
        Self::Zip(err)
    }
}

impl From<std::io::Error> for ReadableError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

fn file_name(name: &str) -> String {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    if clean.is_empty() {
        tr!("common-untitled")
    } else {
        clean
    }
}

/// Draw `doc`'s ink and images (`item`'s files) inside world rectangle
/// `area` (`x, y, width, height`) onto `rgba` (`w`×`h` pixels), under the
/// pattern of `paper` when given.
#[sismo::function("readable::composite")]
fn composite(
    doc: &Doc,
    item: ItemId,
    area: (f32, f32, f32, f32),
    rgba: &mut [u8],
    w: u32,
    h: u32,
    paper: Option<Paper>,
) {
    let info = ImageInfo::new(
        (i32::try_from(w).unwrap_or(0), i32::try_from(h).unwrap_or(0)),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let Some(mut surface) = raster_n32_premul((info.width(), info.height())) else {
        return;
    };
    let canvas = surface.canvas();
    let Some(page) = freya_engine::prelude::raster_from_data(
        &info,
        freya_engine::prelude::SkData::new_copy(rgba),
        usize::try_from(w).unwrap_or(0) * 4,
    ) else {
        return;
    };
    canvas.draw_image(&page, (0, 0), None);
    #[allow(clippy::cast_precision_loss)] // pixel sizes of a page
    let scale = w as f32 / area.2.max(1.0);
    canvas.save();
    canvas.scale((scale, scale));
    canvas.translate((-area.0, -area.1));
    if let Some(paper) = paper {
        crate::flatten::paint_paper(
            canvas,
            paper,
            (area.0, area.1, area.0 + area.2, area.1 + area.3),
        );
    }
    crate::flatten::paint(canvas, doc, item);
    canvas.restore();
    let _ = surface.read_pixels(&info, rgba, usize::try_from(w).unwrap_or(0) * 4, (0, 0));
}

/// The pages `pages` (zero based) of the PDF `bytes` kept by `item`
/// rendered at `scale` pixels per point, with the ink drawn on them when
/// `annotated`. `None` when the file does not open or a page is missing.
#[sismo::function("readable::pdf_pages")]
pub(crate) fn pdf_pages(
    item: ItemId,
    bytes: &[u8],
    pages: &[usize],
    annotated: bool,
    scale: f32,
) -> Option<Vec<PageImage>> {
    let pdf = freya_pdf::PdfDocument::open_bytes(bytes.to_vec()).ok()?;
    let doc = annotated.then(|| bodies::read_doc(item).ok()).flatten();
    let rects = page_world_rects(pdf.page_sizes(), PAGE_GAP);
    let mut out = Vec::with_capacity(pages.len());
    for &index in pages {
        let rect = rects.get(index)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // page sizes in points
        let (w, h) = (
            (rect.2 * scale).round().min(MAX_SIDE_PX) as u32,
            (rect.3 * scale).round().min(MAX_SIDE_PX) as u32,
        );
        let (mut rgba, w, h) = {
            sismo::scope!("readable::render_page");
            pdf.render_page_rgba(index, w.max(1), h.max(1)).ok()?
        };
        if let Some(doc) = &doc {
            composite(doc, item, *rect, &mut rgba, w, h, None);
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        out.push(PageImage {
            rgba,
            width_px: w,
            height_px: h,
            width_pt: rect.2.round() as u32,
            height_pt: rect.3.round() as u32,
        });
    }
    Some(out)
}

/// The PDF `bytes` kept by `item` with the ink drawn on its pages.
#[sismo::function("readable::flattened_pdf")]
pub(crate) fn flattened_pdf(item: ItemId, bytes: &[u8]) -> Option<Vec<u8>> {
    let count = freya_pdf::PdfDocument::open_bytes(bytes.to_vec())
        .ok()?
        .page_count();
    let all: Vec<usize> = (0..count).collect();
    let pages = pdf_pages(item, bytes, &all, true, PAGE_SCALE)?;
    sismo::scope!("readable::encode_pdf");
    image_pdf(&pages).ok().or_else(|| Some(bytes.to_vec()))
}

/// Canvas `item` as pixels, cropped to its ink and images with a margin.
/// `None` when it holds nothing drawable.
#[sismo::function("readable::canvas_pixels")]
pub(crate) fn canvas_pixels(item: ItemId, render: CanvasRender) -> Option<PageImage> {
    let doc = bodies::read_doc(item).ok()?;
    let (x0, y0, x1, y1) = crate::flatten::bounds(&doc)?;
    let area = (
        x0 - CANVAS_MARGIN,
        y0 - CANVAS_MARGIN,
        CANVAS_MARGIN.mul_add(2.0, x1 - x0),
        CANVAS_MARGIN.mul_add(2.0, y1 - y0),
    );
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (w, h) = (
        (area.2 * render.scale).round().min(MAX_SIDE_PX) as u32,
        (area.3 * render.scale).round().min(MAX_SIDE_PX) as u32,
    );
    let fill = render.paper.map_or([0, 0, 0, 0], |(color, _)| color);
    let mut rgba: Vec<u8> = std::iter::repeat_n(fill, (w * h) as usize)
        .flatten()
        .collect();
    let pattern = render.paper.map(|(_, pattern)| pattern);
    composite(&doc, item, area, &mut rgba, w, h, pattern);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(PageImage {
        rgba,
        width_px: w,
        height_px: h,
        width_pt: area.2.round() as u32,
        height_pt: area.3.round() as u32,
    })
}

/// Canvas `item` as one page of `paper` with its `pattern`, cropped to
/// its ink and images.
#[sismo::function("readable::canvas_pdf")]
pub(crate) fn canvas_pdf(item: ItemId, paper: [u8; 4], pattern: Paper) -> Option<Vec<u8>> {
    let page = canvas_pixels(
        item,
        CanvasRender {
            paper: Some((paper, pattern)),
            scale: PAGE_SCALE,
        },
    )?;
    image_pdf(&[page]).ok()
}

/// The marks of a document as a markdown list.
#[sismo::function("readable::marks_markdown")]
fn marks_markdown(marks: &[notin_plugin::kind::Mark<Vec<u8>>]) -> String {
    let mut out = format!("# {}\n\n", tr!("misc-export-bookmarks-heading"));
    for mark in marks {
        out.push_str("- **");
        out.push_str(&mark.title);
        out.push_str("**");
        if !mark.snippet.is_empty() {
            out.push_str(": ");
            out.push_str(&mark.snippet);
        }
        out.push('\n');
    }
    out
}

/// Build the zip for `items` (paths follow the library's folders).
///
/// # Errors
/// [`ReadableError`] when the archive cannot be written.
#[sismo::function("readable::build")]
pub fn build(
    data: &LibraryData,
    items: &[ItemId],
    mut progress: impl FnMut(usize),
) -> Result<Vec<u8>, ReadableError> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    // Files linked from several items of a folder are written once.
    let mut written = std::collections::HashSet::new();
    for (done, id) in items.iter().enumerate() {
        sismo::scope!("readable::item");
        progress(done);
        let Some(item) = data.item(*id) else {
            continue;
        };
        let folder = crate::flows::forms::folder_path(data, item.folder)
            .split(" › ")
            .skip(1)
            .map(file_name)
            .collect::<Vec<_>>()
            .join("/");
        let base = if folder.is_empty() {
            file_name(&item.name)
        } else {
            format!("{folder}/{}", file_name(&item.name))
        };
        let Some(kind) = crate::plugins::kind(&item.kind.plugin_id) else {
            continue;
        };
        let content = crate::plugins::content::read(item.id, &item.kind.plugin_id)
            .unwrap_or_else(|| kind.empty_content());
        let attachment = kind
            .info()
            .attachment
            .and_then(|_| bodies::read_pdf(item.id).ok());
        let meta = crate::plugins::content::meta(item);
        let mut files = Vec::new();
        let mut linked = Vec::new();
        if let Some(file) = kind.export(meta, &content, attachment.as_deref()) {
            files.push((format!("{base}.{}", file.extension), file.bytes));
            // Beside the file, so its relative links (`assets/x.png`) keep working.
            let assets = if folder.is_empty() {
                bodies::ASSETS_SUBDIR.to_owned()
            } else {
                format!("{folder}/{}", bodies::ASSETS_SUBDIR)
            };
            linked.extend(
                bodies::read_assets(item.id)
                    .into_iter()
                    .map(|(name, bytes)| (format!("{assets}/{name}"), bytes)),
            );
        }
        let marks = kind.marks(&content);
        if !marks.is_empty() {
            files.push((
                tr!("misc-export-bookmarks-file", "name" => base.as_str()),
                marks_markdown(&marks).into_bytes(),
            ));
        }
        for (path, bytes) in files {
            zip.start_file(path, options)?;
            zip.write_all(&bytes)?;
        }
        for (path, bytes) in linked {
            if written.insert(path.clone()) {
                zip.start_file(path, options)?;
                zip.write_all(&bytes)?;
            }
        }
    }
    Ok(zip.finish()?.into_inner())
}

//! "Leer en otras apps": a zip other programs open.
//!
//! Each kind exports its documents as a common file (PDFs with the ink
//! drawn on them, canvases as one-page PDFs, notes as markdown), and the
//! highlights, annotations and bookmarks of any document become a
//! markdown list beside it.

use std::io::{Cursor, Write};

use freya_engine::prelude::{AlphaType, ColorType, ImageInfo, raster_n32_premul};
use freya_pdf::write::{PAGE_GAP, PageImage, image_pdf, page_world_rects};
use notein_i18n::tr;
use notein_plugin::UserMessage;

use crate::doc::Doc;
use crate::library::{ItemId, LibraryData, bodies};
use crate::render::BrushRegistry;

/// Pixels per PDF point in flattened pages.
const PAGE_SCALE: f32 = 2.0;

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

/// Draw `doc`'s ink inside world rectangle `area` onto `rgba` (`w`×`h`
/// pixels).
#[sismo::function("readable::composite")]
fn composite(doc: &Doc, area: (f32, f32, f32, f32), rgba: &mut [u8], w: u32, h: u32) {
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
    let registry = BrushRegistry::default();
    sismo::scope!("readable::draw_strokes");
    for layer in doc.layers.iter().filter(|l| l.visible) {
        for stroke in &layer.strokes {
            let Some(preset) = doc.preset(stroke.brush) else {
                continue;
            };
            let brush = registry.brush(preset.kind);
            canvas.draw_path(
                &brush.build_path(preset, stroke, registry.caps()),
                &brush.paint(preset, stroke.color),
            );
        }
    }
    canvas.restore();
    let _ = surface.read_pixels(&info, rgba, usize::try_from(w).unwrap_or(0) * 4, (0, 0));
}

/// The PDF `bytes` kept by `item` with the ink drawn on its pages.
#[sismo::function("readable::flattened_pdf")]
pub(crate) fn flattened_pdf(item: ItemId, bytes: &[u8]) -> Option<Vec<u8>> {
    let bytes = bytes.to_vec();
    let pdf = freya_pdf::PdfDocument::open_bytes(bytes.clone()).ok()?;
    let doc = bodies::read_doc(item).ok();
    let rects = page_world_rects(pdf.page_sizes(), PAGE_GAP);
    let mut pages = Vec::with_capacity(rects.len());
    for (index, rect) in rects.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // page sizes in points
        let (w, h) = (
            (rect.2 * PAGE_SCALE).round() as u32,
            (rect.3 * PAGE_SCALE).round() as u32,
        );
        let (mut rgba, w, h) = {
            sismo::scope!("readable::render_page");
            pdf.render_page_rgba(index, w.max(1), h.max(1)).ok()?
        };
        if let Some(doc) = &doc {
            composite(doc, *rect, &mut rgba, w, h);
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        pages.push(PageImage {
            rgba,
            width_px: w,
            height_px: h,
            width_pt: rect.2.round() as u32,
            height_pt: rect.3.round() as u32,
        });
    }
    sismo::scope!("readable::encode_pdf");
    image_pdf(&pages).ok().or(Some(bytes))
}

/// Canvas `item` as one page of `paper`, cropped to its ink.
#[sismo::function("readable::canvas_pdf")]
pub(crate) fn canvas_pdf(item: ItemId, paper: [u8; 4]) -> Option<Vec<u8>> {
    let doc = bodies::read_doc(item).ok()?;
    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    for p in doc
        .layers
        .iter()
        .flat_map(|l| l.strokes.iter())
        .flat_map(|s| s.points.iter())
    {
        bounds = Some(bounds.map_or((p.x, p.y, p.x, p.y), |(a, b, c, d)| {
            (a.min(p.x), b.min(p.y), c.max(p.x), d.max(p.y))
        }));
    }
    let (x0, y0, x1, y1) = bounds?;
    let margin = 36.0;
    let area = (
        x0 - margin,
        y0 - margin,
        (x1 - x0) + margin * 2.0,
        (y1 - y0) + margin * 2.0,
    );
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (w, h) = (
        (area.2 * PAGE_SCALE).round().min(8000.0) as u32,
        (area.3 * PAGE_SCALE).round().min(8000.0) as u32,
    );
    let mut rgba: Vec<u8> = std::iter::repeat_n(paper, (w * h) as usize)
        .flatten()
        .collect();
    composite(&doc, area, &mut rgba, w, h);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    image_pdf(&[PageImage {
        rgba,
        width_px: w,
        height_px: h,
        width_pt: area.2.round() as u32,
        height_pt: area.3.round() as u32,
    }])
    .ok()
}

/// The marks of a document as a markdown list.
#[sismo::function("readable::marks_markdown")]
fn marks_markdown(marks: &[notein_plugin::kind::Mark<Vec<u8>>]) -> String {
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

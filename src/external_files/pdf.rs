//! A PDF read page after page, rendered from the file's bytes without a
//! library document behind it.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use freya::prelude::*;
use freya_pdf::{PdfDocument, TocEntry};
use notin_i18n::tr;
use notin_ui_kit::theme::palette;
use notin_ui_kit::{
    Lazy, Notice, NoticeTone, Spinner, TapExt, Text, TextRole, tokens, width_class,
};

use super::Session;
use crate::readable::{ImageFormat, encode_page};

/// Space between pages.
const GAP: f32 = 16.0;
/// Widest a page shows.
const MAX_PAGE_WIDTH: f32 = 900.0;
/// Page bitmaps are rendered at widths in steps of this, so resizing the
/// window does not render every page again at every pixel.
const WIDTH_STEP: u32 = 256;
/// Rendered pages kept for scrolling back.
const CACHED_PAGES: usize = 16;
/// Width of the contents panel.
const CONTENTS_WIDTH: f32 = 260.0;

/// The body of a PDF file.
#[derive(Clone, PartialEq)]
pub(super) struct PdfBody {
    pub session: Session,
}

/// The document once it opens.
#[derive(Clone)]
enum Opened {
    Opening,
    Ready(PdfDocument),
    Failed,
}

impl Component for PdfBody {
    #[sismo::function("external_files::pdf_body")]
    fn render(&self) -> impl IntoElement {
        let mut opened = use_state(|| Opened::Opening);
        let bytes = self.session.file.bytes().to_vec();
        use_hook(move || {
            let (tx, rx) = flume::bounded(1);
            std::thread::spawn(move || {
                sismo::scope!("external_files::pdf_open");
                let _ = tx.send(PdfDocument::open_bytes(bytes));
            });
            spawn(async move {
                opened.set(match rx.recv_async().await {
                    Ok(Ok(doc)) => Opened::Ready(doc),
                    Ok(Err(err)) => {
                        log::error!("open pdf: {err}");
                        Opened::Failed
                    }
                    Err(_) => Opened::Failed,
                });
            });
        });
        match opened.read().clone() {
            Opened::Opening => rect()
                .expanded()
                .center()
                .child(Spinner::default())
                .into_element(),
            Opened::Failed => rect()
                .expanded()
                .padding(tokens::space::LG)
                .child(Notice::new(
                    NoticeTone::Danger,
                    tr!("external-files-pdf-failed"),
                ))
                .into_element(),
            Opened::Ready(doc) => Pages {
                renderer: Renderer::from(doc.clone()),
                contents: Rc::new(doc.table_of_contents()),
                sizes: Rc::new(doc.page_sizes().to_vec()),
            }
            .into_element(),
        }
    }
}

/// A page to render, and where the bitmap goes.
struct Job {
    page: usize,
    width: u32,
    height: u32,
    reply: flume::Sender<Option<Bytes>>,
}

/// Renders pages on a thread of its own and keeps the last few.
#[derive(Clone)]
struct Renderer {
    jobs: flume::Sender<Job>,
    cache: Rc<RefCell<PageCache>>,
}

/// Rendered pages by (page, bitmap width), oldest first.
type PageCache = VecDeque<((usize, u32), Bytes)>;

impl PartialEq for Renderer {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.cache, &other.cache)
    }
}

impl From<PdfDocument> for Renderer {
    fn from(doc: PdfDocument) -> Self {
        let (jobs, rx) = flume::unbounded::<Job>();
        std::thread::spawn(move || {
            for job in rx {
                // The page scrolled away before its turn.
                if job.reply.is_disconnected() {
                    continue;
                }
                sismo::scope!("external_files::render_page");
                let image = doc
                    .render_page_rgba(job.page, job.width, job.height)
                    .ok()
                    .and_then(|(rgba, width_px, height_px)| {
                        encode_page(
                            &freya_pdf::write::PageImage {
                                rgba,
                                width_px,
                                height_px,
                                width_pt: width_px,
                                height_pt: height_px,
                            },
                            ImageFormat::Jpg,
                        )
                    })
                    .map(Bytes::from);
                let _ = job.reply.send(image);
            }
        });
        Self {
            jobs,
            cache: Rc::default(),
        }
    }
}

impl Renderer {
    fn cached(&self, key: (usize, u32)) -> Option<Bytes> {
        self.cache
            .borrow()
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, bytes)| bytes.clone())
    }

    fn keep(&self, key: (usize, u32), bytes: Bytes) {
        let mut cache = self.cache.borrow_mut();
        cache.retain(|(k, _)| *k != key);
        cache.push_back((key, bytes));
        while cache.len() > CACHED_PAGES {
            cache.pop_front();
        }
    }

    #[allow(clippy::future_not_send, reason = "awaited on the UI thread")]
    async fn render(&self, page: usize, width: u32, height: u32) -> Option<Bytes> {
        let (reply, rx) = flume::bounded(1);
        self.jobs
            .send(Job {
                page,
                width,
                height,
                reply,
            })
            .ok()?;
        let bytes = rx.recv_async().await.ok().flatten()?;
        self.keep((page, width), bytes.clone());
        Some(bytes)
    }
}

/// Where each page sits in a column `width` wide: its top and height.
fn layout(sizes: &[(f32, f32)], width: f32) -> Vec<(f32, f32)> {
    let mut top = GAP;
    sizes
        .iter()
        .map(|&(w, h)| {
            let height = width * h / w.max(1.0);
            let placed = (top, height);
            top += height + GAP;
            placed
        })
        .collect()
}

/// The pages of the document, and its contents beside them on wide
/// screens.
#[derive(Clone)]
struct Pages {
    renderer: Renderer,
    contents: Rc<Vec<TocEntry>>,
    sizes: Rc<Vec<(f32, f32)>>,
}

impl PartialEq for Pages {
    fn eq(&self, other: &Self) -> bool {
        self.renderer == other.renderer
    }
}

impl Component for Pages {
    #[sismo::function("external_files::pdf_pages")]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut viewport = use_state(|| 0.0f32);
        let scroll = use_scroll_controller(ScrollConfig::default);
        let wide = !width_class().is_compact();
        let gutter = if wide {
            tokens::space::XL
        } else {
            tokens::space::SM
        };
        let column = 2.0f32
            .mul_add(-gutter, *viewport.read())
            .clamp(120.0, MAX_PAGE_WIDTH);
        let placed = Rc::new(layout(&self.sizes, column));
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "pixel widths"
        )]
        let bitmap = ((column * 2.0) as u32).div_ceil(WIDTH_STEP) * WIDTH_STEP;
        let measured = *viewport.read() > 0.0;
        let pages = placed
            .iter()
            .filter(|_| measured)
            .enumerate()
            .map(|(index, &(_, height))| {
                Lazy::new(rect().key((index, bitmap)).width(Size::fill()).child(Page {
                    renderer: self.renderer.clone(),
                    index,
                    width: column,
                    height,
                    bitmap,
                    ratio: {
                        let (w, h) = self.sizes[index];
                        h / w.max(1.0)
                    },
                }))
                .height_hint(height + GAP)
                .recycle(true)
                .key(index)
                .into_element()
            });
        let reader = rect()
            .width(Size::flex(1.0))
            .height(Size::fill())
            .child(
                ScrollView::new_controlled(scroll)
                    .width(Size::fill())
                    .height(Size::fill())
                    .on_sized(move |e: Event<SizedEventData>| {
                        viewport.set_if_modified(e.area.width());
                    })
                    .child(notin_ui_kit::ScrollContent::new(
                        rect()
                            .width(Size::fill())
                            .padding((0.0, 0.0, GAP, 0.0))
                            .cross_align(Alignment::Center)
                            .children(pages),
                    )),
            )
            .child(PageIndicator {
                scroll,
                placed: Rc::clone(&placed),
            });
        rect()
            .expanded()
            .background(p.s1)
            .horizontal()
            .content(Content::Flex)
            .maybe_child((wide && !self.contents.is_empty()).then(|| {
                Contents {
                    entries: Rc::clone(&self.contents),
                    placed: Rc::clone(&placed),
                    scroll,
                }
                .into_element()
            }))
            .child(reader)
    }
}

/// One page: white paper until its bitmap arrives.
#[derive(Clone, PartialEq)]
struct Page {
    renderer: Renderer,
    index: usize,
    width: f32,
    height: f32,
    /// Width of the bitmap to render.
    bitmap: u32,
    /// Height over width of the page.
    ratio: f32,
}

impl Component for Page {
    #[sismo::function("external_files::pdf_page")]
    fn render(&self) -> impl IntoElement {
        let key = (self.index, self.bitmap);
        let mut image = use_state(|| self.renderer.cached(key));
        use_hook(|| {
            if image.peek().is_some() {
                return;
            }
            let renderer = self.renderer.clone();
            let (index, width) = key;
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss,
                reason = "pixel heights"
            )]
            let height = (width as f32 * self.ratio).round() as u32;
            spawn(async move {
                if let Some(bytes) = renderer.render(index, width, height.max(1)).await {
                    image.set(Some(bytes));
                }
            });
        });
        rect()
            .width(Size::fill())
            .cross_align(Alignment::Center)
            .padding((GAP, 0.0, 0.0, 0.0))
            .child(
                rect()
                    .width(Size::px(self.width))
                    .height(Size::px(self.height))
                    .background(Color::WHITE)
                    .corner_radius(tokens::radius::XS)
                    .overflow(Overflow::Clip)
                    .maybe_child(image.read().clone().map(|bytes| {
                        ImageViewer::new((key, bytes))
                            .width(Size::fill())
                            .height(Size::fill())
                    })),
            )
    }
}

/// "p. N de M" over the pages, for the page at the top of the view.
#[derive(Clone)]
struct PageIndicator {
    scroll: ScrollController,
    placed: Rc<Vec<(f32, f32)>>,
}

impl PartialEq for PageIndicator {
    fn eq(&self, other: &Self) -> bool {
        self.scroll == other.scroll && self.placed == other.placed
    }
}

impl Component for PageIndicator {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let top = -self.scroll.position().y + 80.0;
        let page = self
            .placed
            .iter()
            .rposition(|&(start, _)| start <= top)
            .unwrap_or(0);
        rect()
            .position(Position::new_absolute().bottom(tokens::space::LG))
            .width(Size::fill())
            .layer(Layer::Relative(12))
            .center()
            .child(
                rect()
                    .padding((5.0, 12.0))
                    .corner_radius(tokens::radius::FULL)
                    .background(p.inv.with_a(220))
                    .child(
                        Text::new(tr!(
                            "external-files-page",
                            "page" => page + 1,
                            "total" => self.placed.len()
                        ))
                        .role(TextRole::Meta)
                        .color(p.inv_text),
                    ),
            )
    }
}

/// The PDF's own table of contents; an entry scrolls to its page.
#[derive(Clone)]
struct Contents {
    entries: Rc<Vec<TocEntry>>,
    placed: Rc<Vec<(f32, f32)>>,
    scroll: ScrollController,
}

impl PartialEq for Contents {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.entries, &other.entries)
            && self.placed == other.placed
            && self.scroll == other.scroll
    }
}

/// Entries flattened in reading order with their depth.
fn flatten(entries: &[TocEntry], depth: usize, out: &mut Vec<(usize, TocEntry)>) {
    for entry in entries {
        out.push((depth, entry.clone()));
        flatten(&entry.children, depth + 1, out);
    }
}

impl Component for Contents {
    #[sismo::function("external_files::pdf_contents")]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut rows = Vec::new();
        flatten(&self.entries, 0, &mut rows);
        let rows = rows.into_iter().enumerate().map(|(i, (depth, entry))| {
            let top = entry
                .page
                .and_then(|page| self.placed.get(page))
                .map(|&(top, _)| top);
            let mut scroll = self.scroll;
            #[allow(clippy::cast_precision_loss, reason = "a few levels")]
            let indent = depth as f32 * 12.0;
            rect()
                .key(i)
                .width(Size::fill())
                .padding((6.0, 8.0, 6.0, 8.0 + indent))
                .corner_radius(tokens::radius::XS)
                .on_tap(move |_| {
                    if let Some(top) = top {
                        #[allow(clippy::cast_possible_truncation, reason = "pixel offsets")]
                        scroll.scroll_to_y(-((top - GAP).max(0.0) as i32));
                    }
                })
                .child(
                    Text::new(entry.title)
                        .role(if depth == 0 {
                            TextRole::Strong
                        } else {
                            TextRole::Body
                        })
                        .lines(2),
                )
                .into_element()
        });
        rect()
            .width(Size::px(CONTENTS_WIDTH))
            .height(Size::fill())
            .content(Content::Flex)
            .background(p.bg)
            .border(
                Border::new()
                    .fill(p.line)
                    .width(BorderWidth {
                        right: 1.0,
                        ..BorderWidth::default()
                    })
                    .alignment(BorderAlignment::Inner),
            )
            .padding((tokens::space::MD, tokens::space::SM))
            .spacing(tokens::space::XS)
            .child(
                rect()
                    .padding((0.0, 8.0, 6.0, 8.0))
                    .child(Text::new(tr!("external-files-contents")).role(TextRole::Label)),
            )
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::flex(1.0))
                    .child(notin_ui_kit::ScrollContent::new(
                        rect().width(Size::fill()).children(rows),
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{GAP, layout};

    #[test]
    fn pages_stack_with_gaps() {
        let placed = layout(&[(100.0, 200.0), (200.0, 100.0)], 400.0);
        assert_eq!(placed, vec![(GAP, 800.0), (GAP + 800.0 + GAP, 200.0)]);
    }
}

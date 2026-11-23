//! A file dropped on an open document that takes one: a body written in
//! the host's markdown editor (a note, or any plugin page built on it) or
//! a canvas, which gets text boxes and pictures in the middle of the view.
//!
//! One sheet asks how it joins the document, in the words `/ref`
//! already uses:
//!
//! - **Reference**: the file is imported next to the document and a
//!   chip to it goes into the text.
//! - **Link**: imported and linked to the document; the text stays.
//! - **Insert the text**: pages of a PDF (or the whole Markdown / text
//!   file) go into the text.
//! - **Insert as image**: pages of a PDF, as pictures kept with the
//!   document.
//!
//! In a note the insert goes through its editor, at the caret or after
//! the last block, as one undo step. Images are inserted without asking.

use std::cell::Cell;
use std::path::PathBuf;

use freya::prelude::*;
use notin_i18n::tr;
use notin_plugin::SourceRef;
use notin_ui_kit::{
    Button, Group, ListRow, ModalController, Section, Sheet, SheetFooter, Text, TextField,
    TextRole, Toast, ToastTone, Toasts, Trailing,
};

use super::create::import_entry;
use crate::editor::images::StoredImage;
use crate::files::Picked;
use crate::library::ItemId;
use crate::library::LinkKind;

/// Most PDF pages one drop inserts as text.
const MAX_TEXT_PAGES: usize = 50;
/// Most PDF pages one drop inserts as images.
const MAX_IMAGE_PAGES: usize = 20;
/// Pixels per point of a page inserted as an image.
const PAGE_SCALE: f32 = 2.0;

/// How the file joins the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Reference,
    Link,
    Text,
    Image,
}

thread_local! {
    /// The choice made last, offered first next time.
    static LAST: Cell<Choice> = const { Cell::new(Choice::Reference) };
}

/// What the dropped file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileKind {
    Pdf {
        pages: usize,
    },
    /// Markdown or plain text, inserted whole.
    Text,
    Image,
}

impl FileKind {
    const fn choices(self) -> &'static [Choice] {
        match self {
            Self::Pdf { .. } => &[Choice::Reference, Choice::Link, Choice::Text, Choice::Image],
            Self::Text => &[Choice::Reference, Choice::Link, Choice::Text],
            Self::Image => &[],
        }
    }
}

fn extension(name: &str) -> String {
    std::path::Path::new(name)
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Bring the file at `path` into `document`. Returns once it is in (or
/// the person said no).
#[sismo::function("flows::add_to_document")]
pub(super) async fn run(path: PathBuf, document: ItemId, position: usize, total: usize) {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bytes = match crate::opened_files::read(path).await {
        Ok(bytes) => bytes,
        Err(err) => {
            crate::opened_files::failed(&name, &notin_plugin::UserMessage::user_message(&err));
            return;
        }
    };
    let picked = Picked { name, bytes };
    let Some(kind) = classify(&picked).await else {
        refused(&picked.name);
        return;
    };
    let target = document_name(document).unwrap_or_default();
    if kind == FileKind::Image {
        insert_image(document, &picked).await;
        return;
    }
    let Some(asked) = ask(&picked.name, &target, kind, position, total).await else {
        return;
    };
    LAST.with(|last| last.set(asked.choice));
    let inserted = match asked.choice {
        Choice::Reference => reference(document, &picked).await,
        Choice::Link => link(document, &picked).await,
        Choice::Text => text(document, &picked, kind, &asked).await,
        Choice::Image => pages_as_images(document, &picked, kind, &asked).await,
    };
    match inserted {
        Ok(message) => {
            Toasts::get().show(Toast::new(message));
        }
        Err(why) => {
            Toasts::get().show(
                Toast::new(tr!(
                    "errors-headline-detail",
                    "headline" => tr!("flows-add-failed", "name" => picked.name.clone()),
                    "detail" => why
                ))
                .tone(ToastTone::Error),
            );
        }
    }
}

/// `None` for files no choice applies to.
async fn classify(picked: &Picked) -> Option<FileKind> {
    match extension(&picked.name).as_str() {
        "pdf" if picked.bytes.starts_with(b"%PDF-") => {
            let bytes = picked.bytes.clone();
            let pages = off_thread(move || {
                freya_pdf::PdfDocument::open_bytes(bytes)
                    .map(|pdf| pdf.page_count())
                    .ok()
            })
            .await
            .flatten()?;
            Some(FileKind::Pdf { pages })
        }
        "md" | "markdown" | "txt" => std::str::from_utf8(&picked.bytes)
            .is_ok()
            .then_some(FileKind::Text),
        "png" | "jpg" | "jpeg" | "webp" | "gif" => Some(FileKind::Image),
        _ => None,
    }
}

fn refused(name: &str) {
    Toasts::get().show(
        Toast::new(tr!("flows-import-queue-refused", "count" => 1, "names" => name.to_owned()))
            .tone(ToastTone::Error),
    );
}

fn document_name(document: ItemId) -> Option<String> {
    crate::state::LibraryView::try_get()?
        .data()?
        .item(document)
        .map(|item| item.name.clone())
}

/// Run `work` on its own thread; `None` when it could not start.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = flume::bounded(1);
    std::thread::Builder::new()
        .name("add-to-document".into())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .ok()?;
    rx.recv_async().await.ok()
}

/// What the person chose in the sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Asked {
    choice: Choice,
    /// Zero-based pages, for text and images of a PDF.
    pages: Vec<usize>,
    /// Keep the PDF in the library and point to it.
    keep_file: bool,
}

/// The sheet; `None` when it closed without inserting.
async fn ask(
    file: &str,
    document: &str,
    kind: FileKind,
    position: usize,
    total: usize,
) -> Option<Asked> {
    let choices = kind.choices();
    let first = LAST.with(Cell::get);
    let choice = State::create_in_scope(
        if choices.contains(&first) {
            first
        } else {
            Choice::Reference
        },
        ScopeId::ROOT,
    );
    let pages = State::create_in_scope("1".to_owned(), ScopeId::ROOT);
    let keep_file = State::create_in_scope(true, ScopeId::ROOT);
    let (tx, rx) = flume::bounded(1);
    let cancelled = tx.clone();
    let title = if total > 1 {
        tr!("flows-add-title-queued", "name" => file.to_owned(), "position" => position, "total" => total)
    } else {
        tr!("flows-add-title", "name" => file.to_owned())
    };
    let count = match kind {
        FileKind::Pdf { pages } => pages,
        FileKind::Text | FileKind::Image => 0,
    };
    Sheet::titled(title)
        .subtitle(tr!("flows-add-subtitle", "document" => document.to_owned()))
        .on_close(move |()| {
            let _ = cancelled.try_send(None);
        })
        .child(Choices {
            offered: choices,
            choice,
            pages,
            keep_file,
            count,
        })
        .footer(SheetFooter::new([Button::new(tr!("flows-add-submit"))
            .primary()
            .on_press(move |_| {
                let asked = Asked {
                    choice: *choice.peek(),
                    pages: parse_pages(&pages.peek(), count),
                    keep_file: *keep_file.peek(),
                };
                let mut modal = ModalController::get();
                modal.clear_top_on_close();
                modal.close();
                let _ = tx.try_send(Some(asked));
            })
            .into_element()]))
        .push();
    rx.recv_async().await.ok().flatten()
}

/// The options of the sheet, and the pages when they apply.
#[derive(Clone, PartialEq)]
struct Choices {
    offered: &'static [Choice],
    choice: State<Choice>,
    pages: State<String>,
    keep_file: State<bool>,
    /// Pages of the PDF; 0 for a text file.
    count: usize,
}

impl Component for Choices {
    #[sismo::function("flows::add_to_document_choices")]
    fn render(&self) -> impl IntoElement {
        let mut choice = self.choice;
        let chosen = *choice.read();
        let rows: Vec<Element> = self
            .offered
            .iter()
            .map(|&option| {
                let (title, detail) = labels(option, self.count > 0);
                let mut row = ListRow::new(title)
                    .subtitle(detail)
                    .on_press(move |_| choice.set(option));
                if option == chosen {
                    row = row.trailing(Trailing::Element(
                        SvgViewer::new(freya::icons::lucide::check())
                            .color(notin_ui_kit::palette().accent)
                            .width(Size::px(18.0))
                            .height(Size::px(18.0))
                            .into_element(),
                    ));
                }
                row.into_element()
            })
            .collect();
        let pages_apply = self.count > 0 && matches!(chosen, Choice::Text | Choice::Image);
        let mut keep_file = self.keep_file;
        rect()
            .width(Size::fill())
            .spacing(14.0)
            .child(Group::default().children(rows))
            .maybe_child(pages_apply.then(|| {
                Section::new(tr!("flows-add-pages")).child(
                    rect()
                        .width(Size::fill())
                        .spacing(8.0)
                        .child(
                            TextField::new(self.pages)
                                .label(tr!("flows-add-pages-field", "count" => self.count))
                                .placeholder(tr!("flows-add-pages-placeholder")),
                        )
                        .child(Text::new(tr!("flows-add-pages-hint")).role(TextRole::Hint)),
                )
            }))
            .maybe_child(matches!(chosen, Choice::Text | Choice::Image).then(|| {
                Group::default().children([ListRow::new(tr!("flows-add-keep-file"))
                    .subtitle(if *keep_file.read() {
                        tr!("flows-add-keep-file-on")
                    } else {
                        tr!("flows-add-keep-file-off")
                    })
                    .trailing(Trailing::Switch {
                        on: *keep_file.read(),
                        on_toggle: EventHandler::new(move |on| keep_file.set(on)),
                    })
                    .into_element()])
            }))
    }
}

fn labels(choice: Choice, pdf: bool) -> (String, String) {
    match choice {
        Choice::Reference => (
            tr!("flows-add-reference"),
            tr!("flows-add-reference-detail"),
        ),
        Choice::Link => (tr!("flows-add-link"), tr!("flows-add-link-detail")),
        Choice::Text if pdf => (tr!("flows-add-text"), tr!("flows-add-text-pages-detail")),
        Choice::Text => (tr!("flows-add-text"), tr!("flows-add-text-detail")),
        Choice::Image => (tr!("flows-add-image"), tr!("flows-add-image-detail")),
    }
}

/// "1-3, 7" as zero-based page indices below `count`, in order, without
/// repeats. Empty or unreadable text means the first page.
fn parse_pages(text: &str, count: usize) -> Vec<usize> {
    let mut pages = Vec::new();
    for part in text.split([',', ';', ' ']).filter(|part| !part.is_empty()) {
        let (from, to) = match part.split_once('-') {
            Some((from, to)) => (from.trim().parse::<usize>(), to.trim().parse::<usize>()),
            None => (part.trim().parse::<usize>(), part.trim().parse::<usize>()),
        };
        let (Ok(from), Ok(to)) = (from, to) else {
            continue;
        };
        for page in from.min(to)..=from.max(to) {
            if (1..=count).contains(&page) && !pages.contains(&(page - 1)) {
                pages.push(page - 1);
            }
        }
    }
    if pages.is_empty() && count > 0 {
        pages.push(0);
    }
    pages
}

/// Where dropped content goes in an open document.
#[derive(Clone)]
pub(crate) enum Destination {
    /// A body written in the host's markdown editor (a note): at the
    /// caret, or after the last block.
    Editor(freya_markdown_editor::prelude::EditorController),
    /// A canvas on screen: text boxes and pictures at the middle of the
    /// view.
    Board(std::sync::Arc<std::sync::Mutex<crate::canvas::Board>>),
}

impl Destination {
    /// Where `document` takes dropped content, when it is on screen and
    /// takes any.
    #[must_use]
    pub(crate) fn of(document: ItemId) -> Option<Self> {
        crate::editor::body_editor(document)
            .map(Self::Editor)
            .or_else(|| crate::active_boards::get_item(document).map(Self::Board))
    }

    /// [`Self::of`], or why there is none.
    fn open(document: ItemId) -> Result<Self, String> {
        Self::of(document).ok_or_else(|| tr!("flows-add-closed"))
    }

    fn insert_markdown(&self, markdown: &str) -> Result<(), String> {
        let inserted = match self {
            Self::Editor(editor) => {
                let mounted = editor.is_mounted();
                if mounted {
                    editor.insert_markdown_anywhere(markdown);
                }
                mounted
            }
            Self::Board(board) => crate::canvas::lock(board).insert_text_at_view_center(markdown),
        };
        inserted
            .then_some(())
            .ok_or_else(|| tr!("flows-add-closed"))
    }

    /// Pictures stored with the document, with what they came from
    /// (`source`, a `notein://` link) when it is kept.
    fn insert_images(
        &self,
        images: &[StoredImage],
        label: &str,
        source: Option<&str>,
    ) -> Result<(), String> {
        match self {
            Self::Editor(_) => {
                let label = label.replace(['[', ']'], "");
                let mut markdown: Vec<String> = images
                    .iter()
                    .map(|image| format!("![{label}]({})", image.url))
                    .collect();
                markdown.extend(source.map(|url| link_markdown(&label, url)));
                self.insert_markdown(&markdown.join("\n\n"))
            }
            Self::Board(board) => {
                let assets: Vec<(String, (u32, u32))> = images
                    .iter()
                    .map(|image| (image.name.clone(), image.size))
                    .collect();
                crate::canvas::lock(board)
                    .add_snapshots_at_view_center(&assets, source)
                    .then_some(())
                    .ok_or_else(|| tr!("flows-add-closed"))
            }
        }
    }
}

/// Import the file next to `document` and return its reference URL.
async fn import_beside(document: ItemId, picked: &Picked) -> Result<(ItemId, String), String> {
    let entry = import_entry(&picked.name).ok_or_else(|| tr!("opened-files-unsupported"))?;
    let folder = crate::state::LibraryView::try_get()
        .and_then(|view| view.data())
        .and_then(|data| data.item(document).map(|item| item.folder))
        // Shared documents live in a folder nothing can be added to.
        .filter(|folder| *folder != crate::library::SHARED_WITH_ME)
        .unwrap_or(crate::library::ROOT_FOLDER);
    let item = super::create::import_quietly(&entry, folder, picked).await?;
    let url = SourceRef::Plugin {
        plugin_id: entry.kind.clone(),
        item: item.0,
        anchor: Vec::new(),
        selection: None,
    }
    .to_url();
    Ok((item, url))
}

fn link_markdown(label: &str, url: &str) -> String {
    // Brackets would end the label early.
    let label = label.replace(['[', ']'], "");
    format!("[{label}]({url})")
}

async fn reference(document: ItemId, picked: &Picked) -> Result<String, String> {
    let (_, url) = import_beside(document, picked).await?;
    let label = file_label(&picked.name);
    Destination::open(document)?.insert_markdown(&link_markdown(&label, &url))?;
    Ok(tr!("flows-add-done", "name" => label))
}

async fn link(document: ItemId, picked: &Picked) -> Result<String, String> {
    let (item, _) = import_beside(document, picked).await?;
    let handle = crate::library::shared()
        .await
        .map_err(|err| notin_plugin::UserMessage::user_message(&err))?;
    handle
        .lock()
        .await
        .create_link(document, item, LinkKind::Related)
        .await
        .map_err(|err| notin_plugin::UserMessage::user_message(&err))?;
    Ok(tr!("flows-add-linked", "name" => file_label(&picked.name)))
}

/// The file name without its extension, as a document gets named.
fn file_label(name: &str) -> String {
    std::path::Path::new(name).file_stem().map_or_else(
        || name.to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

/// A line pointing to the kept PDF, under what came from it.
async fn source_line(document: ItemId, picked: &Picked, asked: &Asked) -> Result<String, String> {
    if !asked.keep_file {
        return Ok(String::new());
    }
    let (_, url) = import_beside(document, picked).await?;
    Ok(format!(
        "\n\n{}",
        link_markdown(&file_label(&picked.name), &url)
    ))
}

async fn text(
    document: ItemId,
    picked: &Picked,
    kind: FileKind,
    asked: &Asked,
) -> Result<String, String> {
    let body = match kind {
        FileKind::Pdf { .. } => {
            let bytes = picked.bytes.clone();
            let pages: Vec<usize> = asked.pages.iter().copied().take(MAX_TEXT_PAGES).collect();
            let texts = off_thread(move || {
                let pdf = freya_pdf::PdfDocument::open_bytes(bytes).ok()?;
                Some(
                    pages
                        .iter()
                        .filter_map(|&page| pdf.extract_page_text(page).ok())
                        .map(|text| text.trim().to_owned())
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>(),
                )
            })
            .await
            .flatten()
            .ok_or_else(|| tr!("flows-add-unreadable"))?;
            if texts.is_empty() {
                return Err(tr!("flows-add-no-text"));
            }
            texts.join("\n\n")
        }
        FileKind::Text if extension(&picked.name) == "txt" => {
            // Plain text: every line break is meant.
            String::from_utf8_lossy(&picked.bytes).replace('\n', "  \n")
        }
        FileKind::Text | FileKind::Image => String::from_utf8_lossy(&picked.bytes).into_owned(),
    };
    let source = source_line(document, picked, asked).await?;
    Destination::open(document)?.insert_markdown(&format!("{body}{source}"))?;
    Ok(tr!("flows-add-done", "name" => file_label(&picked.name)))
}

async fn pages_as_images(
    document: ItemId,
    picked: &Picked,
    kind: FileKind,
    asked: &Asked,
) -> Result<String, String> {
    if !matches!(kind, FileKind::Pdf { .. }) {
        return Err(tr!("flows-add-unreadable"));
    }
    let bytes = picked.bytes.clone();
    let pages: Vec<usize> = asked.pages.iter().copied().take(MAX_IMAGE_PAGES).collect();
    let encoded = off_thread(move || {
        // Not annotated: the file is not in the library, it has no ink.
        let rendered =
            crate::readable::pdf_pages(ItemId([0; 16]), &bytes, &pages, false, PAGE_SCALE)?;
        rendered
            .iter()
            .map(|page| crate::readable::encode_page(page, crate::readable::ImageFormat::Png))
            .collect::<Option<Vec<_>>>()
    })
    .await
    .flatten()
    .ok_or_else(|| tr!("flows-add-unreadable"))?;
    let images = crate::editor::images::ItemImages::new(document);
    let label = file_label(&picked.name);
    let mut stored = Vec::with_capacity(encoded.len());
    for png in encoded {
        stored.push(
            images
                .store(png)
                .await
                .map_err(|err| notin_plugin::UserMessage::user_message(&err))?,
        );
    }
    let source = if asked.keep_file {
        Some(import_beside(document, picked).await?.1)
    } else {
        None
    };
    Destination::open(document)?.insert_images(&stored, &label, source.as_deref())?;
    Ok(tr!("flows-add-done", "name" => label))
}

async fn insert_image(document: ItemId, picked: &Picked) {
    let images = crate::editor::images::ItemImages::new(document);
    let result = match images.store(picked.bytes.clone()).await {
        Ok(stored) => Destination::open(document).and_then(|destination| {
            destination.insert_images(&[stored], &file_label(&picked.name), None)
        }),
        Err(err) => Err(notin_plugin::UserMessage::user_message(&err)),
    };
    match result {
        Ok(()) => {
            Toasts::get().show(Toast::new(
                tr!("flows-add-done", "name" => file_label(&picked.name)),
            ));
        }
        Err(why) => {
            Toasts::get().show(
                Toast::new(tr!(
                    "errors-headline-detail",
                    "headline" => tr!("flows-add-failed", "name" => picked.name.clone()),
                    "detail" => why
                ))
                .tone(ToastTone::Error),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_ranges() {
        assert_eq!(parse_pages("1-3, 7", 10), [0, 1, 2, 6]);
        assert_eq!(
            parse_pages("3-1", 10),
            [0, 1, 2],
            "a range written backwards"
        );
        assert_eq!(
            parse_pages("2, 2, 40", 10),
            [1],
            "no repeats, none past the end"
        );
        assert_eq!(parse_pages("", 5), [0], "first page by default");
        assert_eq!(parse_pages("x", 0), Vec::<usize>::new());
    }

    #[test]
    fn labels_lose_brackets() {
        assert_eq!(link_markdown("a [b]", "u"), "[a b](u)");
    }
}

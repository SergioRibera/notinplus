//! Landing page rendered by [`crate::route::Route::Home`].
//!
//! Wires two entry points side by side:
//!
//! * The original **picker-based** buttons that open a blank canvas
//!   or import a PDF (still standalone — they do not touch the
//!   library).
//! * A minimal **library** panel used during bring-up to exercise the
//!   [`crate::library::Library`] plumbing end to end. The panel lists
//!   root-level folders, items, and tags and exposes buttons for the
//!   basic mutations. It is intentionally rough — real UI lands later.
//!
//! State surfaced back to the UI:
//!
//! * `status` — coarse label for the picker pipeline.
//! * `lib_state` — one of Loading / Ready / Error, populated by an
//!   on-mount async task that opens the library.
//! * `snap` — cloned [`LibraryIndex`] snapshot, refreshed after every
//!   mutation so the reactive tree re-renders without re-locking the
//!   library on every read.
//!
//! Every mutation runs inside the freya async executor via `spawn`
//! since event handlers are synchronous.

use std::io::Read;
use std::sync::Arc;

use async_lock::Mutex as AsyncMutex;
use freya::prelude::*;
use freya::router::*;
use freya_pdf::{PdfBackground, PdfDocument};
use istmo::{IstmoError, Runtime};
use istmo_file_picker::{
    FileFilter, FilePickerClient, FilePickerError, OwnedPickedFile, PickConfig, PickedFileReader,
};

use crate::canvas::{Board, lock};
use crate::library::{FolderId, ItemKind, Library, LibraryIndex, ROOT_FOLDER, Rgba, Tag};
use crate::route::Route;

/// Shared library handle. Wrapped in an async-aware mutex so any
/// number of `spawn`ed handlers can await it without holding a
/// blocking lock across `.await`.
type LibHandle = Arc<AsyncMutex<Library>>;

/// UI status broadcast by the picker pipeline. Kept intentionally
/// coarse — the label reads well on a single line and matches the
/// error variants users can actually act on.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PickerStatus {
    Idle,
    Picking,
    Reading,
    Error(String),
}

impl PickerStatus {
    fn text(&self) -> Option<String> {
        match self {
            Self::Idle => None,
            Self::Picking => Some("waiting for file picker…".into()),
            Self::Reading => Some("reading PDF…".into()),
            Self::Error(msg) => Some(msg.clone()),
        }
    }

    fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }
}

/// Library bring-up state. Split from the loaded [`LibHandle`] so
/// error / loading branches do not need to hold a dummy Arc.
#[derive(Clone)]
enum LibState {
    Loading,
    Ready(LibHandle),
    Error(String),
}

/// Landing page.
#[derive(Debug, PartialEq)]
pub struct Home;

impl Component for Home {
    fn render(&self) -> impl IntoElement {
        let status = use_state(|| PickerStatus::Idle);
        let status_display = status.read().clone();

        let lib_state = use_state(|| LibState::Loading);
        let snap = use_state(|| Option::<LibraryIndex>::None);

        // Kick the library open exactly once. `use_hook` fires per
        // component instance; the returned unit is discarded.
        use_hook({
            let mut lib_state = lib_state;
            let mut snap = snap;
            move || {
                spawn(async move {
                    match open_library().await {
                        Ok(handle) => {
                            let index = handle.lock().await.index().clone();
                            snap.set(Some(index));
                            lib_state.set(LibState::Ready(handle));
                        }
                        Err(msg) => {
                            log::error!("library open failed: {msg}");
                            lib_state.set(LibState::Error(msg));
                        }
                    }
                });
            }
        });

        let lib_snapshot = snap.read().clone();
        let mut body = rect()
            .vertical()
            .spacing(10.0)
            .padding(14.0)
            .background(Color::from_rgb(34, 34, 42))
            .with_corner_radius(12.0)
            .width(Size::px(420.0))
            .child(
                label()
                    .color(Color::WHITE)
                    .font_size(16.0)
                    .text("Library (bring-up)"),
            );

        match (&lib_state.read().clone(), lib_snapshot) {
            (LibState::Loading, _) => {
                body = body.child(muted_label("loading library…"));
            }
            (LibState::Error(msg), _) => {
                body = body.child(error_label(msg));
            }
            (LibState::Ready(handle), Some(index)) => {
                body = body
                    .child(action_row(handle.clone(), snap))
                    .child(divider())
                    .child(tags_row(&index, handle.clone(), snap))
                    .child(divider())
                    .child(folders_list(&index, handle.clone(), snap))
                    .child(divider())
                    .child(items_list(&index, handle.clone(), snap));
            }
            (LibState::Ready(_), None) => {
                body = body.child(muted_label("library ready, snapshot pending…"));
            }
        }

        rect()
            .width(Size::fill())
            .height(Size::fill())
            .background(Color::from_rgb(24, 24, 30))
            .center()
            .child(body)
    }
}

fn action_row(handle: LibHandle, snap: State<Option<LibraryIndex>>) -> impl IntoElement {
    rect()
        .horizontal()
        .spacing(8.0)
        .child(mini_button("+ Folder", Color::from_rgb(80, 140, 220), {
            let handle = handle.clone();
            move |_| {
                let handle = handle.clone();
                let mut snap = snap;
                spawn(async move {
                    let mut lib = handle.lock().await;
                    if let Err(err) = lib.create_folder(ROOT_FOLDER, "New folder").await {
                        log::error!("create_folder: {err}");
                        return;
                    }
                    refresh_snapshot(&lib, &mut snap);
                });
            }
        }))
        .child(mini_button("+ Canvas", Color::from_rgb(90, 180, 130), {
            let handle = handle.clone();
            move |_| {
                let handle = handle.clone();
                let mut snap = snap;
                spawn(async move {
                    let mut lib = handle.lock().await;
                    if let Err(err) = lib
                        .create_item(ROOT_FOLDER, ItemKind::Canvas, "Untitled canvas")
                        .await
                    {
                        log::error!("create_item: {err}");
                        return;
                    }
                    refresh_snapshot(&lib, &mut snap);
                });
            }
        }))
        .child(mini_button("+ Tag", Color::from_rgb(230, 170, 90), {
            let handle = handle.clone();
            move |_| {
                let handle = handle.clone();
                let mut snap = snap;
                let color = pick_tag_color();
                spawn(async move {
                    let mut lib = handle.lock().await;
                    if let Err(err) = lib.create_tag("tag", color).await {
                        log::error!("create_tag: {err}");
                        return;
                    }
                    refresh_snapshot(&lib, &mut snap);
                });
            }
        }))
        .child(mini_button("Reload", Color::from_rgb(120, 120, 140), {
            let handle = handle;
            move |_| {
                spawn_snapshot(handle.clone(), snap);
            }
        }))
}

fn tags_row(
    index: &LibraryIndex,
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
) -> impl IntoElement {
    let mut row = rect().vertical().spacing(4.0).child(section_label("Tags"));
    if index.tags.is_empty() {
        row = row.child(muted_label("no tags yet"));
        return row;
    }
    let mut chips = rect().horizontal().spacing(6.0);
    for tag in &index.tags {
        chips = chips.child(tag_chip(tag.clone(), handle.clone(), snap));
    }
    row.child(chips)
}

fn tag_chip(tag: Tag, handle: LibHandle, snap: State<Option<LibraryIndex>>) -> impl IntoElement {
    let tag_id = tag.id;
    let bg = rgba_to_color(tag.color);
    rect()
        .horizontal()
        .spacing(6.0)
        .padding((4.0, 8.0))
        .background(bg)
        .with_corner_radius(8.0)
        .child(label().color(Color::WHITE).font_size(12.0).text(tag.name))
        .child(
            rect()
                .padding((2.0, 6.0))
                .background(Color::from_argb(90, 0, 0, 0))
                .with_corner_radius(6.0)
                .child(label().color(Color::WHITE).font_size(11.0).text("×"))
                .on_press(move |_| {
                    let handle = handle.clone();
                    let mut snap = snap;
                    spawn(async move {
                        let mut lib = handle.lock().await;
                        if let Err(err) = lib.delete_tag(tag_id).await {
                            log::error!("delete_tag: {err}");
                            return;
                        }
                        refresh_snapshot(&lib, &mut snap);
                    });
                }),
        )
}

fn folders_list(
    index: &LibraryIndex,
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
) -> impl IntoElement {
    let mut column = rect()
        .vertical()
        .spacing(4.0)
        .child(section_label("Folders"));
    let mut count = 0;
    for folder in &index.folders {
        if folder.parent != ROOT_FOLDER {
            continue;
        }
        count += 1;
        column = column.child(folder_row(
            folder.id,
            folder.name.clone(),
            handle.clone(),
            snap,
        ));
    }
    if count == 0 {
        column = column.child(muted_label("no folders at root"));
    }
    column
}

fn folder_row(
    id: FolderId,
    name: String,
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
) -> impl IntoElement {
    row_scaffold(&format!("📁  {}  (id={})", name, id.0)).child(delete_button(move |()| {
        let handle = handle.clone();
        let mut snap = snap;
        spawn(async move {
            let mut lib = handle.lock().await;
            if let Err(err) = lib.delete_folder(id).await {
                log::error!("delete_folder: {err}");
                return;
            }
            refresh_snapshot(&lib, &mut snap);
        });
    }))
}

fn items_list(
    index: &LibraryIndex,
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
) -> impl IntoElement {
    let mut column = rect().vertical().spacing(4.0).child(section_label("Items"));
    let mut count = 0;
    for item in &index.items {
        if item.folder != ROOT_FOLDER {
            continue;
        }
        count += 1;
        let id = item.id;
        let name = item.name.clone();
        let kind_icon = match item.kind {
            ItemKind::Canvas => "🖌",
            ItemKind::PdfCanvas => "📄",
        };
        let tag_suffix = if item.tags.is_empty() {
            String::new()
        } else {
            format!("  [{} tag(s)]", item.tags.len())
        };
        let text = format!("{kind_icon}  {name}  (id={}){}", id.0, tag_suffix);
        column = column.child(row_scaffold(&text).child(delete_button({
            let handle = handle.clone();
            move |()| {
                let handle = handle.clone();
                let mut snap = snap;
                spawn(async move {
                    let mut lib = handle.lock().await;
                    if let Err(err) = lib.delete_item(id).await {
                        log::error!("delete_item: {err}");
                        return;
                    }
                    refresh_snapshot(&lib, &mut snap);
                });
            }
        })));
    }
    if count == 0 {
        column = column.child(muted_label("no items at root"));
    }
    column
}

fn row_scaffold(text: &str) -> Rect {
    rect()
        .horizontal()
        .spacing(8.0)
        .padding((6.0, 8.0))
        .background(Color::from_rgb(44, 44, 54))
        .with_corner_radius(6.0)
        .child(
            label()
                .color(Color::WHITE)
                .font_size(13.0)
                .text(text.to_owned()),
        )
}

fn delete_button<F: Fn(()) + 'static>(handler: F) -> Rect {
    rect()
        .padding((2.0, 8.0))
        .background(Color::from_rgb(180, 60, 60))
        .with_corner_radius(6.0)
        .child(label().color(Color::WHITE).font_size(12.0).text("del"))
        .on_press(move |_| handler(()))
}

fn mini_button<F: Fn(()) + 'static>(text: &'static str, bg: Color, handler: F) -> Rect {
    rect()
        .padding((6.0, 10.0))
        .background(bg)
        .with_corner_radius(8.0)
        .child(label().color(Color::WHITE).font_size(12.0).text(text))
        .on_press(move |_| handler(()))
}

fn section_label(text: &'static str) -> impl IntoElement {
    label()
        .color(Color::from_rgb(160, 160, 180))
        .font_size(11.0)
        .text(text)
}

fn muted_label(text: &'static str) -> impl IntoElement {
    label()
        .color(Color::from_rgb(140, 140, 160))
        .font_size(12.0)
        .text(text)
}

fn error_label(text: &str) -> impl IntoElement {
    label()
        .color(Color::from_rgb(240, 120, 120))
        .font_size(12.0)
        .text(text.to_owned())
}

fn divider() -> impl IntoElement {
    rect()
        .width(Size::fill())
        .height(Size::px(1.0))
        .background(Color::from_rgb(60, 60, 72))
}

// ---------------------------------------------------------------------------
// Library plumbing helpers
// ---------------------------------------------------------------------------

async fn open_library() -> Result<LibHandle, String> {
    let runtime = Runtime::global().map_err(|e| format!("runtime not started: {e}"))?;
    let lib = Library::open(&runtime)
        .await
        .map_err(|e| format!("open library: {e}"))?;
    Ok(Arc::new(AsyncMutex::new(lib)))
}

/// Push the current in-memory index into the reactive snapshot slot
/// so the panel re-renders.
fn refresh_snapshot(lib: &Library, snap: &mut State<Option<LibraryIndex>>) {
    snap.set(Some(lib.index().clone()));
}

fn spawn_snapshot(handle: LibHandle, mut snap: State<Option<LibraryIndex>>) {
    spawn(async move {
        let index = handle.lock().await.index().clone();
        snap.set(Some(index));
    });
}

// ---------------------------------------------------------------------------
// Colour helpers
// ---------------------------------------------------------------------------

fn rgba_to_color(rgba: Rgba) -> Color {
    Color::from_argb(rgba[3], rgba[0], rgba[1], rgba[2])
}

/// Cycle through a small preset palette for tag creation so a demo
/// user gets visible colour variety without a colour picker UI yet.
fn pick_tag_color() -> Rgba {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static IDX: AtomicUsize = AtomicUsize::new(0);
    const PALETTE: [Rgba; 6] = [
        [230, 110, 110, 255],
        [110, 200, 130, 255],
        [110, 160, 230, 255],
        [230, 200, 90, 255],
        [200, 130, 230, 255],
        [110, 210, 210, 255],
    ];
    let i = IDX.fetch_add(1, Ordering::Relaxed) % PALETTE.len();
    PALETTE[i]
}

// ---------------------------------------------------------------------------
// Existing picker flow (unchanged)
// ---------------------------------------------------------------------------

fn canvas_button() -> impl IntoElement {
    action_button("New infinite canvas", Color::from_rgb(80, 140, 220)).on_press(|_| {
        let _ = RouterContext::get().push(Route::CanvasView);
    })
}

fn pdf_button(mut status: State<PickerStatus>) -> impl IntoElement {
    action_button("Open a PDF", Color::from_rgb(120, 90, 200)).on_press(move |_| {
        if matches!(
            *status.read(),
            PickerStatus::Picking | PickerStatus::Reading
        ) {
            return;
        }
        status.set(PickerStatus::Picking);
        spawn(async move {
            match load_pdf_via_picker(&mut status).await {
                Ok(PickerOutcome::Loaded) => {
                    status.set(PickerStatus::Idle);
                    let _ = RouterContext::get().push(Route::CanvasPdfView);
                }
                Ok(PickerOutcome::Cancelled) => {
                    status.set(PickerStatus::Idle);
                }
                Err(msg) => {
                    log::error!("file-picker flow: {msg}");
                    status.set(PickerStatus::Error(msg));
                }
            }
        });
    })
}

enum PickerOutcome {
    Loaded,
    Cancelled,
}

async fn load_pdf_via_picker(status: &mut State<PickerStatus>) -> Result<PickerOutcome, String> {
    let runtime = Runtime::global().map_err(|e| format!("runtime not started: {e}"))?;
    let picker = FilePickerClient::from_runtime(&runtime)
        .map_err(|e| format!("file-picker plugin unavailable: {e}"))?;

    let config = PickConfig {
        dialog_title: Some("Open a PDF".into()),
        filter: FileFilter {
            mime_types: vec!["application/pdf".into()],
            extensions: vec!["pdf".into()],
            uti_types: vec!["com.adobe.pdf".into()],
        },
        start_directory_hint: None,
        allow_multiple: false,
    };

    let Some(picked) = pick_file(&picker, config)
        .await
        .map_err(|e| picker_error_message(&e))?
    else {
        return Ok(PickerOutcome::Cancelled);
    };
    let picked = Arc::new(picked);

    status.set(PickerStatus::Reading);
    let mut reader = picked
        .open_reader(&picker)
        .await
        .map_err(|e| format!("open: {e}"))?;
    let bytes = read_all(&mut reader).map_err(|e| format!("read: {e}"))?;

    let doc = PdfDocument::open_bytes(bytes).map_err(|e| format!("parse PDF: {e}"))?;
    let background: Arc<dyn freya_canvas_bg::CanvasBackground> = Arc::new(PdfBackground::new(doc));
    let board = Board::shared();
    lock(&board).set_background(background);
    log::info!("loaded PDF background: {}", picked.display_name);
    Ok(PickerOutcome::Loaded)
}

fn read_all(reader: &mut PickedFileReader) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

async fn pick_file(
    picker: &FilePickerClient,
    config: PickConfig,
) -> Result<Option<OwnedPickedFile>, FilePickerError> {
    match picker.pick_file_owned(config).await {
        Ok(v) => Ok(v),
        Err(IstmoError::PluginError { bytes }) => {
            let (err, _) = bincode::decode_from_slice::<FilePickerError, _>(
                &bytes,
                bincode::config::standard(),
            )
            .map_err(|e| FilePickerError::Io(format!("decode plugin error payload: {e}")))?;
            Err(err)
        }
        Err(other) => Err(FilePickerError::Backend(other.to_string())),
    }
}

fn picker_error_message(err: &FilePickerError) -> String {
    match err {
        FilePickerError::UserCancelled => "cancelled".into(),
        other => other.to_string(),
    }
}

fn action_button(text: &'static str, background: Color) -> Rect {
    rect()
        .width(Size::px(260.0))
        .padding((14.0, 20.0))
        .background(background)
        .with_corner_radius(10.0)
        .center()
        .child(label().color(Color::WHITE).font_size(16.0).text(text))
}

fn status_label(status: &PickerStatus, text: String) -> impl IntoElement {
    let color = if status.is_error() {
        Color::from_rgb(240, 120, 120)
    } else {
        Color::from_rgb(180, 180, 190)
    };
    label().color(color).font_size(13.0).text(text)
}

fn spacer(px: f32) -> impl IntoElement {
    rect().width(Size::px(1.0)).height(Size::px(px))
}

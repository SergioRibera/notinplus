//! Landing page rendered by [`crate::route::Route::Home`].
//!
//! One button opens a blank infinite canvas; the other launches the
//! platform file picker via the `istmo-file-picker` plugin, reads the
//! chosen PDF into a [`freya_pdf::PdfBackground`], installs it on the
//! shared [`Board`], and navigates to [`crate::route::Route::CanvasPdfView`].
//!
//! State surfaced back to the UI:
//!
//! * `status` — a short reactive label under the buttons that tracks
//!   the async pick + read + parse pipeline (idle / picking / reading
//!   / parse-error / open error).
//!
//! The picker call itself lives inside a `spawn`ed task since Freya
//! event handlers are synchronous.

use std::io::Read;
use std::sync::Arc;

use freya::prelude::*;
use freya::router::*;
use freya_pdf::{PdfBackground, PdfDocument};
use istmo::{IstmoError, Runtime};
use istmo_file_picker::{
    FileFilter, FilePickerClient, FilePickerError, OwnedPickedFile, PickConfig, PickedFileReader,
};

use crate::canvas::{Board, lock};
use crate::route::Route;

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

pub(crate) fn render() -> impl IntoElement {
    let status = use_state(|| PickerStatus::Idle);
    let status_display = status.read().clone();

    rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(Color::from_rgb(24, 24, 30))
        .center()
        .child(
            rect()
                .vertical()
                .spacing(20.0)
                .center()
                .child(
                    label()
                        .color(Color::WHITE)
                        .font_size(32.0)
                        .text("notinplus"),
                )
                .child(
                    label()
                        .color(Color::from_rgb(180, 180, 190))
                        .font_size(14.0)
                        .text("draw, annotate, keep."),
                )
                .child(spacer(24.0))
                .child(canvas_button())
                .child(pdf_button(status))
                .maybe_child(status_display.text().map(|t| status_label(&status_display, t))),
        )
}

fn canvas_button() -> impl IntoElement {
    action_button("New infinite canvas", Color::from_rgb(80, 140, 220))
        .on_press(|_| {
            let _ = RouterContext::get().push(Route::CanvasView);
        })
}

fn pdf_button(mut status: State<PickerStatus>) -> impl IntoElement {
    action_button("Open a PDF", Color::from_rgb(120, 90, 200))
        .on_press(move |_| {
            if matches!(*status.read(), PickerStatus::Picking | PickerStatus::Reading) {
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
        // User cancelled the picker without selecting a file — no
        // banner, no navigation, back to idle so they can retry.
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

/// Wrap [`FilePickerClient::pick_file_owned`] so the caller sees the
/// typed [`FilePickerError`] instead of the wire-level [`IstmoError`].
///
/// Trait-declared domain errors travel over the frame protocol inside
/// [`IstmoError::PluginError`] as a bincode blob; every caller that
/// wants to react to specific variants (cancellation, permission
/// denied, …) has to decode it. The `fd_bridge` module inside
/// `istmo-file-picker` does the same dance for its own methods but
/// keeps the helper private, so we replicate it here for the picker
/// entry points we actually invoke.
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
        .child(
            label()
                .color(Color::WHITE)
                .font_size(16.0)
                .text(text),
        )
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

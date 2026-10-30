//! The camera session behind the scanner, on a thread of its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use istmo::StreamItem;
use istmo_camera::{
    AccessStatus, CameraClient, CameraClientExt, CameraControls, CameraError, CameraFacing,
    CameraInitConfig, FocusMode, PixelFormat, Resolution, SessionConfig, SessionHandle,
};

use super::ScanError;
use super::frame::Picture;

/// What the capture thread reports to the sheet.
#[derive(Debug)]
pub(super) enum Event {
    /// The newest picture from the camera.
    Picture(Picture),
    /// A code was read; the thread has stopped.
    Found(String),
    /// The camera could not start or stopped; the thread has ended.
    Failed(ScanError),
}

impl From<CameraError> for ScanError {
    fn from(err: CameraError) -> Self {
        match err {
            CameraError::PermissionDenied => Self::Denied,
            CameraError::DeviceNotFound(_) => Self::Unavailable,
            CameraError::ConcurrencyUnsupported => Self::Busy,
            CameraError::Interrupted | CameraError::SessionClosed => Self::Interrupted,
            CameraError::FormatUnsupported => Self::NoFormat,
            other @ CameraError::Backend(_) => Self::Camera(other),
        }
    }
}

/// A running capture. Dropping it stops the thread, which releases the
/// camera.
#[derive(Debug)]
pub(super) struct Capture {
    stop: Arc<AtomicBool>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Capture {
    /// Open the camera and read frames until a code turns up.
    pub(super) fn start() -> (Self, flume::Receiver<Event>) {
        // Room for one picture in flight: the sheet only shows the newest.
        let (tx, rx) = flume::bounded(2);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let spawned = std::thread::Builder::new()
            .name("qr-scan".into())
            .spawn(move || {
                let outcome = pollster::block_on(async {
                    let session = open().await?;
                    let found = read(&session, &thread_stop, &tx);
                    // Close rather than drop: the release a drop sends is
                    // fire and forget, and a camera still held makes the
                    // next scan find it busy.
                    match session.close().await {
                        Ok(()) => log::info!("scan: camera released"),
                        Err(err) => log::warn!("scan: releasing the camera: {err}"),
                    }
                    found
                });
                let event = match outcome {
                    Ok(Some(text)) => Event::Found(text),
                    Ok(None) => return,
                    Err(err) => Event::Failed(err),
                };
                let _ = tx.send(event);
            });
        if let Err(err) = spawned {
            log::warn!("scan: capture thread: {err}");
        }
        (Self { stop }, rx)
    }
}

/// Ask for access and open the back camera (any camera without one).
async fn open() -> Result<SessionHandle, ScanError> {
    let client = CameraClient::acquire_with(CameraInitConfig::default())
        .await
        .map_err(|err| ScanError::from(CameraError::from(err)))?;
    let client = Arc::new(client);
    match client.request_access().await.map_err(CameraError::from)? {
        AccessStatus::Granted => {}
        AccessStatus::Denied | AccessStatus::Restricted | AccessStatus::NotDetermined => {
            return Err(ScanError::Denied);
        }
    }
    let devices = client.devices().await.map_err(CameraError::from)?;
    let device = devices
        .iter()
        .find(|device| device.facing == CameraFacing::Back)
        .or_else(|| devices.first())
        .ok_or(ScanError::Unavailable)?;
    log::info!("scan: camera {} ({:?})", device.label, device.facing);
    let base = SessionConfig::new(&device.id)
        .with_target_fps(30)
        .with_initial_controls(CameraControls {
            focus: Some(FocusMode::ContinuousVideo),
            ..CameraControls::default()
        });
    // Packed RGBA is what both the preview and the decoder take; the
    // resolution is a wish the camera may not grant.
    let attempts = [
        base.clone()
            .with_pixel_format(PixelFormat::Rgba8)
            .with_resolution(Resolution {
                width: 1280,
                height: 720,
            }),
        base.clone().with_pixel_format(PixelFormat::Rgba8),
        base.with_pixel_format(PixelFormat::Bgra8),
    ];
    for config in attempts {
        match client.open_session(config).await {
            Ok(session) => {
                log::info!("scan: session {:?}", session.negotiated());
                return Ok(session);
            }
            Err(CameraError::FormatUnsupported) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Err(ScanError::NoFormat)
}

/// Forward pictures until a code is read (`Some`) or `stop` is raised
/// (`None`).
fn read(
    session: &SessionHandle,
    stop: &AtomicBool,
    tx: &flume::Sender<Event>,
) -> Result<Option<String>, ScanError> {
    let frames = session.frames()?;
    while !stop.load(Ordering::Acquire) {
        let frame = match frames.recv() {
            Ok(StreamItem::Event(frame)) => frame,
            Ok(StreamItem::Completed | StreamItem::Cancelled | StreamItem::Failed(())) => {
                return Err(ScanError::Stopped);
            }
            Err(err) => {
                log::warn!("scan: frame stream: {err}");
                return Err(ScanError::Stopped);
            }
        };
        let Some(picture) = Picture::of(&frame) else {
            continue;
        };
        if let Some(text) = picture.read_qr() {
            return Ok(Some(text));
        }
        if let Err(flume::TrySendError::Disconnected(_)) = tx.try_send(Event::Picture(picture)) {
            break;
        }
    }
    Ok(None)
}

//! Reading QR codes with the camera: a sheet with the live picture that
//! closes on the first code it reads.

mod capture;
mod frame;

use std::cell::RefCell;
use std::rc::Rc;

use freya::prelude::*;
use freya_engine::prelude::{
    AlphaType, Color as SkColor, ColorType, ImageInfo, Paint, PaintStyle, RRect, Rect, SkData,
    SkImage, raster_from_data,
};
use notein_i18n::tr;
use notein_plugin::UserMessage;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Button, Sheet, SheetFooter, Spinner, Text, TextRole, tokens};

use capture::{Capture, Event};
use frame::Picture;

/// Height of the camera picture in the sheet.
const PREVIEW_HEIGHT: f32 = 320.0;
/// Side of the aiming square, as a fraction of the picture's short side.
const FINDER: f32 = 0.62;

/// Why the camera scanner did not return a code.
#[derive(Debug)]
pub enum ScanError {
    /// No camera on this device.
    Unavailable,
    /// The user refused camera access.
    Denied,
    /// Another session (or app) holds the camera.
    Busy,
    /// The system interrupted the capture (a call, the app paused).
    Interrupted,
    /// The camera offers no picture format the decoder reads.
    NoFormat,
    /// The picture stream ended or failed while scanning.
    Stopped,
    /// Any other camera failure.
    Camera(istmo_camera::CameraError),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("no camera"),
            Self::Denied => f.write_str("camera access denied"),
            Self::Busy => f.write_str("camera busy"),
            Self::Interrupted => f.write_str("camera capture interrupted"),
            Self::NoFormat => f.write_str("the camera offers no RGBA format"),
            Self::Stopped => f.write_str("the camera stopped"),
            Self::Camera(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Camera(err) => Some(err),
            _ => None,
        }
    }
}

impl UserMessage for ScanError {
    fn user_message(&self) -> String {
        match self {
            Self::Unavailable => tr!("errors-scan-unavailable"),
            Self::Denied => tr!("errors-scan-denied"),
            Self::Busy => tr!("errors-scan-busy"),
            Self::Interrupted => tr!("errors-scan-interrupted"),
            Self::NoFormat => tr!("errors-scan-no-format"),
            Self::Stopped => tr!("errors-scan-stopped"),
            Self::Camera(_) => tr!("errors-scan-failed"),
        }
    }
}

type Outcome = Result<Option<String>, ScanError>;

/// Open the scanner sheet over the current one, with `prompt` under the
/// picture. Resolves with the code read, `Ok(None)` when the user backs
/// out. Call it from the UI thread; await the result anywhere.
///
/// # Errors
/// [`ScanError`] when there is no camera, no permission, or it fails.
pub fn scan_qr(prompt: &str) -> impl Future<Output = Outcome> + use<> {
    let (tx, rx) = flume::bounded(1);
    Sheet::titled(tr!("scan-title"))
        .child(Scanner {
            prompt: prompt.to_owned(),
            reply: Reply(Rc::new(RefCell::new(Some(tx)))),
        })
        .footer(SheetFooter::new([Button::new(tr!("common-cancel"))
            .ghost()
            .on_press(|_| Sheet::close())
            .into_element()]))
        .push();
    async move { rx.recv_async().await.unwrap_or(Ok(None)) }
}

/// Where the scanner's outcome goes; answered once.
#[derive(Clone)]
struct Reply(Rc<RefCell<Option<flume::Sender<Outcome>>>>);

impl PartialEq for Reply {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Reply {
    fn send(&self, outcome: Outcome) {
        if let Some(tx) = self.0.borrow_mut().take() {
            let _ = tx.send(outcome);
        }
    }
}

/// The newest picture, ready to draw.
struct Shown {
    image: SkImage,
    rotation: u16,
}

impl Shown {
    fn of(picture: &Picture) -> Option<Self> {
        let width = i32::try_from(picture.width).ok()?;
        let height = i32::try_from(picture.height).ok()?;
        let info = ImageInfo::new(
            (width, height),
            ColorType::RGBA8888,
            AlphaType::Unpremul,
            None,
        );
        let image = raster_from_data(
            &info,
            SkData::new_copy(&picture.rgba),
            usize::try_from(width).ok()? * 4,
        )?;
        Some(Self {
            image,
            rotation: picture.rotation,
        })
    }

    /// Fill `size` with the picture turned upright, cropping the excess.
    fn paint(&self, canvas: &freya_engine::prelude::Canvas, (width, height): (f32, f32)) {
        #[allow(
            clippy::cast_precision_loss,
            reason = "picture sides stay below 1000 px"
        )]
        let (w, h) = (self.image.width() as f32, self.image.height() as f32);
        let turned = self.rotation % 180 == 90;
        let (upright_w, upright_h) = if turned { (h, w) } else { (w, h) };
        let scale = (width / upright_w).max(height / upright_h);
        canvas.save();
        canvas.translate((width * 0.5, height * 0.5));
        canvas.rotate(f32::from(self.rotation), None);
        let (draw_w, draw_h) = (w * scale, h * scale);
        canvas.draw_image_rect(
            &self.image,
            None,
            Rect::from_xywh(-draw_w * 0.5, -draw_h * 0.5, draw_w, draw_h),
            &Paint::default(),
        );
        canvas.restore();
    }
}

/// The square to hold the code in.
fn paint_finder(canvas: &freya_engine::prelude::Canvas, (width, height): (f32, f32)) {
    let side = width.min(height) * FINDER;
    let square = Rect::from_xywh((width - side) * 0.5, (height - side) * 0.5, side, side);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Stroke);
    paint.set_stroke_width(3.0);
    paint.set_color(SkColor::from_argb(0xE6, 0xFF, 0xFF, 0xFF));
    canvas.draw_rrect(RRect::new_rect_xy(square, 16.0, 16.0), &paint);
}

/// Body of the scanner sheet: the live picture and the prompt.
#[derive(Clone, PartialEq)]
struct Scanner {
    prompt: String,
    reply: Reply,
}

impl Component for Scanner {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let shown = use_hook(|| Rc::new(RefCell::new(None::<Shown>)));
        let mut pictures = use_state(|| 0u64);
        let capture = use_hook({
            let reply = self.reply.clone();
            let shown = Rc::clone(&shown);
            move || {
                let (capture, rx) = Capture::start();
                spawn(async move {
                    while let Ok(event) = rx.recv_async().await {
                        match event {
                            Event::Picture(picture) => {
                                *shown.borrow_mut() = Shown::of(&picture);
                                // A write per picture repaints the preview.
                                *pictures.write() += 1;
                            }
                            Event::Found(text) => {
                                log::info!("scan: read a code of {} bytes", text.len());
                                reply.send(Ok(Some(text)));
                                Sheet::close();
                            }
                            Event::Failed(err) => {
                                log::warn!("scan: {err}");
                                reply.send(Err(err));
                                Sheet::close();
                            }
                        }
                    }
                });
                Rc::new(RefCell::new(Some(capture)))
            }
        });
        use_drop({
            let reply = self.reply.clone();
            move || {
                // Stops the camera; backing out answers `None`.
                capture.borrow_mut().take();
                reply.send(Ok(None));
            }
        });

        let painted = Rc::clone(&shown);
        let preview = rect()
            .width(Size::fill())
            .height(Size::px(PREVIEW_HEIGHT))
            .corner_radius(tokens::radius::LG)
            .overflow(Overflow::Clip)
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .spacing(10.0)
            .background(RenderCallback::new(move |ctx| {
                let RenderTarget::Background { canvas, .. } = &ctx.target else {
                    return;
                };
                let size = (ctx.size.width, ctx.size.height);
                let mut black = Paint::default();
                black.set_color(SkColor::BLACK);
                canvas.draw_rect(Rect::from_wh(size.0, size.1), &black);
                if let Some(shown) = painted.borrow().as_ref() {
                    shown.paint(canvas, size);
                }
                paint_finder(canvas, size);
            }))
            .maybe(pictures() == 0, |starting| {
                starting
                    .child(Spinner::default().size(24.0).color(Color::WHITE))
                    .child(
                        Text::new(tr!("scan-opening-camera"))
                            .role(TextRole::Meta)
                            .color(Color::WHITE),
                    )
            });
        rect()
            .width(Size::fill())
            .spacing(12.0)
            .child(preview)
            .child(
                Text::new(self.prompt.clone())
                    .role(TextRole::Hint)
                    .color(p.t2),
            )
    }
}

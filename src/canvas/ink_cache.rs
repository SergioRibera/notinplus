//! Committed ink kept as an image of the screen.
//!
//! Painting every stroke on every frame costs more the more a page holds,
//! even when nothing but the pen moved. The paint pass instead draws the
//! committed ink (and, for the bottom pass, the paper under it) once into
//! an offscreen image a little larger than the view, at the screen's own
//! pixel density, and copies that image on the frames that follow.
//!
//! - Panning shifts the image by whole pixels, so it stays sharp; the
//!   image is drawn again once the view leaves its margin.
//! - While two fingers zoom, the image is stretched and looks soft; it is
//!   drawn again, sharp, as soon as the fingers lift.
//! - Any change to the ink, the paper or the screen draws it again.

#![allow(
    clippy::cast_precision_loss,
    clippy::suboptimal_flops,
    reason = "pixel counts stay far below 2^24; plain arithmetic reads closer to the geometry"
)]

use freya_engine::prelude::{
    Canvas, Color, CubicResampler, FilterMode, IRect, Image, ImageInfo, Matrix, MipmapMode,
    SamplingOptions, Surface,
};

use super::{SurfaceBounds, Viewport};

/// Extra image kept past each edge of the view, as a share of its size.
const MARGIN: f32 = 0.125;
/// Largest image kept, in pixels.
const MAX_PIXELS: i64 = 48 << 20;

const STRETCHED: SamplingOptions = SamplingOptions {
    max_aniso: 0,
    use_cubic: false,
    cubic: CubicResampler { b: 0.0, c: 0.0 },
    filter: FilterMode::Linear,
    mipmap: MipmapMode::None,
};

/// What the image shows: revisions of the ink and of the paper.
type Content = (u64, u64);

/// The ink of one paint pass, held between frames. Lives with the paint
/// callback, on the thread that owns the GPU context.
#[derive(Debug, Default)]
pub struct InkCache {
    surface: Option<Surface>,
    image: Option<Image>,
    held: Option<Held>,
    /// Whether the ink of the pass blends with what lies under it, for
    /// the ink revision it was worked out for.
    blends: Option<(u64, bool)>,
}

/// Where a frame is painted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// Device pixels per local unit.
    pub density: f32,
    /// Device position of local `(0, 0)`.
    pub offset: (f32, f32),
    /// Local size of the paint surface.
    pub size: (f32, f32),
    pub viewport: Viewport,
    /// Device pixels being drawn.
    pub clip: Option<IRect>,
}

impl Frame {
    /// The frame `canvas` paints, when its transform only scales evenly
    /// and translates.
    pub fn of(canvas: &Canvas, size: (f32, f32), viewport: Viewport) -> Option<Self> {
        let device: Matrix = canvas.local_to_device_as_3x3();
        let density = device.scale_x();
        let even = device.is_scale_translate()
            && density.is_finite()
            && density > 0.0
            && (density - device.scale_y()).abs() < 1e-4;
        even.then(|| Self {
            density,
            offset: (device.translate_x(), device.translate_y()),
            size,
            viewport,
            clip: canvas.device_clip_bounds(),
        })
    }

    /// Device position of world `(0, 0)`.
    fn origin(&self) -> (f32, f32) {
        (
            self.offset.0 + self.density * self.viewport.tx,
            self.offset.1 + self.density * self.viewport.ty,
        )
    }

    /// Device pixels per world unit.
    fn zoom(&self) -> f32 {
        self.density * self.viewport.scale
    }

    /// Device rect the surface covers and is being drawn.
    fn visible(&self) -> (f32, f32, f32, f32) {
        let (x, y) = self.offset;
        let mut rect = (
            x,
            y,
            x + self.size.0 * self.density,
            y + self.size.1 * self.density,
        );
        if let Some(clip) = self.clip {
            rect = (
                rect.0.max(clip.left as f32),
                rect.1.max(clip.top as f32),
                rect.2.min(clip.right as f32),
                rect.3.min(clip.bottom as f32),
            );
        }
        rect
    }
}

/// What the held image was drawn for.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Held {
    content: Content,
    density: f32,
    size: (f32, f32),
    /// Device position of world `(0, 0)` when drawn.
    origin: (f32, f32),
    /// Device pixels per world unit when drawn.
    zoom: f32,
    /// Device position of the image's first pixel when drawn.
    corner: (f32, f32),
    pixels: (i32, i32),
}

/// How the held image lands on a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Placement {
    /// Same zoom: copied pixel for pixel at this device position.
    Shifted(f32, f32),
    /// Another zoom: image pixel `q` lands at `scale * q + offset`.
    Stretched { scale: f32, offset: (f32, f32) },
}

impl Held {
    fn placement(&self, frame: &Frame) -> Placement {
        let (x, y) = frame.origin();
        let zoom = frame.zoom();
        if (zoom - self.zoom).abs() <= f32::EPSILON * self.zoom.max(1.0) {
            return Placement::Shifted(
                (self.corner.0 + x - self.origin.0).round(),
                (self.corner.1 + y - self.origin.1).round(),
            );
        }
        let scale = zoom / self.zoom;
        Placement::Stretched {
            scale,
            offset: (
                x - scale * (self.origin.0 - self.corner.0),
                y - scale * (self.origin.1 - self.corner.1),
            ),
        }
    }

    /// Whether the image, placed for `frame`, fills what it shows.
    fn covers(&self, frame: &Frame) -> bool {
        const SLACK: f32 = 1.0;
        let (scale, (ox, oy)) = match self.placement(frame) {
            Placement::Shifted(x, y) => (1.0, (x, y)),
            Placement::Stretched { scale, offset } => (scale, offset),
        };
        let (min_x, min_y, max_x, max_y) = frame.visible();
        if min_x >= max_x || min_y >= max_y {
            return true;
        }
        let (w, h) = (self.pixels.0 as f32, self.pixels.1 as f32);
        (min_x - ox) / scale >= -SLACK
            && (min_y - oy) / scale >= -SLACK
            && (max_x - ox) / scale <= w + SLACK
            && (max_y - oy) / scale <= h + SLACK
    }
}

impl InkCache {
    /// Whether the held image can serve `frame` without drawing again.
    /// A stretched image only serves while `zooming`.
    #[must_use]
    #[sismo::function("ink_cache::fits")]
    pub fn fits(&self, frame: &Frame, content: Content, zooming: bool) -> bool {
        let Some(held) = &self.held else {
            sismo::instant!("ink_cache::miss_empty");
            return false;
        };
        if self.image.is_none() {
            sismo::instant!("ink_cache::miss_no_image");
            return false;
        }
        if held.content != content {
            sismo::instant!("ink_cache::miss_content");
            return false;
        }
        if (held.density - frame.density).abs() > f32::EPSILON || held.size != frame.size {
            sismo::instant!("ink_cache::miss_size");
            return false;
        }
        if !zooming && !matches!(held.placement(frame), Placement::Shifted(..)) {
            sismo::instant!("ink_cache::miss_zoom_changed");
            return false;
        }
        if !held.covers(frame) {
            sismo::instant!("ink_cache::miss_out_of_margin");
            return false;
        }
        true
    }

    /// Draw the image for `frame` again through `paint`, which receives
    /// a canvas already in the surface's local units and the local bounds
    /// to fill. `false` when no image could be made.
    #[sismo::function("ink_cache::redraw")]
    pub fn redraw(
        &mut self,
        canvas: &Canvas,
        frame: &Frame,
        content: Content,
        paint: impl FnOnce(&Canvas, SurfaceBounds),
    ) -> bool {
        let d = frame.density;
        #[allow(clippy::cast_possible_truncation)]
        let (w, h) = (
            (frame.size.0 * d).ceil() as i32,
            (frame.size.1 * d).ceil() as i32,
        );
        if w <= 0 || h <= 0 {
            return false;
        }
        #[allow(clippy::cast_possible_truncation)]
        let margin = (
            (w as f32 * MARGIN).ceil() as i32,
            (h as f32 * MARGIN).ceil() as i32,
        );
        let pixels = (w + 2 * margin.0, h + 2 * margin.1);
        sismo::counter!(
            "ink_cache.pixels",
            i64::from(pixels.0) * i64::from(pixels.1)
        );
        if i64::from(pixels.0) * i64::from(pixels.1) > MAX_PIXELS {
            return false;
        }
        // Let go of the last image first so the surface is drawn in place
        // instead of copied.
        {
            sismo::scope!("ink_cache::release_image");
            self.image = None;
        }
        if self.surface.as_ref().map(|s| (s.width(), s.height())) != Some(pixels) {
            sismo::scope!("ink_cache::new_surface");
            self.surface = canvas.new_surface(&ImageInfo::new_n32_premul(pixels, None), None);
        }
        let Some(surface) = self.surface.as_mut() else {
            self.held = None;
            return false;
        };
        // Keep the sub-pixel part of the device offset inside the image so
        // copying it back lands on whole pixels.
        let corner = (
            frame.offset.0.floor() - margin.0 as f32,
            frame.offset.1.floor() - margin.1 as f32,
        );
        let target = surface.canvas();
        target.save();
        target.reset_matrix();
        {
            sismo::scope!("ink_cache::clear");
            target.clear(Color::TRANSPARENT);
        }
        target.translate((frame.offset.0 - corner.0, frame.offset.1 - corner.1));
        target.scale((d, d));
        paint(
            target,
            SurfaceBounds {
                min_x: -(margin.0 as f32) / d,
                min_y: -(margin.1 as f32) / d,
                max_x: frame.size.0 + margin.0 as f32 / d,
                max_y: frame.size.1 + margin.1 as f32 / d,
            },
        );
        target.restore();
        {
            sismo::scope!("ink_cache::image_snapshot");
            self.image = Some(surface.image_snapshot());
        }
        self.held = Some(Held {
            content,
            density: d,
            size: frame.size,
            origin: frame.origin(),
            zoom: frame.zoom(),
            corner,
            pixels,
        });
        true
    }

    /// Copy the held image onto `canvas` for `frame`.
    #[sismo::function("ink_cache::draw")]
    pub fn draw(&self, canvas: &Canvas, frame: &Frame) {
        let (Some(image), Some(held)) = (&self.image, &self.held) else {
            return;
        };
        canvas.save();
        canvas.reset_matrix();
        match held.placement(frame) {
            Placement::Shifted(x, y) => {
                canvas.draw_image(image, (x, y), None);
            }
            Placement::Stretched { scale, offset } => {
                sismo::instant!("ink_cache::draw_stretched");
                canvas.translate(offset);
                canvas.scale((scale, scale));
                canvas.draw_image_with_sampling_options(image, (0.0, 0.0), STRETCHED, None);
            }
        }
        canvas.restore();
    }

    /// Whether the ink at ink revision `rev` blends with what lies under
    /// it, worked out by `blends` once per revision.
    pub fn blends(&mut self, rev: u64, blends: impl FnOnce() -> bool) -> bool {
        match self.blends {
            Some((held, answer)) if held == rev => answer,
            _ => {
                let answer = blends();
                self.blends = Some((rev, answer));
                answer
            }
        }
    }
}

impl Drop for InkCache {
    fn drop(&mut self) {
        sismo::scope!("ink_cache::drop");
        self.surface = None;
        self.image = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(tx: f32, ty: f32, scale: f32) -> Frame {
        Frame {
            density: 2.5,
            offset: (100.3, 40.0),
            size: (400.0, 300.0),
            viewport: Viewport { tx, ty, scale },
            clip: None,
        }
    }

    fn held_for(frame: &Frame) -> Held {
        let margin = (125, 94);
        Held {
            content: (0, 0),
            density: frame.density,
            size: frame.size,
            origin: frame.origin(),
            zoom: frame.zoom(),
            corner: (
                frame.offset.0.floor() - margin.0 as f32,
                frame.offset.1.floor() - margin.1 as f32,
            ),
            pixels: (1000 + 2 * margin.0, 750 + 2 * margin.1),
        }
    }

    #[test]
    fn an_unchanged_view_copies_the_image_onto_whole_pixels() {
        let f = frame(10.0, -20.0, 1.5);
        let held = held_for(&f);
        assert_eq!(held.placement(&f), Placement::Shifted(-25.0, -54.0));
        assert!(held.covers(&f));
    }

    #[test]
    fn a_pan_shifts_the_image_until_it_leaves_the_margin() {
        let f = frame(10.0, -20.0, 1.5);
        let held = held_for(&f);
        let near = frame(10.0 + 40.0, -20.0, 1.5);
        assert_eq!(held.placement(&near), Placement::Shifted(75.0, -54.0));
        assert!(held.covers(&near));
        let far = frame(10.0 + 60.0, -20.0, 1.5);
        assert!(!held.covers(&far));
    }

    #[test]
    fn a_zoom_stretches_the_image_about_the_world() {
        let f = frame(0.0, 0.0, 1.0);
        let held = held_for(&f);
        let zoomed = frame(-100.0, -50.0, 2.0);
        let Placement::Stretched { scale, offset } = held.placement(&zoomed) else {
            panic!("expected a stretch");
        };
        assert!((scale - 2.0).abs() < 1e-6);
        // World (40, 30) sits on image pixel corner-relative; both frames
        // must put it at the same device position.
        let (wx, wy) = (40.0, 30.0);
        let qx = f.origin().0 + f.zoom() * wx - held.corner.0;
        let qy = f.origin().1 + f.zoom() * wy - held.corner.1;
        let (dx, dy) = (scale * qx + offset.0, scale * qy + offset.1);
        assert!((dx - (zoomed.origin().0 + zoomed.zoom() * wx)).abs() < 1e-3);
        assert!((dy - (zoomed.origin().1 + zoomed.zoom() * wy)).abs() < 1e-3);
        assert!(held.covers(&zoomed));
    }

    #[test]
    fn zooming_out_past_the_margin_is_not_covered() {
        let f = frame(0.0, 0.0, 1.0);
        let held = held_for(&f);
        assert!(!held.covers(&frame(0.0, 0.0, 0.5)));
    }
}

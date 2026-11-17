//! Canvas images as the board paints them: decoded off the UI thread and
//! drawn into the layer passes under each layer's ink, so an image sits in
//! its layer's paint order like any stroke.
//!
//! Images used to be UI elements laid between the passes, but Freya paints
//! by tree depth, not by sibling order: a nested image always covered the
//! ink of the layers above it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use freya_canvas_bg::RedrawHandle;
use freya_engine::prelude::{
    AlphaType, Canvas, Color as SkColor, ColorType, ISize, ImageInfo, Paint, Rect, SkData, SkImage,
    raster_from_data,
};

use crate::image_box::ImageBox;
use crate::library::ItemId;

/// Fill of an image whose pixels are not here (yet).
const PLACEHOLDER: SkColor = SkColor::from_argb(40, 128, 128, 128);

/// Where one image's pixels are.
enum Slot {
    Loading,
    Ready(SkImage),
    /// Unreadable; tried again when more of the item's files arrive.
    Missing,
}

type Slots = Arc<Mutex<HashMap<String, Slot>>>;

/// Decoded images of one item, keyed by asset name.
#[derive(Default)]
pub(super) struct Pictures {
    item: Option<ItemId>,
    slots: Slots,
}

impl std::fmt::Debug for Pictures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pictures")
            .field("item", &self.item)
            .finish_non_exhaustive()
    }
}

impl Pictures {
    /// Read images from `item` from now on. `true` when that changed what
    /// is painted.
    pub(super) fn set_item(&mut self, item: Option<ItemId>) -> bool {
        if self.item == item {
            return false;
        }
        self.item = item;
        // Loads still running for the old item land in the old map.
        self.slots = Slots::default();
        true
    }

    /// Pixels of `asset`, starting to load them on a miss. `redraw` is
    /// pinged once they arrive.
    fn get(&self, asset: &str, redraw: &RedrawHandle) -> Option<SkImage> {
        let item = self.item?;
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        match slots.get(asset) {
            Some(Slot::Ready(image)) => return Some(image.clone()),
            Some(Slot::Loading | Slot::Missing) => return None,
            None => {}
        }
        slots.insert(asset.to_owned(), Slot::Loading);
        drop(slots);
        spawn_load(
            item,
            asset.to_owned(),
            Arc::downgrade(&self.slots),
            redraw.clone(),
        );
        None
    }

    /// Draw `images`, each shifted by its world offset, in world units
    /// under the canvas's current transform, skipping the ones outside
    /// `visible` (`min_x, min_y, max_x, max_y`).
    #[sismo::function("canvas::paint_pictures")]
    pub(super) fn paint<'a>(
        &self,
        canvas: &Canvas,
        images: impl Iterator<Item = (&'a ImageBox, (f32, f32))>,
        visible: (f32, f32, f32, f32),
        opacity: f32,
        redraw: &RedrawHandle,
    ) {
        if self.item.is_none() {
            return;
        }
        for (image, (dx, dy)) in images {
            let (x0, y0, x1, y1) = image.frame.bounds(image.height);
            if x1 + dx < visible.0
                || y1 + dy < visible.1
                || x0 + dx > visible.2
                || y0 + dy > visible.3
            {
                continue;
            }
            canvas.save();
            canvas.translate((dx, dy));
            match self.get(&image.asset, redraw) {
                Some(decoded) => draw_image(canvas, &decoded, image, opacity),
                None => draw_placeholder(canvas, image, opacity),
            }
            canvas.restore();
        }
    }
}

/// Decode `asset` on its own thread and store it in `slots`, waiting for
/// more of `item`'s files while it cannot be read.
fn spawn_load(
    item: ItemId,
    asset: String,
    slots: Weak<Mutex<HashMap<String, Slot>>>,
    redraw: RedrawHandle,
) {
    let spawned = std::thread::Builder::new()
        .name("notein-canvas-image".to_owned())
        .spawn(move || {
            let arrivals = crate::plugins::content::watch_assets(item);
            loop {
                let slot = decode(item, &asset).map_or(Slot::Missing, Slot::Ready);
                let ready = matches!(slot, Slot::Ready(_));
                let Some(slots) = slots.upgrade() else {
                    return;
                };
                slots
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(asset.clone(), slot);
                drop(slots);
                redraw.ping();
                if ready || arrivals.recv().is_err() {
                    return;
                }
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("canvas image {item:?}: no loader thread: {err}");
    }
}

#[sismo::function("canvas::decode_picture")]
fn decode(item: ItemId, asset: &str) -> Option<SkImage> {
    let rgba = crate::editor::images::asset_rgba(item, asset)
        .inspect_err(|err| tracing::warn!("canvas image {asset}: {err}"))
        .ok()?;
    let (width, height) = rgba.dimensions();
    let info = ImageInfo::new(
        ISize::new(i32::try_from(width).ok()?, i32::try_from(height).ok()?),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let row_bytes = usize::try_from(width).ok()? * 4;
    raster_from_data(&info, SkData::new_copy(rgba.as_raw()), row_bytes)
}

/// Draw `decoded` stretched over `image`'s frame, turned around the
/// frame's centre.
fn draw_image(canvas: &Canvas, decoded: &SkImage, image: &ImageBox, opacity: f32) {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_alpha_f(opacity);
    in_frame(canvas, image, |canvas, rect| {
        canvas.draw_image_rect(decoded, None, rect, &paint);
    });
}

fn draw_placeholder(canvas: &Canvas, image: &ImageBox, opacity: f32) {
    let mut paint = Paint::default();
    paint.set_color(PLACEHOLDER);
    paint.set_alpha_f(f32::from(PLACEHOLDER.a()) / 255.0 * opacity);
    in_frame(canvas, image, |canvas, rect| {
        canvas.draw_rect(rect, &paint);
    });
}

/// Run `draw` with the canvas turned to `image`'s rotation, handing it the
/// frame as a rect centred on the origin.
fn in_frame(canvas: &Canvas, image: &ImageBox, draw: impl FnOnce(&Canvas, Rect)) {
    let (width, height) = (image.frame.width, image.height);
    let (cx, cy) = image.frame.center(height);
    canvas.save();
    canvas.translate((cx, cy));
    canvas.rotate(image.frame.rotation, None);
    draw(
        canvas,
        Rect::from_xywh(-width * 0.5, -height * 0.5, width, height),
    );
    canvas.restore();
}

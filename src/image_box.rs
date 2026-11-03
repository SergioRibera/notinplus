//! Images placed on the canvas.
//!
//! An [`ImageBox`] shows one file of the item's `assets/` folder in world
//! space. Its placement reuses [`TextFrame`] (unrotated top-left corner,
//! width and a rotation around the centre); unlike a text box the height is
//! stored, since it does not follow from any content. Resizing keeps the
//! aspect ratio the image was added with.

use crate::text_box::TextFrame;

/// Widest side of a freshly added image, in world units.
pub const IMAGE_DEFAULT_SIDE: f32 = 320.0;
/// Shortest side a resize may reach, in world units.
pub const IMAGE_MIN_SIDE: f32 = 24.0;

/// Globally unique image identifier.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct ImageBoxId(pub [u8; 16]);

impl ImageBoxId {
    #[must_use]
    pub fn new_v4() -> Self {
        Self(*uuid::Uuid::new_v4().as_bytes())
    }
}

/// An image file of the item laid out on the canvas.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub struct ImageBox {
    pub id: ImageBoxId,
    pub frame: TextFrame,
    /// Height in world units; the width lives in `frame`.
    pub height: f32,
    /// File name inside the item's `assets/` folder.
    pub asset: String,
}

impl ImageBox {
    /// A new box for `asset`, `natural` pixels big, centred on `(cx, cy)`
    /// and scaled so its longest side is [`IMAGE_DEFAULT_SIDE`].
    #[must_use]
    pub fn centred(asset: impl Into<String>, natural: (u32, u32), cx: f32, cy: f32) -> Self {
        #[allow(clippy::cast_precision_loss, reason = "pixel sizes")]
        let (nw, nh) = (natural.0.max(1) as f32, natural.1.max(1) as f32);
        let scale = IMAGE_DEFAULT_SIDE / nw.max(nh);
        let (width, height) = (nw * scale, nh * scale);
        Self {
            id: ImageBoxId::new_v4(),
            frame: TextFrame {
                x: width.mul_add(-0.5, cx),
                y: height.mul_add(-0.5, cy),
                width,
                rotation: 0.0,
            },
            height,
            asset: asset.into(),
        }
    }

    /// Width over height, as the box is laid out.
    #[must_use]
    pub fn aspect(&self) -> f32 {
        self.frame.width / self.height.max(f32::EPSILON)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centred_scales_the_long_side_down() {
        let image = ImageBox::centred("a.png", (1000, 500), 100.0, 100.0);
        assert!((image.frame.width - IMAGE_DEFAULT_SIDE).abs() < f32::EPSILON);
        assert!(IMAGE_DEFAULT_SIDE.mul_add(-0.5, image.height).abs() < f32::EPSILON);
        let (cx, cy) = image.frame.center(image.height);
        assert!((cx - 100.0).abs() < 1e-3 && (cy - 100.0).abs() < 1e-3);
        assert!((image.aspect() - 2.0).abs() < 1e-3);
    }
}

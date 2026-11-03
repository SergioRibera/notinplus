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
    /// Link to where the picture was taken from (a PDF page), when it
    /// is a snapshot of another document.
    pub source: Option<String>,
}

/// Space between the pictures of one snapshot laid out as a column, in
/// world units.
const SNAPSHOT_GAP: f32 = 12.0;

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
            source: None,
        }
    }

    /// Boxes for `assets` (each `natural` pixels big), one under the
    /// other, each scaled like [`Self::centred`], the column centred on
    /// `(cx, cy)` and every box linking to `source`.
    #[must_use]
    pub fn column(
        assets: &[(String, (u32, u32))],
        source: Option<&str>,
        cx: f32,
        cy: f32,
    ) -> Vec<Self> {
        let mut boxes: Vec<Self> = assets
            .iter()
            .map(|(asset, natural)| Self::centred(asset.clone(), *natural, cx, cy))
            .collect();
        #[allow(clippy::cast_precision_loss, reason = "a handful of boxes")]
        let gaps = boxes.len().saturating_sub(1) as f32;
        let total = SNAPSHOT_GAP.mul_add(gaps, boxes.iter().map(|b| b.height).sum::<f32>());
        let mut y = total.mul_add(-0.5, cy);
        for image in &mut boxes {
            image.frame.y = y;
            image.source = source.map(str::to_owned);
            y += image.height + SNAPSHOT_GAP;
        }
        boxes
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
        assert_eq!(image.source, None);
    }

    #[test]
    fn a_column_stacks_the_boxes_around_the_centre() {
        let assets = [
            ("a.png".to_owned(), (100, 100)),
            ("b.png".to_owned(), (100, 200)),
        ];
        let column = ImageBox::column(&assets, Some("notein://x"), 0.0, 0.0);
        assert_eq!(column.len(), 2);
        assert!(
            column
                .iter()
                .all(|b| b.source.as_deref() == Some("notein://x"))
        );
        let (top, bottom) = (&column[0], &column[1]);
        assert!((top.frame.y + top.height + SNAPSHOT_GAP - bottom.frame.y).abs() < 1e-3);
        let total = top.height + SNAPSHOT_GAP + bottom.height;
        assert!(total.mul_add(0.5, top.frame.y).abs() < 1e-3);
        assert!(top.frame.width.mul_add(0.5, top.frame.x).abs() < 1e-3);
    }
}

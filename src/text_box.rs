//! Text boxes placed on the canvas.
//!
//! A [`TextBox`] holds a markdown body that the note editor lays out in
//! world space. Its [`TextFrame`] is the unrotated top-left corner, the wrap
//! width and a rotation around the box centre. The height is not stored: it
//! follows from the laid-out text, so the UI measures it and reports it to
//! the board, which needs it for hit testing.

/// Font size of body text at 100% zoom, in world units.
pub const TEXT_BASE_SIZE: f32 = 20.0;
/// Wrap width of a freshly created box, in world units.
pub const TEXT_DEFAULT_WIDTH: f32 = 320.0;
/// Narrowest wrap width a resize may reach, in world units.
pub const TEXT_MIN_WIDTH: f32 = 48.0;
/// Height assumed for a box the UI has not measured yet: one line of body
/// text.
pub const TEXT_FALLBACK_HEIGHT: f32 = TEXT_BASE_SIZE * 1.45;

/// Globally unique text box identifier.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct TextBoxId(pub [u8; 16]);

impl TextBoxId {
    #[must_use]
    pub fn new_v4() -> Self {
        Self(*uuid::Uuid::new_v4().as_bytes())
    }
}

/// Placement of a text box in world units. `rotation` is in degrees,
/// clockwise, around the centre of the laid-out box.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextFrame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub rotation: f32,
}

impl TextFrame {
    /// Unrotated frame with its top-left corner at `(x, y)` and the default
    /// wrap width.
    #[must_use]
    pub const fn at(x: f32, y: f32) -> Self {
        Self {
            x,
            y,
            width: TEXT_DEFAULT_WIDTH,
            rotation: 0.0,
        }
    }

    /// Centre of the box when it is `height` tall.
    #[must_use]
    pub fn center(&self, height: f32) -> (f32, f32) {
        (self.width.mul_add(0.5, self.x), height.mul_add(0.5, self.y))
    }

    /// Express the world point `(wx, wy)` relative to the unrotated
    /// top-left corner of the box, undoing its rotation.
    #[must_use]
    pub fn to_local(&self, height: f32, wx: f32, wy: f32) -> (f32, f32) {
        let (cx, cy) = self.center(height);
        let (sin, cos) = (-self.rotation).to_radians().sin_cos();
        let (dx, dy) = (wx - cx, wy - cy);
        (
            self.width.mul_add(0.5, dx.mul_add(cos, -dy * sin)),
            height.mul_add(0.5, dx.mul_add(sin, dy * cos)),
        )
    }

    /// Whether `(wx, wy)` lies inside the box grown by `margin` on every
    /// side.
    #[must_use]
    pub fn contains(&self, height: f32, wx: f32, wy: f32, margin: f32) -> bool {
        let (lx, ly) = self.to_local(height, wx, wy);
        (-margin..=self.width + margin).contains(&lx) && (-margin..=height + margin).contains(&ly)
    }

    /// Whether a circle of `radius` around `(wx, wy)` overlaps the box.
    #[must_use]
    pub fn touches_circle(&self, height: f32, wx: f32, wy: f32, radius: f32) -> bool {
        let (lx, ly) = self.to_local(height, wx, wy);
        let nx = lx.clamp(0.0, self.width);
        let ny = ly.clamp(0.0, height);
        (lx - nx).hypot(ly - ny) <= radius
    }

    /// Axis-aligned bounds of the rotated box: `(min_x, min_y, max_x,
    /// max_y)`.
    #[must_use]
    pub fn bounds(&self, height: f32) -> (f32, f32, f32, f32) {
        let (cx, cy) = self.center(height);
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let half_w = self.width * 0.5;
        let half_h = height * 0.5;
        let extent_x = (half_w * cos).abs() + (half_h * sin).abs();
        let extent_y = (half_w * sin).abs() + (half_h * cos).abs();
        (cx - extent_x, cy - extent_y, cx + extent_x, cy + extent_y)
    }

    /// World position of the unrotated local point `(lx, ly)`.
    #[must_use]
    pub fn to_world(&self, height: f32, lx: f32, ly: f32) -> (f32, f32) {
        let (cx, cy) = self.center(height);
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let (dx, dy) = (self.width.mul_add(-0.5, lx), height.mul_add(-0.5, ly));
        (
            dx.mul_add(cos, -dy * sin) + cx,
            dx.mul_add(sin, dy * cos) + cy,
        )
    }
}

/// Markdown text laid out on the canvas.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub struct TextBox {
    pub id: TextBoxId,
    pub frame: TextFrame,
    pub markdown: String,
    /// Body text colour as RGBA. Inline colour marks in `markdown` win.
    pub color: [u8; 4],
}

impl TextBox {
    /// Whether the body holds nothing worth keeping.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.markdown.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotated(rotation: f32) -> TextFrame {
        TextFrame {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            rotation,
        }
    }

    #[test]
    fn local_round_trips_through_world() {
        let frame = rotated(33.0);
        let (wx, wy) = frame.to_world(40.0, 12.0, 7.0);
        let (lx, ly) = frame.to_local(40.0, wx, wy);
        assert!((lx - 12.0).abs() < 1e-3 && (ly - 7.0).abs() < 1e-3);
    }

    #[test]
    fn rotation_moves_the_hit_area() {
        let frame = rotated(90.0);
        // Unrotated the box spans x 10..110; turned a quarter it is 40 wide
        // and 100 tall around the same centre (60, 40).
        assert!(!frame.contains(40.0, 100.0, 40.0, 0.0));
        assert!(frame.contains(40.0, 60.0, 85.0, 0.0));
    }

    #[test]
    fn circle_touches_edge() {
        let frame = rotated(0.0);
        assert!(frame.touches_circle(40.0, 115.0, 30.0, 6.0));
        assert!(!frame.touches_circle(40.0, 120.0, 30.0, 6.0));
    }
}

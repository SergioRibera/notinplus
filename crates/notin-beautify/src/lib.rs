//! Ink beautification.
//!
//! The ink stays the writer's own: raw samples are never rewritten.
//! What this crate computes is a small correction stored next to them —
//! a [`StrokeFix`] (smoothing + one affine per handwritten word) or a
//! [`StrokeShape`] (the clean figure a drawn stroke snaps to). Renderers
//! turn raw samples plus correction into what is shown, the same way on
//! every device.
//!
//! No UI, no document model: points in, corrections out.

mod batch;
mod geom;
mod layout;
mod metrics;
mod shape;
mod smooth;
mod style;
#[cfg(test)]
mod testing;
mod word;

pub use batch::{Context, Outcome, beautify_all};
pub use geom::{Heads, ShapeGeom, StrokeShape};
pub use layout::{Row, group_lines, group_words, is_writing, joins_word, same_line};
pub use metrics::{Bounds, Line, WordMetrics, median};
pub use shape::{arrow_head, fit_shape, snap_ends};
pub use smooth::{smooth_points, smooth_pressure};
pub use style::StyleProfile;
pub use word::{Placement, Target, correct_word};

use wincode::{SchemaRead, SchemaWrite};

/// A point in world units.
#[derive(Clone, Copy, Debug, Default, PartialEq, SchemaWrite, SchemaRead)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub fn distance(self, other: Self) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    /// The point `t` of the way from `self` to `other`.
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self::new(
            (other.x - self.x).mul_add(t, self.x),
            (other.y - self.y).mul_add(t, self.y),
        )
    }
}

/// `x' = a·x + c·y + tx`, `y' = b·x + d·y + ty`.
#[derive(Clone, Copy, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    #[must_use]
    pub const fn translation(dx: f32, dy: f32) -> Self {
        Self {
            tx: dx,
            ty: dy,
            ..Self::IDENTITY
        }
    }

    #[must_use]
    pub fn apply(&self, p: Pt) -> Pt {
        Pt::new(
            self.a.mul_add(p.x, self.c.mul_add(p.y, self.tx)),
            self.b.mul_add(p.x, self.d.mul_add(p.y, self.ty)),
        )
    }

    /// `self` followed by a move of `(dx, dy)`.
    #[must_use]
    pub const fn translated(self, dx: f32, dy: f32) -> Self {
        Self {
            tx: self.tx + dx,
            ty: self.ty + dy,
            ..self
        }
    }

    /// `other ∘ self`: apply `self`, then `other`.
    #[must_use]
    pub fn then(&self, other: &Self) -> Self {
        Self {
            a: other.a.mul_add(self.a, other.c * self.b),
            b: other.b.mul_add(self.a, other.d * self.b),
            c: other.a.mul_add(self.c, other.c * self.d),
            d: other.b.mul_add(self.c, other.d * self.d),
            tx: other.a.mul_add(self.tx, other.c.mul_add(self.ty, other.tx)),
            ty: other.b.mul_add(self.tx, other.d.mul_add(self.ty, other.ty)),
        }
    }

    /// Component-wise blend, `t = 0` → `self`, `t = 1` → `other`. Good
    /// enough for animating the small corrections this crate makes.
    #[must_use]
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let mix = |from: f32, to: f32| (to - from).mul_add(t, from);
        Self {
            a: mix(self.a, other.a),
            b: mix(self.b, other.b),
            c: mix(self.c, other.c),
            d: mix(self.d, other.d),
            tx: mix(self.tx, other.tx),
            ty: mix(self.ty, other.ty),
        }
    }

    #[must_use]
    pub const fn is_finite(&self) -> bool {
        self.a.is_finite()
            && self.b.is_finite()
            && self.c.is_finite()
            && self.d.is_finite()
            && self.tx.is_finite()
            && self.ty.is_finite()
    }
}

/// Jitter smoothing applied to raw samples before the word affine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
pub enum Smooth {
    #[default]
    Off,
    Light,
    Normal,
}

/// How a handwritten stroke is shown: its raw points through this.
#[derive(Clone, Copy, Debug, Default, PartialEq, SchemaWrite, SchemaRead)]
pub struct StrokeFix {
    /// Word-level affine: rotate to the baseline, unslant, scale, move.
    pub affine: Affine,
    /// Smoothing applied to the raw points before `affine`.
    pub smooth: Smooth,
}

impl StrokeFix {
    /// Raw points as shown: smoothed, then through the affine.
    #[must_use]
    pub fn apply(&self, raw: &[Pt]) -> Vec<Pt> {
        smooth_points(raw, self.smooth)
            .into_iter()
            .map(|p| self.affine.apply(p))
            .collect()
    }

    /// The same fix with its result moved by `(dx, dy)`.
    #[must_use]
    pub const fn translated(self, dx: f32, dy: f32) -> Self {
        Self {
            affine: self.affine.translated(dx, dy),
            smooth: self.smooth,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn then_composes_in_order() {
        let scale = Affine {
            a: 2.0,
            d: 2.0,
            ..Affine::IDENTITY
        };
        let shift = Affine::translation(10.0, 0.0);
        let p = Pt::new(1.0, 1.0);
        assert_eq!(scale.then(&shift).apply(p), Pt::new(12.0, 2.0));
        assert_eq!(shift.then(&scale).apply(p), Pt::new(22.0, 2.0));
    }

    #[test]
    fn lerp_ends_at_both_affines() {
        let to = Affine::translation(4.0, -2.0);
        assert_eq!(Affine::IDENTITY.lerp(&to, 0.0), Affine::IDENTITY);
        assert_eq!(Affine::IDENTITY.lerp(&to, 1.0), to);
    }
}

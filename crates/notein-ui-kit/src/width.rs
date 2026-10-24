//! Width classes: the three layouts every screen adapts to.

use freya::prelude::*;

/// Layout class derived from the window's logical width.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WidthClass {
    /// Below 600 px: phones and narrow windows. Sheets come from the
    /// bottom, panels become sheets, panes become tabs.
    Compact,
    /// 600 to 1024 px: tablets in portrait, medium windows.
    Medium,
    /// 1024 px and wider: tablets in landscape and desktop.
    Expanded,
}

impl WidthClass {
    /// Upper bound (exclusive) of [`WidthClass::Compact`].
    pub const COMPACT_MAX: f32 = 600.0;
    /// Upper bound (exclusive) of [`WidthClass::Medium`].
    pub const MEDIUM_MAX: f32 = 1024.0;

    /// Class of a window `width` logical pixels wide.
    #[must_use]
    pub fn of(width: f32) -> Self {
        if width < Self::COMPACT_MAX {
            Self::Compact
        } else if width < Self::MEDIUM_MAX {
            Self::Medium
        } else {
            Self::Expanded
        }
    }

    #[must_use]
    pub const fn is_compact(self) -> bool {
        matches!(self, Self::Compact)
    }

    /// Side gutter for this class.
    #[must_use]
    pub const fn gutter(self) -> f32 {
        match self {
            Self::Compact => crate::tokens::space::GUTTER_COMPACT,
            Self::Medium | Self::Expanded => crate::tokens::space::GUTTER,
        }
    }
}

/// Current width class of the root window. Subscribes the caller to
/// window resizes.
#[must_use]
pub fn width_class() -> WidthClass {
    WidthClass::of(Platform::get().root_size.read().width)
}

#[cfg(test)]
mod tests {
    use super::WidthClass;

    #[test]
    fn boundaries() {
        assert_eq!(WidthClass::of(390.0), WidthClass::Compact);
        assert_eq!(WidthClass::of(600.0), WidthClass::Medium);
        assert_eq!(WidthClass::of(834.0), WidthClass::Medium);
        assert_eq!(WidthClass::of(1194.0), WidthClass::Expanded);
    }
}

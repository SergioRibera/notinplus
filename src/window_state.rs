//! Where the main window was when it closed: size, position, maximized
//! and fullscreen. The next launch opens it the same way.
//!
//! Kept out of [`crate::settings::Settings`]: it is read before the
//! interface starts, written from the close hook outside the settings
//! store, and a change to its layout must not reset the user's settings.
//!
//! A maximized, fullscreen or minimized window does not report the
//! bounds it returns to, so those keep the last normal bounds and only
//! flip the flags. A saved position off every connected monitor (the
//! monitor was unplugged) is dropped and the system places the window.

use freya::winit::dpi::{LogicalSize, PhysicalPosition};
use freya::winit::event_loop::ActiveEventLoop;
use freya::winit::window::{Fullscreen, Window, WindowAttributes};

use crate::persist::Persisted;

/// Smallest restored side, so a bogus record never opens a sliver.
const MIN_SIDE: u32 = 240;
/// How far into a monitor the title bar must start to count as visible.
const GRIP: i32 = 48;

/// The main window as it closed.
#[istmo::message]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowState {
    /// Inner size in logical pixels while neither maximized nor
    /// fullscreen.
    pub size: Option<(u32, u32)>,
    /// Outer top-left corner in physical pixels at that size; `None`
    /// where the platform hides it (Wayland).
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
    pub fullscreen: bool,
}

impl Persisted for WindowState {
    const FILE: &'static str = "window.bin";
}

impl WindowState {
    /// Record `window`, keeping the normal bounds of `self` when the
    /// window does not show them.
    #[must_use]
    pub fn capture(self, window: &Window) -> Self {
        let fullscreen = window.fullscreen().is_some();
        let maximized = window.is_maximized();
        let minimized = window.is_minimized().unwrap_or(false);
        let (size, position) = if fullscreen || maximized || minimized {
            (self.size, self.position)
        } else {
            let size: LogicalSize<f64> = window.inner_size().to_logical(window.scale_factor());
            (
                Some((round_side(size.width), round_side(size.height))),
                window.outer_position().ok().map(|p| (p.x, p.y)),
            )
        };
        Self {
            size,
            position,
            maximized,
            fullscreen,
        }
    }

    /// Open the window the way it was recorded.
    #[must_use]
    pub fn apply(
        self,
        mut attributes: WindowAttributes,
        event_loop: &ActiveEventLoop,
    ) -> WindowAttributes {
        if let Some((width, height)) = self.size {
            attributes = attributes
                .with_inner_size(LogicalSize::new(width.max(MIN_SIDE), height.max(MIN_SIDE)));
        }
        if let Some((x, y)) = self.position
            && is_visible(x, y, event_loop)
        {
            attributes = attributes.with_position(PhysicalPosition::new(x, y));
        }
        if self.fullscreen {
            attributes = attributes.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        attributes.with_maximized(self.maximized)
    }
}

/// Whether the start of a title bar at `(x, y)` lies on a connected
/// monitor.
fn is_visible(x: i32, y: i32, event_loop: &ActiveEventLoop) -> bool {
    let (x, y) = (
        i64::from(x) + i64::from(GRIP),
        i64::from(y) + i64::from(GRIP / 3),
    );
    event_loop.available_monitors().any(|monitor| {
        let origin = monitor.position();
        let size = monitor.size();
        let (left, top) = (i64::from(origin.x), i64::from(origin.y));
        (left..left + i64::from(size.width)).contains(&x)
            && (top..top + i64::from(size.height)).contains(&y)
    })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to a window side first"
)]
fn round_side(side: f64) -> u32 {
    side.round().clamp(f64::from(MIN_SIDE), f64::from(u16::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_disk_encoding() {
        let state = WindowState {
            size: Some((1280, 800)),
            position: Some((-1900, 40)),
            maximized: true,
            fullscreen: false,
        };
        let bytes = notin_plugin::encode(&state).expect("encode");
        assert_eq!(
            notin_plugin::decode::<WindowState>(&bytes).expect("decode"),
            state
        );
    }

    #[test]
    fn sides_are_clamped() {
        assert_eq!(round_side(10.0), MIN_SIDE);
        assert_eq!(round_side(1279.6), 1280);
        assert_eq!(round_side(f64::MAX), u32::from(u16::MAX));
    }
}

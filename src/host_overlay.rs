//! Freya overlay for the shell-side progress bar + toast stack.
//!
//! Mount [`HostOverlay`] once at the root of the app tree. It polls
//! [`crate::host_api::host`] via a background timer, bumping an
//! internal state counter when the version advances. Progress bars
//! render across the top as a thin separator-style line; toasts stack
//! bottom-right with colour tints per [`NotificationKind`].

use std::time::Duration;

use freya::prelude::*;
use notein_plugin::NotificationKind;

use crate::host_api::{self, HostInner, Notification, ProgressEntry};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const BAR_HEIGHT_PX: f32 = 3.0;
const TOAST_WIDTH_PX: f32 = 320.0;

#[derive(Debug, PartialEq)]
pub struct HostOverlay;

impl Component for HostOverlay {
    fn render(&self) -> impl IntoElement {
        let host = host_api::host();
        // Local tick drives unconditional re-render every `POLL_INTERVAL`
        // regardless of host version changes — needed so stale `Finished`
        // progress entries and auto-dismiss notifications actually fall
        // out of the overlay without a user interaction forcing a repaint.
        let mut tick: State<u64> = use_state(|| 0);

        use_hook({
            move || {
                spawn(async move {
                    loop {
                        async_io::Timer::after(POLL_INTERVAL).await;
                        let next = tick.peek().wrapping_add(1);
                        tick.set(next);
                    }
                });
            }
        });

        let _ = *tick.read();
        let bars = host.snapshot_progress();
        let toasts = host.snapshot_notifications();

        rect()
            .position(Position::new_absolute().top(0.).left(0.).right(0.).bottom(0.))
            .width(Size::fill())
            .height(Size::fill())
            .child(progress_bar_stack(&bars))
            .child(toast_stack(&toasts, host))
    }
}

fn progress_bar_stack(bars: &[ProgressEntry]) -> impl IntoElement {
    let mut root = rect()
        .position(Position::new_absolute().top(0.).left(0.).right(0.))
        .width(Size::fill())
        .spacing(1.0);
    for entry in bars {
        root = root.child(progress_bar(entry));
    }
    root
}

fn progress_bar(entry: &ProgressEntry) -> impl IntoElement {
    let (color, fraction) = match &entry.finished {
        Some(f) if f.success => (Color::from_rgb(90, 170, 110), 1.0),
        Some(_) => (Color::from_rgb(190, 90, 90), 1.0),
        None => (
            Color::from_rgb(90, 140, 220),
            entry
                .total
                .filter(|t| *t > 0)
                .map(|t| (entry.done as f32 / t as f32).clamp(0.0, 1.0))
                .unwrap_or(0.4),
        ),
    };
    rect()
        .width(Size::fill())
        .height(Size::px(BAR_HEIGHT_PX))
        .background(Color::from_rgb(28, 28, 32))
        .child(
            rect()
                .width(Size::percent(fraction * 100.0))
                .height(Size::px(BAR_HEIGHT_PX))
                .background(color),
        )
}

fn toast_stack(toasts: &[Notification], host: std::sync::Arc<HostInner>) -> impl IntoElement {
    let mut root = rect()
        .position(Position::new_absolute().bottom(16.).right(16.))
        .width(Size::px(TOAST_WIDTH_PX))
        .spacing(8.0);
    for n in toasts {
        root = root.child(toast(n.clone(), host.clone()));
    }
    root
}

fn toast(n: Notification, host: std::sync::Arc<HostInner>) -> impl IntoElement {
    let (bg, fg) = kind_colors(n.kind);
    let id = n.id;
    rect()
        .width(Size::fill())
        .padding((10., 14.))
        .with_corner_radius(10.0)
        .background(bg)
        .spacing(4.0)
        .on_press(move |_: Event<PressEventData>| host.dismiss_notification(id))
        .child(
            label()
                .font_size(11.0)
                .color(Color::from_rgb(230, 230, 230))
                .text(n.plugin_id.clone()),
        )
        .child(
            label()
                .font_size(13.5)
                .color(fg)
                .text(n.message.clone()),
        )
}

const fn kind_colors(kind: NotificationKind) -> (Color, Color) {
    match kind {
        NotificationKind::Info => (
            Color::from_rgb(32, 48, 70),
            Color::from_rgb(220, 220, 220),
        ),
        NotificationKind::Success => (
            Color::from_rgb(32, 70, 48),
            Color::from_rgb(230, 240, 230),
        ),
        NotificationKind::Warning => (
            Color::from_rgb(70, 60, 32),
            Color::from_rgb(240, 230, 200),
        ),
        NotificationKind::Error => (
            Color::from_rgb(70, 32, 36),
            Color::from_rgb(240, 220, 220),
        ),
    }
}

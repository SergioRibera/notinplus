//! Freya overlay for the shell-side progress bar + toast stack.
//!
//! Mount [`HostOverlay`] once at the root of the app tree. It polls
//! [`crate::host_api::host`] via a background timer, bumping an
//! internal state counter when the version advances. Progress bars
//! render across the top as a thin separator-style line; toasts stack
//! bottom-right with colour tints per [`NotificationKind`].

use std::time::Duration;

use freya::prelude::*;
use notein_plugin::{BookmarkDraft, BookmarkEditRequest, NotificationKind, Rgba, SourceRef};

use crate::components::BookmarkBodyEditor;
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
        let pending_edit = host.snapshot_bookmark_edit();

        let mut root = rect()
            .position(Position::new_absolute().top(0.).left(0.).right(0.).bottom(0.))
            .width(Size::fill())
            .height(Size::fill())
            .child(progress_bar_stack(&bars))
            .child(toast_stack(&toasts, host.clone()));
        if let Some((id, _plugin, request)) = pending_edit {
            root = root.child(BookmarkEditModal {
                id,
                request,
                host: host.clone(),
            });
        }
        root
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

/// Modal the shell renders when any plugin pushes a bookmark edit
/// request through [`host_api::HostInner::request_bookmark_edit`].
/// Hosts the FormInput body field + the ColorWheel + the Save /
/// Cancel buttons; result flows back to the plugin via
/// [`host_api::HostInner::resolve_bookmark_edit`].
///
/// Wrapped as a `Component` so the `use_state` hooks inside its render
/// have a proper scope context (plain fn calls would panic at
/// `CurrentContext::with`).
#[derive(Clone)]
pub struct BookmarkEditModal {
    pub id: u64,
    pub request: BookmarkEditRequest,
    pub host: std::sync::Arc<HostInner>,
}

impl PartialEq for BookmarkEditModal {
    fn eq(&self, other: &Self) -> bool {
        // Only the `id` matters for Freya's diff: the shell drops +
        // re-mounts whenever a new request supersedes the current.
        self.id == other.id
    }
}

impl std::fmt::Debug for BookmarkEditModal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BookmarkEditModal")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Component for BookmarkEditModal {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        let seed_body = self.request.draft.body.clone();
        let seed_color = self.request.draft.color;
        let seed_refs = self.request.draft.refs.clone();
        let title = self.request.title.clone();
        let anchor = self.request.anchor;
        let host = self.host.clone();
        let preview_sink = self.request.on_preview.clone();

        let body: State<String> = use_state(move || seed_body.clone());
        let color: State<Option<Rgba>> = use_state(move || seed_color);
        let refs: State<Vec<SourceRef>> = use_state(move || seed_refs.clone());

        let initial_wheel_color = seed_color
            .map(|c| Color::from_rgb(c[0], c[1], c[2]))
            .unwrap_or_else(|| Color::from_rgb(255, 225, 110));

        // Emit the initial preview exactly once on mount so the
        // plugin's render reflects the seeded colour before the user
        // touches the wheel. Guarded by `use_hook` so re-renders
        // don't keep firing.
        {
            let preview_sink = preview_sink.clone();
            let body = body;
            let color = color;
            let refs = refs;
            use_hook(move || {
                if let Some(sink) = preview_sink.as_ref() {
                    sink(BookmarkDraft {
                        body: body.read().clone(),
                        color: *color.read(),
                        refs: refs.read().clone(),
                    });
                }
            });
        }

        let fire_preview = {
            let preview_sink = preview_sink.clone();
            let body = body;
            let color = color;
            let refs = refs;
            std::sync::Arc::new(move || {
                if let Some(sink) = preview_sink.as_ref() {
                    sink(BookmarkDraft {
                        body: body.read().clone(),
                        color: *color.read(),
                        refs: refs.read().clone(),
                    });
                }
            })
        };

        let on_color_change = {
            let mut color = color;
            let fire_preview = fire_preview.clone();
            move |c: Color| {
                color.set(Some([c.r(), c.g(), c.b(), c.a()]));
                fire_preview();
            }
        };

        let on_save = {
            let host = host.clone();
            move |_: Event<PressEventData>| {
                let draft = BookmarkDraft {
                    body: body.read().clone(),
                    color: *color.read(),
                    refs: refs.read().clone(),
                };
                host.resolve_bookmark_edit(id, Some(draft));
            }
        };
        let on_cancel = {
            let host = host.clone();
            move |_: Event<PressEventData>| {
                host.resolve_bookmark_edit(id, None);
            }
        };

        let (left, top) = anchor
            .map(|(x, y)| (x.max(16.0), (y + 12.0).max(16.0)))
            .unwrap_or((f32::NAN, 72.0));

        let card = rect()
            .layer(Layer::OverlayLevel(6))
            .width(Size::px(320.0))
            .padding((12., 14.))
            .with_corner_radius(12.0)
            .background(Color::from_rgb(28, 28, 32))
            .spacing(10.0);

        let card = if left.is_nan() {
            card.position(Position::new_global().top(top).right(16.))
        } else {
            card.position(Position::new_global().top(top).left(left))
        };

        let refs_snapshot = refs.read().clone();
        card.child(
            label()
                .font_size(13.0)
                .color(Color::from_rgb(220, 220, 220))
                .text(title.into_owned()),
        )
        .child(
            // Reuse notein's own rich body editor — chip refs, `[[`
            // autocomplete popup, atomic chip delete all behave
            // exactly like they do in the canvas / PDF bookmark card.
            BookmarkBodyEditor::new(Writable::<String>::from(body))
                .placeholder("Nota (opcional)")
                .completions(crate::bookmark_ui::plugin_completions_provider()),
        )
        .child(crate::bookmark_ui::ref_chip_row(&refs_snapshot))
        .child(
            notein_ui_kit::ColorWheel::new()
                .initial(initial_wheel_color)
                .allow_custom(true)
                .on_change(on_color_change),
        )
        .child(
            rect()
                .direction(Direction::Horizontal)
                .spacing(8.0)
                .child(
                    rect()
                        .padding((6., 12.))
                        .with_corner_radius(6.0)
                        .background(Color::from_rgb(55, 60, 70))
                        .on_press(on_cancel)
                        .child(
                            label()
                                .font_size(12.0)
                                .color(Color::from_rgb(230, 230, 230))
                                .text("Cancelar"),
                        ),
                )
                .child(
                    rect()
                        .padding((6., 12.))
                        .with_corner_radius(6.0)
                        .background(Color::from_rgb(90, 110, 60))
                        .on_press(on_save)
                        .child(
                            label()
                                .font_size(12.0)
                                .color(Color::from_rgb(240, 240, 240))
                                .text("Guardar"),
                        ),
                ),
        )
    }
}

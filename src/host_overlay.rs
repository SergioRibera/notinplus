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

use crate::components::NoteEditor;
use crate::components::color_wheel::{ColorWheel, DEFAULT_SWATCHES, color_swatch_strip};
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

        let toast_mask = crate::ui_mask::use_mask_slot();
        let _ = *tick.read();
        let bars = host.snapshot_progress();
        let toasts = host.snapshot_notifications();
        if toasts.is_empty() {
            toast_mask.clear();
        }
        let pending_edit = host.snapshot_bookmark_edit();

        let mut root = rect()
            .position(
                Position::new_absolute()
                    .top(0.)
                    .left(0.)
                    .right(0.)
                    .bottom(0.),
            )
            .width(Size::fill())
            .height(Size::fill())
            .child(progress_bar_stack(&bars))
            .child(toast_stack(&toasts, host.clone(), toast_mask));
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

/// Toasts in the bottom-right corner. `mask` keeps a pen tap on them off
/// the canvas below.
fn toast_stack(
    toasts: &[Notification],
    host: std::sync::Arc<HostInner>,
    mask: crate::ui_mask::MaskSlot,
) -> impl IntoElement {
    let mut root = rect()
        .position(Position::new_absolute().bottom(16.).right(16.))
        .width(Size::px(TOAST_WIDTH_PX))
        .spacing(8.0)
        .on_sized(move |e: Event<SizedEventData>| mask.publish(e.area));
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
        .child(label().font_size(13.5).color(fg).text(n.message.clone()))
}

const fn kind_colors(kind: NotificationKind) -> (Color, Color) {
    match kind {
        NotificationKind::Info => (Color::from_rgb(32, 48, 70), Color::from_rgb(220, 220, 220)),
        NotificationKind::Success => (Color::from_rgb(32, 70, 48), Color::from_rgb(230, 240, 230)),
        NotificationKind::Warning => (Color::from_rgb(70, 60, 32), Color::from_rgb(240, 230, 200)),
        NotificationKind::Error => (Color::from_rgb(70, 32, 36), Color::from_rgb(240, 220, 220)),
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

        let mask = crate::ui_mask::use_mask_slot();
        let body: State<String> = use_state(move || seed_body.clone());
        let color: State<Option<Rgba>> = use_state(move || seed_color);
        let refs: State<Vec<SourceRef>> = use_state(move || seed_refs.clone());

        // Install the chip-tap close hook — in-app chip refs
        // (Bible / Canvas / Plugin) hand off to another route, so
        // this modal needs to tear down before the destination
        // renders. `resolve_bookmark_edit(id, None)` fires the
        // original `on_done` callback with `None`, matching the
        // Cancel path, so the plugin side cleans up correctly.
        {
            let host = host.clone();
            let on_chip_tap_close: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(move || {
                host.resolve_bookmark_edit(id, None);
            });
            freya::prelude::provide_context(crate::components::ChipTapCloseCallback(
                on_chip_tap_close,
            ));
        }
        // Full HSVA panel lives behind a trigger now; the inline card
        // only surfaces the preset swatches + a rainbow slot that pops
        // the panel open. Keeps the modal compact when the user is
        // happy with one of the defaults.
        let mut color_picker_open: State<bool> = use_state(|| false);
        // Last custom-wheel colour. Painted on the special slot so a
        // reopen remembers the user's non-preset pick after they pick a
        // preset in between.
        let mut color_special: State<Option<Color>> = use_state(move || {
            seed_color
                .map(|c| Color::from_rgb(c[0], c[1], c[2]))
                .filter(|c| !DEFAULT_SWATCHES.iter().any(|p| p == c))
        });

        let initial_wheel_color = seed_color
            .map(|c| Color::from_rgb(c[0], c[1], c[2]))
            .unwrap_or_else(|| Color::from_rgb(0x6F, 0x63, 0x31));

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

        // Preset swatch commit: closes the picker and clears the custom
        // slot so the ring sits on the chosen preset.
        let on_preset_change = {
            let mut color = color;
            let fire_preview = fire_preview.clone();
            move |c: Color| {
                color.set(Some([c.r(), c.g(), c.b(), c.a()]));
                color_special.set(None);
                color_picker_open.set(false);
                fire_preview();
            }
        };
        // Rainbow-slot press: toggles the floating HSVA picker.
        let on_special_press = move |_: Event<PressEventData>| {
            let cur = *color_picker_open.read();
            color_picker_open.set(!cur);
        };
        // Picker commits mirror back into the special slot so the slot
        // stays painted with the user's latest custom pick.
        let on_wheel_change = {
            let mut color = color;
            let fire_preview = fire_preview.clone();
            move |c: Color| {
                color.set(Some([c.r(), c.g(), c.b(), c.a()]));
                color_special.set(Some(c));
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

        // Capture the card's own measured area so the clamp can flip
        // the modal upward when the anchor sits near the bottom of
        // the viewport. First frame hides it (opacity 0) until the
        // on_sized callback publishes a real height — otherwise the
        // user would see a one-frame flash at the clipped position.
        let mut card_area: State<Area> = use_state(Area::default);
        let viewport = freya::prelude::try_consume_context::<notein_ui_kit::ViewportArea>();

        let (anchor_left, anchor_top_below) = anchor
            .map(|(x, y)| (Some(x), y + 12.0))
            .unwrap_or((None, 72.0));

        let measured = *card_area.read();
        let is_measured = measured.height() > 0.0;
        let card = rect()
            .layer(Layer::OverlayLevel(6))
            .width(Size::auto())
            .max_width(Size::px(342.))
            .padding((12., 14.))
            .with_corner_radius(12.0)
            .background(Color::from_rgb(28, 28, 32))
            .spacing(10.0)
            .on_sized(move |e: Event<SizedEventData>| {
                let new_area = e.data().area;
                mask.publish(new_area);
                if *card_area.peek() != new_area {
                    card_area.set(new_area);
                }
            })
            .opacity(if is_measured || anchor_left.is_none() {
                1.0
            } else {
                0.0
            });

        let card = match (anchor_left, viewport, is_measured) {
            (None, _, _) => card.position(Position::new_global().top(anchor_top_below).right(16.)),
            (Some(x), Some(viewport_ctx), true) => {
                let viewport_area = viewport_ctx.get();
                if viewport_area.width() <= 0.0 {
                    card.position(
                        Position::new_global()
                            .top(anchor_top_below.max(16.0))
                            .left(x.max(16.0)),
                    )
                } else {
                    let top = notein_ui_kit::clamp_popup_top_from_point(
                        viewport_area,
                        anchor_top_below - 12.0,
                        anchor_top_below,
                        measured.height(),
                        16.0,
                    );
                    let left =
                        notein_ui_kit::clamp_popup_left(viewport_area, x, measured.width(), 16.0);
                    card.position(Position::new_global().top(top).left(left))
                }
            }
            (Some(x), _, _) => card.position(
                Position::new_global()
                    .top(anchor_top_below.max(16.0))
                    .left(x.max(16.0)),
            ),
        };

        let refs_snapshot = refs.read().clone();
        let current_color = (*color.read())
            .map(|c| Color::from_rgb(c[0], c[1], c[2]))
            .unwrap_or(initial_wheel_color);
        let stored_special = *color_special.read();
        let matches_preset = DEFAULT_SWATCHES.iter().any(|c| *c == current_color);
        let display_special = stored_special.or_else(|| (!matches_preset).then_some(current_color));
        let selected_is_special = !matches_preset;
        let selected_preset = matches_preset.then_some(current_color);
        let picker_open = *color_picker_open.read();
        // Wrap the wheel so a `pointer_leave` from the floating panel
        // auto-dismisses it — matches the implicit tooltip behaviour
        // the Bible reader's ref popup uses.
        let picker = picker_open.then(|| {
            let wheel = ColorWheel::new()
                .initial(display_special.unwrap_or(initial_wheel_color))
                .swatches(Vec::<Color>::new())
                .allow_custom(true)
                .width(240.0)
                .on_change(on_wheel_change);
            rect()
                .on_pointer_leave(move |_| {
                    color_picker_open.set(false);
                })
                .child(wheel)
        });

        card.child(
            label()
                .font_size(13.0)
                .color(Color::from_rgb(220, 220, 220))
                .text(title.into_owned()),
        )
        .child(
            NoteEditor::new(Writable::<String>::from(body))
                .placeholder("Nota (opcional)")
                .completions(crate::bookmark_ui::plugin_completions_provider()),
        )
        .child(crate::bookmark_ui::ref_chip_row(&refs_snapshot))
        .child(color_swatch_strip(
            selected_preset,
            display_special,
            selected_is_special,
            picker_open,
            on_preset_change,
            on_special_press,
            picker,
        ))
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

//! Touch-aware drag-and-drop primitives.
//!
//! Vendored from `freya-components` (commit ba7b80b) because the
//! upstream `DragZone::on_pointer_down` guard rejects every event
//! whose `.button()` is not `MouseButton::Left`, which is always the
//! case for touch events — a finger-dragged card never leaves the
//! `Pressing` phase. Same deal in `DropZone`, where the drop fires
//! only on `on_mouse_up`; touch never triggers it.
//!
//! Two surgical changes against the upstream source:
//!
//! - `DragZone`: `button() == Some(Left)` → `is_primary()` so both
//!   left-mouse and any touch start a press.
//! - `DropZone`: pair `on_mouse_up` with `on_touch_end` so a finger
//!   lifted over the target also fires the drop.
//!
//! Everything else (API, drag ghost, phases) matches the upstream so
//! callers swap imports without rewiring.

use freya::prelude::*;

#[derive(Clone, Copy)]
enum DragPhase {
    Idle,
    Pressing {
        press_point: CursorPoint,
        offset: CursorPoint,
    },
    Dragging {
        position: CursorPoint,
        offset: CursorPoint,
    },
}

/// Access the global drag state for payloads of type `T`.
pub fn use_drag<T: 'static>() -> State<Option<T>> {
    match try_consume_root_context() {
        Some(s) => s,
        None => {
            let state = State::<Option<T>>::create_in_scope(None, ScopeId::ROOT);
            provide_context_for_scope_id(state, ScopeId::ROOT);
            state
        }
    }
}

/// Draggable card. Mirrors freya's `DragZone` but accepts touch input.
#[derive(Clone, PartialEq)]
pub struct DragZone<T: Clone + 'static + PartialEq> {
    drag_element: Option<Element>,
    children: Element,
    data: T,
    show_while_dragging: bool,
    drag_threshold: f64,
    enabled: bool,
    key: DiffKey,
}

impl<T: Clone + PartialEq + 'static> KeyExt for DragZone<T> {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl<T: Clone + PartialEq + 'static> DragZone<T> {
    pub fn new(data: T, children: impl Into<Element>) -> Self {
        Self {
            data,
            children: children.into(),
            drag_element: None,
            show_while_dragging: true,
            drag_threshold: 4.0,
            enabled: true,
            key: DiffKey::default(),
        }
    }

    pub fn show_while_dragging(mut self, show_while_dragging: bool) -> Self {
        self.show_while_dragging = show_while_dragging;
        self
    }

    pub fn drag_element(mut self, drag_element: impl Into<Element>) -> Self {
        self.drag_element = Some(drag_element.into());
        self
    }

    pub fn drag_threshold(mut self, drag_threshold: f64) -> Self {
        self.drag_threshold = drag_threshold;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

impl<T: Clone + PartialEq> Component for DragZone<T> {
    fn render(&self) -> impl IntoElement {
        let mut drags = use_drag::<T>();
        let mut phase = use_state(|| DragPhase::Idle);
        let mut drag_element_size = use_state(|| None::<Size2D>);
        let data = self.data.clone();
        let drag_threshold = self.drag_threshold;

        let on_global_pointer_move = move |e: Event<PointerEventData>| match phase() {
            DragPhase::Dragging { offset, .. } => {
                phase.set(DragPhase::Dragging {
                    position: e.global_location(),
                    offset,
                });
            }
            DragPhase::Pressing {
                press_point,
                offset,
            } => {
                let current = e.global_location();
                let dx = current.x - press_point.x;
                let dy = current.y - press_point.y;

                if (dx * dx + dy * dy).sqrt() >= drag_threshold {
                    phase.set(DragPhase::Dragging {
                        position: current,
                        offset,
                    });
                    *drags.write() = Some(data.clone());
                }
            }
            DragPhase::Idle => {}
        };

        let on_pointer_down = move |e: Event<PointerEventData>| {
            // Upstream filtered on `button() == Some(Left)` which drops
            // every touch (button is always `None` for touch). Use
            // `is_primary()` so touch + left-click both start a press.
            if !e.data().is_primary() {
                return;
            }
            phase.set(DragPhase::Pressing {
                press_point: e.global_location(),
                offset: e.element_location(),
            });
        };

        let on_global_pointer_press = move |_: Event<PointerEventData>| {
            if !matches!(phase(), DragPhase::Idle) {
                phase.set(DragPhase::Idle);
                *drags.write() = None;
            }
        };

        let dragging = match phase() {
            DragPhase::Dragging { position, offset } => Some((position, offset)),
            _ => None,
        };

        rect()
            .on_global_pointer_press(on_global_pointer_press)
            .on_global_pointer_move(on_global_pointer_move)
            .maybe(self.enabled, |rect| rect.on_pointer_down(on_pointer_down))
            .maybe_child((dragging.zip(self.drag_element.clone())).map(
                |((position, offset), drag_element)| {
                    let size = *drag_element_size.read();
                    let anchor = size.map_or(offset, |size| {
                        offset.min(CursorPoint::new(size.width as f64, size.height as f64))
                    });
                    let (x, y) = (position - anchor).to_f32().to_tuple();
                    rect()
                        .position(Position::new_global())
                        .layer(Layer::Overlay)
                        .interactive(false)
                        .opacity(if size.is_some() { 1. } else { 0. })
                        .offset_x(x + 1.)
                        .offset_y(y + 1.)
                        .on_sized(move |e: Event<SizedEventData>| {
                            drag_element_size.set_if_modified(Some(e.area.size))
                        })
                        .child(drag_element)
                },
            ))
            .maybe_child(
                (self.show_while_dragging || dragging.is_none()).then(|| self.children.clone()),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// Drop target paired with a [`DragZone`]. Accepts both mouse-up and
/// touch-end releases over the target element.
#[derive(PartialEq, Clone)]
pub struct DropZone<T: 'static + PartialEq + Clone> {
    children: Element,
    on_drop: EventHandler<T>,
    on_drag_over: Option<EventHandler<bool>>,
    width: Size,
    height: Size,
    key: DiffKey,
}

impl<T: Clone + PartialEq + 'static> KeyExt for DropZone<T> {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl<T: PartialEq + Clone + 'static> DropZone<T> {
    pub fn new(children: impl Into<Element>, on_drop: impl Into<EventHandler<T>>) -> Self {
        Self {
            children: children.into(),
            on_drop: on_drop.into(),
            on_drag_over: None,
            width: Size::auto(),
            height: Size::auto(),
            key: DiffKey::default(),
        }
    }

    pub fn on_drag_over(mut self, on_drag_over: impl Into<EventHandler<bool>>) -> Self {
        self.on_drag_over = Some(on_drag_over.into());
        self
    }
}

impl<T: Clone + PartialEq + 'static> Component for DropZone<T> {
    fn render(&self) -> impl IntoElement {
        let mut drags = use_drag::<T>();
        let on_drop = self.on_drop.clone();
        let on_drag_over = self.on_drag_over.clone();

        // Shared commit path for mouse-up and touch-end. Returns true if
        // a drop was dispatched so the caller can `stop_propagation`
        // uniformly across both event families.
        let commit = {
            let on_drop = on_drop.clone();
            let on_drag_over = on_drag_over.clone();
            move || {
                if let Some(payload) = &*drags.read() {
                    on_drop.call(payload.clone());
                }
                if drags.read().is_some() {
                    *drags.write() = None;
                    if let Some(cb) = &on_drag_over {
                        cb.call(false);
                    }
                }
            }
        };

        let mut mouse_commit = commit.clone();
        let on_mouse_up = move |e: Event<MouseEventData>| {
            e.stop_propagation();
            mouse_commit();
        };

        let mut touch_commit = commit;
        let on_touch_end = move |e: Event<TouchEventData>| {
            e.stop_propagation();
            touch_commit();
        };

        rect()
            .on_mouse_up(on_mouse_up)
            .on_touch_end(on_touch_end)
            .width(self.width.clone())
            .height(self.height.clone())
            .map(on_drag_over, move |el, on_drag_over| {
                el.on_pointer_enter({
                    let on_drag_over = on_drag_over.clone();
                    move |_| {
                        if drags.read().is_some() {
                            on_drag_over.call(true);
                        }
                    }
                })
                .on_pointer_leave(move |_| {
                    if drags.read().is_some() {
                        on_drag_over.call(false);
                    }
                })
            })
            .child(self.children.clone())
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

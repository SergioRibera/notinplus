//! Menus and pickers anchored to a button, shared by the toolbar and the
//! block headers, and telling the host where they open.
//!
//! Hosts that route input outside freya, like a canvas taking stylus input
//! from a native backend, need to know which parts of the window a popup
//! covers so a tap on it does not reach what lies below.

use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

use freya::prelude::*;

use crate::theme::EditorColors;

/// Space between a button and its popup.
const POPUP_GAP: f32 = 4.0;
/// Space kept between a popup and the window edges.
pub(crate) const POPUP_MARGIN: f32 = 8.0;
/// Height of a menu row: a comfortable touch target that still reads as a
/// list with a mouse.
pub(crate) const MENU_ROW: f32 = 40.0;
/// Travel that turns a touch in a popup into a scroll.
const SLOP: f64 = 8.0;

thread_local! {
    /// Where the last finger that scrolled a popup lifted, so the row under
    /// it ignores that lift. The rows are built by whoever owns the popup,
    /// outside its scope, hence no context.
    static LIFTED_AFTER_SCROLL: Cell<Option<CursorPoint>> = const { Cell::new(None) };
}

/// Whether `press` is the lift of a finger that scrolled a popup.
fn ends_a_scroll(press: &PressEventData) -> bool {
    let PressEventData::Touch(touch) = press else {
        return false;
    };
    let at = touch.global_location;
    LIFTED_AFTER_SCROLL
        .get()
        .is_some_and(|lift| (at.x - lift.x).abs() < 1.0 && (at.y - lift.y).abs() < 1.0)
}

/// Called with the window area of a popup the editor or its toolbar
/// opens, and with `None` once it closes. Provide it as context above the
/// editor and the toolbar.
#[derive(Clone)]
pub struct PopupReporter(Rc<dyn Fn(Option<Area>)>);

impl PopupReporter {
    #[must_use]
    pub fn new(report: impl Fn(Option<Area>) + 'static) -> Self {
        Self(Rc::new(report))
    }
}

impl PartialEq for PopupReporter {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for PopupReporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PopupReporter").finish_non_exhaustive()
    }
}

/// Popup content that keeps the [`PopupReporter`] in context, if any,
/// informed of its area while mounted.
#[derive(Clone, PartialEq)]
pub(crate) struct ReportedPopup {
    pub(crate) child: Element,
}

impl Component for ReportedPopup {
    #[cfg_attr(feature = "sismo", sismo::function("markdown::reported_popup"))]
    fn render(&self) -> impl IntoElement {
        let reporter = use_try_consume::<PopupReporter>();
        use_drop({
            let reporter = reporter.clone();
            move || {
                #[cfg(feature = "sismo")]
                sismo::scope!("markdown::reported_popup_drop");
                if let Some(reporter) = reporter {
                    (reporter.0)(None);
                }
            }
        });
        rect()
            .on_sized(move |event: Event<SizedEventData>| {
                if let Some(reporter) = &reporter {
                    (reporter.0)(Some(event.area));
                }
            })
            .child(self.child.clone())
    }
}

/// Region menus, pickers and tooltips must stay inside, provided by the
/// toolbar to its buttons. `None` means the whole window.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct PopupArea(pub(crate) Option<Area>);

/// Area popups may use: the closest [`PopupArea`], else the window.
#[cfg_attr(feature = "sismo", sismo::function("markdown::popup_bounds"))]
pub(crate) fn popup_bounds() -> Area {
    try_consume_context::<PopupArea>()
        .and_then(|area| area.0)
        .unwrap_or_else(|| {
            let window = *Platform::get().current_window().root_size.read();
            Area::new((0., 0.).into(), (window.width, window.height).into())
        })
}

/// Child with an optional popup that opens right under it, or right above
/// it when there is no room below, and never leaves the window: the anchor
/// can sit anywhere (top of a page, docked above a soft keyboard, scrolled
/// sideways).
///
/// With `on_dismiss`, a press outside both the child and the popup, or
/// Escape, asks to close it.
#[derive(Clone, PartialEq)]
pub(crate) struct Anchored {
    pub(crate) child: Element,
    pub(crate) popup: Option<Element>,
    pub(crate) on_dismiss: Option<EventHandler<()>>,
}

impl Anchored {
    pub(crate) const fn new(child: Element, popup: Option<Element>) -> Self {
        Self {
            child,
            popup,
            on_dismiss: None,
        }
    }

    pub(crate) fn on_dismiss(mut self, on_dismiss: impl FnMut() + 'static) -> Self {
        let mut on_dismiss = on_dismiss;
        self.on_dismiss = Some(EventHandler::new(move |()| on_dismiss()));
        self
    }
}

impl Component for Anchored {
    #[cfg_attr(feature = "sismo", sismo::function("markdown::anchored"))]
    fn render(&self) -> impl IntoElement {
        let mut anchor = use_state(Area::default);
        let mut measured = use_state(Area::default);
        // Where a finger went down in the popup and whether it travelled.
        let touch = use_hook(|| Rc::new(Cell::new(None::<(CursorPoint, bool)>)));
        let bounds = popup_bounds();
        let dismiss = self.on_dismiss.clone().filter(|_| self.popup.is_some());
        let popup = self.popup.clone().map(|popup| {
            let size = *measured.read();
            let (top, left) = popup_origin(*anchor.read(), (size.width(), size.height()), bounds);
            let outside = dismiss.clone();
            let escape = dismiss.clone();
            rect()
                .layer(Layer::Overlay)
                .position(Position::new_global().top(top).left(left))
                // Hidden until measured, so it never flashes off screen.
                .opacity(if size.height() > 0. { 1. } else { 0. })
                .on_sized(move |event: Event<SizedEventData>| {
                    measured.set_if_modified(event.area);
                })
                .on_global_pointer_up(move |event: Event<PointerEventData>| {
                    let Some(dismiss) = &outside else {
                        return;
                    };
                    let point = event.global_location();
                    let inside = |area: Area| area.contains(point.to_f32());
                    if !inside(*measured.peek()) && !inside(*anchor.peek()) {
                        dismiss.call(());
                    }
                })
                .on_global_key_down(move |event: Event<KeyboardEventData>| {
                    if event.key == Key::Named(NamedKey::Escape)
                        && let Some(dismiss) = &escape
                    {
                        dismiss.call(());
                    }
                })
                // The popup sits inside what it is anchored to: a touch or a
                // scroll in it must not also edit or scroll the note below.
                .on_pointer_down({
                    let touch = Rc::clone(&touch);
                    move |event: Event<PointerEventData>| {
                        event.stop_propagation();
                        if matches!(event.data(), PointerEventData::Touch(_)) {
                            touch.set(Some((event.global_location(), false)));
                        }
                    }
                })
                .on_capture_global_pointer_move({
                    let touch = Rc::clone(&touch);
                    move |event: Event<PointerEventData>| {
                        if let Some((origin, travelled)) = touch.get() {
                            let at = event.global_location();
                            let far = (at.x - origin.x).hypot(at.y - origin.y) >= SLOP;
                            touch.set(Some((origin, travelled || far)));
                        }
                    }
                })
                .on_capture_global_pointer_up({
                    let touch = Rc::clone(&touch);
                    move |event: Event<PointerEventData>| {
                        if let Some((_, true)) = touch.take() {
                            LIFTED_AFTER_SCROLL.set(Some(event.global_location()));
                        }
                    }
                })
                .on_pointer_press(|event: Event<PointerEventData>| event.stop_propagation())
                .on_touch_start(|event: Event<TouchEventData>| event.stop_propagation())
                .on_wheel(|event: Event<WheelEventData>| event.stop_propagation())
                .child(ReportedPopup { child: popup })
        });
        rect()
            .on_sized(move |event: Event<SizedEventData>| anchor.set_if_modified(event.area))
            .child(self.child.clone())
            .maybe_child(popup)
    }
}

/// Window position `(top, left)` of a `popup` sized `(width, height)` for a
/// button at `anchor`, kept inside `bounds`.
#[cfg_attr(feature = "sismo", sismo::function("markdown::popup_origin"))]
pub(crate) fn popup_origin(anchor: Area, popup: (f32, f32), bounds: Area) -> (f32, f32) {
    let (width, height) = popup;
    let below = anchor.max_y() + POPUP_GAP;
    let above = anchor.min_y() - POPUP_GAP - height;
    let (min_top, max_bottom) = (bounds.min_y() + POPUP_MARGIN, bounds.max_y() - POPUP_MARGIN);
    let top = if bounds.height() <= 0. || below + height <= max_bottom {
        below
    } else if above >= min_top {
        above
    } else {
        (max_bottom - height).max(min_top)
    };
    let (min_left, max_left) = (
        bounds.min_x() + POPUP_MARGIN,
        (bounds.max_x() - width - POPUP_MARGIN).max(bounds.min_x() + POPUP_MARGIN),
    );
    let left = if bounds.width() <= 0. {
        anchor.min_x()
    } else {
        anchor.min_x().clamp(min_left, max_left)
    };
    (top, left)
}

pub(crate) fn popup_frame(colors: &EditorColors) -> Rect {
    rect()
        .padding(8.)
        .background(colors.popup_background)
        .corner_radius(10.)
        .border(
            Border::new()
                .width(1.)
                .alignment(BorderAlignment::Inner)
                .fill(colors.border),
        )
        .shadow(
            Shadow::new()
                .y(6.)
                .blur(18.)
                .color(Color::from_argb(70, 0, 0, 0)),
        )
}

/// One entry of a popup menu.
#[derive(Clone, PartialEq)]
pub(crate) struct MenuItem {
    pub(crate) text: String,
    /// Quieter text after the entry, cut short when it does not fit.
    pub(crate) detail: Option<String>,
    pub(crate) icon: Option<(Bytes, Color)>,
    pub(crate) selected: bool,
    pub(crate) on_press: EventHandler<()>,
}

impl MenuItem {
    pub(crate) fn new(text: impl Into<String>, on_press: impl FnMut() + 'static) -> Self {
        let mut on_press = on_press;
        Self {
            text: text.into(),
            detail: None,
            icon: None,
            selected: false,
            on_press: EventHandler::new(move |()| on_press()),
        }
    }

    pub(crate) fn icon(mut self, icon: Bytes, color: Color) -> Self {
        self.icon = Some((icon, color));
        self
    }

    pub(crate) fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub(crate) const fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// The row painted for this entry. `after` runs once the entry's own
    /// action did, typically to close the menu.
    #[cfg_attr(feature = "sismo", sismo::function("markdown::menu_row"))]
    pub(crate) fn row(
        &self,
        key: usize,
        colors: &EditorColors,
        after: Option<EventHandler<()>>,
    ) -> Element {
        let on_press = self.on_press.clone();
        rect()
            .key(key)
            .width(Size::fill())
            .height(Size::px(MENU_ROW))
            .padding((0., 10.))
            .corner_radius(6.)
            .horizontal()
            .content(Content::flex())
            .spacing(10.)
            .cross_align(Alignment::center())
            .background(if self.selected {
                colors.popup_selected
            } else {
                Color::TRANSPARENT
            })
            .cursor(CursorIcon::Pointer)
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
                if ends_a_scroll(event.data()) {
                    return;
                }
                on_press.call(());
                if let Some(after) = &after {
                    after.call(());
                }
            })
            .maybe_child(self.icon.clone().map(|(icon, color)| {
                SvgViewer::new(icon)
                    .color(color)
                    .width(Size::px(16.))
                    .height(Size::px(16.))
            }))
            .child(
                label()
                    .font_size(13.5)
                    .color(colors.text)
                    .max_lines(1)
                    .text_overflow(TextOverflow::Ellipsis)
                    .width(if self.detail.is_some() {
                        Size::auto()
                    } else {
                        Size::flex(1.)
                    })
                    .text(self.text.clone()),
            )
            .maybe_child(self.detail.clone().map(|detail| {
                label()
                    .width(Size::flex(1.))
                    .font_size(12.)
                    .font_family("monospace")
                    .color(colors.muted)
                    .max_lines(1)
                    .text_overflow(TextOverflow::Ellipsis)
                    .text_align(TextAlign::End)
                    .text(detail)
            }))
            .into_element()
    }
}

/// Colors of a text field inside a popup, following the editor theme.
pub(crate) fn input_colors(colors: &EditorColors) -> InputColorsThemePartial {
    InputColorsThemePartial {
        background: Some(colors.code_background.into()),
        focus_background: Some(colors.code_background.into()),
        color: Some(colors.text.into()),
        placeholder_color: Some(colors.placeholder.into()),
        border_fill: Some(colors.border.into()),
        focus_border_fill: Some(colors.accent.into()),
    }
}

/// Thin line between groups of menu entries.
pub(crate) fn menu_separator(colors: &EditorColors) -> Element {
    rect()
        .width(Size::fill())
        .height(Size::px(1.))
        .margin((4., 6.))
        .background(colors.border)
        .into_element()
}

#[cfg(test)]
mod tests {
    use freya::prelude::Area;

    use super::popup_origin;

    fn button(top: f32) -> Area {
        Area::new((100., top).into(), (30., 30.).into())
    }

    fn window(width: f32, height: f32) -> Area {
        Area::new((0., 0.).into(), (width, height).into())
    }

    #[test]
    fn opens_below_when_it_fits() {
        let (top, left) = popup_origin(button(10.), (150., 200.), window(800., 600.));
        assert!((top - 44.).abs() < f32::EPSILON);
        assert!((left - 100.).abs() < f32::EPSILON);
    }

    #[test]
    fn flips_above_near_the_bottom() {
        let (top, _) = popup_origin(button(550.), (150., 200.), window(800., 600.));
        assert!((top - 346.).abs() < f32::EPSILON);
    }

    #[test]
    fn flips_above_a_covered_bottom() {
        // Window 600 tall, but only the top 400 visible above a keyboard.
        let (top, _) = popup_origin(button(330.), (150., 200.), window(800., 400.));
        assert!((top - 126.).abs() < f32::EPSILON);
    }

    #[test]
    fn stays_inside_the_window() {
        let (top, left) = popup_origin(button(100.), (300., 580.), window(320., 600.));
        assert!((top - 12.).abs() < f32::EPSILON);
        assert!((left - 12.).abs() < f32::EPSILON);
    }
}

//! Overlay primitive shared by every dialog / sheet / popover in the
//! app.
//!
//! One controller ([`ModalController`]) lives at the root scope, one
//! portal ([`ModalPortal`]) sits as the last child of the app shell,
//! and every consumer calls
//! `ModalController::get().open(Modal::new(body).center())`.
//! `open` replaces the top modal; `push` stacks one above it (Settings
//! › Vincular). Escape and backdrop taps only reach the top one.
//!
//! # Builder style
//!
//! [`Modal`] follows the freya-widget convention: no `.build()`
//! terminator, setters consume `self`, and the type itself implements
//! [`Component`]. It is only rendered by [`ModalPortal`]; consumers
//! should not embed a `Modal` in their own tree.
//!
//! Consumers that need dialog-shaped bodies (title / description /
//! actions) can build them explicitly and hand the resulting element
//! to `Modal::new(body)`. Higher-level wrappers (Dialog, Sheet, …)
//! live in sibling modules and delegate here.

use freya::animation::{AnimNum, Ease, Function, OnCreation, ReadAnimatedValue, use_animation};
use freya::prelude::*;

use crate::viewport::Gutters;

/// Host-provided chrome hooks.
///
/// The covered edges of the window and a mask sink the modal publishes
/// its area to. Installed via
/// `use_provide_context(ModalChrome { … })` at app root; absent context
/// means default (nothing covered, no mask) which is the common case for
/// plugin-opened modals that do not need to block any underlying
/// stroke-canvas input.
#[derive(Clone, Default)]
pub struct ModalChrome {
    /// Live covered edges (system bars, cutouts, keyboard) in layout
    /// units.
    pub gutters: Option<Memo<Gutters>>,
    pub on_sized: Option<Callback<Area, ()>>,
    pub on_dismiss: Option<Callback<(), ()>>,
}

impl ModalChrome {
    /// The covered edges now; subscribes the caller to their changes.
    #[must_use]
    pub fn gutters(&self) -> Gutters {
        self.gutters
            .map(|gutters| *gutters.read())
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for ModalChrome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModalChrome")
            .field("gutters", &self.gutters.map(|gutters| *gutters.peek()))
            .finish_non_exhaustive()
    }
}

/// Where the modal card sits inside the covered area.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ModalPlacement {
    /// Card centered horizontally + vertically over the backdrop.
    /// The default — matches the classic dialog shape.
    #[default]
    Center,
    /// Card anchored to the bottom of the covered area, spanning the
    /// full width by default. Consumers usually set their own
    /// horizontal padding on the body.
    BottomSheet,
    /// No wrapper alignment — the body positions itself with
    /// `Position::new_absolute()` inside the full-area backdrop. Use
    /// for popovers and FAB menus that anchor to a specific corner.
    Manual,
}

impl ModalPlacement {
    /// Card scale and vertical offset at entrance progress `settle`
    /// (0 just opened, 1 at rest).
    fn entrance(self, settle: f32) -> (f32, f32) {
        let rest = 1.0 - settle;
        match self {
            Self::Center => (0.04f32.mul_add(-rest, 1.0), 0.0),
            Self::BottomSheet => (1.0, 48.0 * rest),
            Self::Manual => (0.02f32.mul_add(-rest, 1.0), -6.0 * rest),
        }
    }
}

/// Modal descriptor. Cheap to clone (only holds an `Element` and a
/// handful of `Copy` fields), safe to stash inside a `State` for the
/// controller to read/write.
#[derive(Clone)]
pub struct Modal {
    body: Element,
    placement: ModalPlacement,
    width: Option<f32>,
    dismiss_on_backdrop: bool,
    dismiss_on_escape: bool,
    backdrop_alpha: u8,
    on_close: Option<NoArgCallback<()>>,
    opened_at: std::time::Instant,
}

impl std::fmt::Debug for Modal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Modal")
            .field("placement", &self.placement)
            .field("width", &self.width)
            .field("dismiss_on_backdrop", &self.dismiss_on_backdrop)
            .field("dismiss_on_escape", &self.dismiss_on_escape)
            .field("backdrop_alpha", &self.backdrop_alpha)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Modal {
    // Force the portal to redraw every time a new Modal is pushed, even
    // when the shape happens to match structurally — bodies are opaque
    // `Element`s and equality across them is meaningless.
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Modal {
    /// Start a new modal with the given body element. The body owns
    /// its own layout inside the placement-provided container.
    #[must_use]
    pub fn new(body: impl IntoElement + 'static) -> Self {
        Self {
            body: body.into_element(),
            placement: ModalPlacement::default(),
            width: None,
            dismiss_on_backdrop: true,
            dismiss_on_escape: true,
            backdrop_alpha: 160,
            on_close: None,
            opened_at: std::time::Instant::now(),
        }
    }

    #[must_use]
    pub const fn placement(mut self, placement: ModalPlacement) -> Self {
        self.placement = placement;
        self
    }

    #[must_use]
    pub const fn center(self) -> Self {
        self.placement(ModalPlacement::Center)
    }

    #[must_use]
    pub const fn bottom_sheet(self) -> Self {
        self.placement(ModalPlacement::BottomSheet)
    }

    #[must_use]
    pub const fn manual(self) -> Self {
        self.placement(ModalPlacement::Manual)
    }

    /// Pin the card to a specific pixel width. Skipped for
    /// [`ModalPlacement::Manual`] since the body controls its own
    /// geometry.
    #[must_use]
    pub const fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Whether tapping outside the body dismisses the modal. Defaults
    /// to `true`.
    #[must_use]
    pub const fn dismiss_on_backdrop(mut self, dismiss: bool) -> Self {
        self.dismiss_on_backdrop = dismiss;
        self
    }

    /// Whether pressing `Escape` (desktop) or the Android back button
    /// dismisses the modal. Defaults to `true`. Android's hardware /
    /// gesture back arrives via winit as `NamedKey::BrowserBack`.
    #[must_use]
    pub const fn dismiss_on_escape(mut self, dismiss: bool) -> Self {
        self.dismiss_on_escape = dismiss;
        self
    }

    /// 0-255 alpha for the black scrim. Defaults to `160`; drop it to
    /// `0` for a transparent popover.
    #[must_use]
    pub const fn backdrop_alpha(mut self, alpha: u8) -> Self {
        self.backdrop_alpha = alpha;
        self
    }

    /// Fired after the modal closes for any reason (backdrop click,
    /// explicit `ModalController::close`, replacement).
    #[must_use]
    pub fn on_close(mut self, f: impl Into<NoArgCallback<()>>) -> Self {
        self.on_close = Some(f.into());
        self
    }
}

#[derive(Clone, PartialEq, Default, Debug)]
struct ModalState {
    /// Open modals, bottom first. The last one is on top and receives
    /// Escape / back.
    stack: Vec<Modal>,
}

/// A backdrop tap this soon after opening belongs to the gesture that
/// opened the modal (the release of a secondary click), not to the user
/// dismissing it.
const DISMISS_GRACE: std::time::Duration = std::time::Duration::from_millis(300);

/// Root-scoped controller shared across the app.
///
/// Look up with [`ModalController::get`]: the first call inside the process
/// installs the state on `ScopeId::ROOT` so subsequent calls in any
/// scope observe the same slot.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ModalController {
    state: State<ModalState>,
}

impl std::fmt::Debug for ModalController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModalController").finish_non_exhaustive()
    }
}

impl ModalController {
    /// Fetch (or lazily install) the controller.
    ///
    /// # Panics
    /// Panics if the root scope cannot host a new state — should never
    /// happen in a running freya app, but the underlying
    /// `create_in_scope` may abort in a corrupted runtime.
    #[must_use]
    pub fn get() -> Self {
        if let Some(rt) = try_consume_root_context::<Self>() {
            return rt;
        }
        let controller = Self {
            state: State::create_in_scope(ModalState::default(), ScopeId::ROOT),
        };
        provide_context_for_scope_id(controller, ScopeId::ROOT);
        controller
    }

    /// Replace the top modal with `modal` (or open it when none is
    /// open). Fires the replaced modal's `on_close`.
    pub fn open(&mut self, modal: Modal) {
        let replaced = self.state.write().stack.pop();
        Self::fire_on_close(replaced);
        self.state.write().stack.push(modal);
    }

    /// Open `modal` on top of the current one, which stays underneath
    /// (Settings › Vincular, a confirmation over a sheet).
    pub fn push(&mut self, modal: Modal) {
        self.state.write().stack.push(modal);
    }

    /// Dismiss the current modal (fires `on_close` if set). No-op when
    /// nothing is open.
    pub fn close(&mut self) {
        if !self.is_open() {
            return;
        }
        let closed = self.state.write().stack.pop();
        Self::fire_on_close(closed);
    }

    /// Run `f` when the top modal closes for any reason (backdrop,
    /// Escape, `close`).
    pub fn set_top_on_close(&mut self, f: impl Into<NoArgCallback<()>>) {
        if let Some(top) = self.state.write().stack.last_mut() {
            top.on_close = Some(f.into());
        }
    }

    /// Forget the top modal's close callback (it is being handed over
    /// elsewhere, not dismissed).
    pub fn clear_top_on_close(&mut self) {
        if let Some(top) = self.state.write().stack.last_mut() {
            top.on_close = None;
        }
    }

    /// Dismiss every open modal, top first.
    pub fn close_all(&mut self) {
        while self.is_open() {
            self.close();
        }
    }

    /// Whether a modal is currently open. Useful for suppressing
    /// hotkeys or backdrop hits from outside handlers.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.state.read().stack.is_empty()
    }

    /// How many modals are stacked.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.state.read().stack.len()
    }

    fn fire_on_close(modal: Option<Modal>) {
        if let Some(cb) = modal.and_then(|m| m.on_close) {
            cb.call();
        }
    }
}

/// Renders the currently open modal (if any) over the tracked `area`.
/// Place as the **last child** of the app shell so it stacks above
/// every route.
#[derive(Clone, PartialEq, Eq)]
pub struct ModalPortal {
    area: State<Area>,
}

impl std::fmt::Debug for ModalPortal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModalPortal").finish_non_exhaustive()
    }
}

impl ModalPortal {
    #[must_use]
    pub fn new(area: impl Into<State<Area>>) -> Self {
        Self { area: area.into() }
    }
}

impl Component for ModalPortal {
    fn render(&self) -> impl IntoElement {
        let controller = ModalController::get();
        let stack = controller.state.read().stack.clone();
        let top = stack.len().saturating_sub(1);
        let mut veiled = false;
        rect()
            .position(Position::new_absolute().top(0.0).left(0.0))
            .children(stack.into_iter().enumerate().map(|(depth, model)| {
                let veil = Veil::for_layer(model.backdrop_alpha, veiled);
                veiled |= veil == Veil::Blur;
                ModalOverlay {
                    model,
                    area: self.area,
                    depth,
                    is_top: depth == top,
                    veil,
                }
                .into_element()
            }))
    }
}

/// What a modal paints over the layers beneath it. Only the lowest
/// modal of a stack blurs; the ones above dim lightly so the blur is
/// never applied twice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Veil {
    Blur,
    Dim,
    /// Popovers: nothing visible, but still solid so the pointer does
    /// not hover what lies under it.
    Clear,
}

impl Veil {
    const BLUR: f32 = 10.0;

    const fn for_layer(alpha: u8, below_is_veiled: bool) -> Self {
        match (alpha, below_is_veiled) {
            (0, _) => Self::Clear,
            (_, false) => Self::Blur,
            (_, true) => Self::Dim,
        }
    }

    /// Scrim colour and blur radius at animation progress `t`.
    fn paint(self, alpha: u8, t: f32) -> (Color, f32) {
        let scaled = |a: f32| {
            // Clamped to 1..=255 so the scrim stays solid for hit testing.
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let a = (a * t).round().clamp(1.0, 255.0) as u8;
            Color::from_argb(a, 0, 0, 0)
        };
        match self {
            Self::Blur => (scaled(f32::from(alpha)), Self::BLUR * t),
            Self::Dim => (scaled(f32::from(alpha) / 3.0), 0.0),
            Self::Clear => (Color::from_argb(1, 0, 0, 0), 0.0),
        }
    }
}

#[derive(Clone, PartialEq)]
struct ModalOverlay {
    model: Modal,
    area: State<Area>,
    depth: usize,
    is_top: bool,
    veil: Veil,
}

impl std::fmt::Debug for ModalOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModalOverlay")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl Component for ModalOverlay {
    fn render(&self) -> impl IntoElement {
        let area = *self.area.read();
        let model = self.model.clone();
        // Optional host chrome — insets (bottom gutter) + mask sink.
        // Absent context means a plugin-mounted modal that does not
        // need safe-area padding or pen-input blocking.
        let chrome = use_try_consume::<ModalChrome>().unwrap_or_default();
        let gutters = chrome.gutters();
        let dismiss_sink = chrome.on_dismiss.clone();
        use_drop(move || {
            if let Some(cb) = dismiss_sink {
                cb.call(());
            }
        });

        // The card is never faded: freya applies opacity to every node on
        // its own, so a half-transparent card shows its own panels and the
        // veil through each other. It grows (centred), rises (sheets) or
        // drops in (popovers) while the veil fades in behind it.
        let animations = use_animation(|conf| {
            conf.on_creation(OnCreation::Run);
            (
                AnimNum::new(0., 1.)
                    .time(180)
                    .ease(Ease::Out)
                    .function(Function::Cubic),
                AnimNum::new(0., 1.)
                    .time(240)
                    .ease(Ease::Out)
                    .function(Function::Expo),
            )
        });
        let (fade, settle) = if crate::theme::reduce_motion() {
            (1.0, 1.0)
        } else {
            animations.get().value()
        };

        let width = area.width();
        let height = area.height();
        // Where the card may sit: the window without its covered edges
        // (system bars, cutouts, keyboard). The backdrop still covers the
        // whole window so taps on those edges dismiss the modal.
        let frame = gutters.inside(area);
        let placement = model.placement;
        let dismiss = model.dismiss_on_backdrop;
        let dismiss_on_escape = model.dismiss_on_escape && self.is_top;
        // Two overlay levels per modal, capped below freya's 16.
        let level = 4 + 2 * u8::try_from(self.depth.min(5)).unwrap_or(5);
        let (scrim, blur) = self.veil.paint(model.backdrop_alpha, fade);
        let (scale, rise) = placement.entrance(settle);
        let opened_at = model.opened_at;
        // A pinned width never runs past the side gutters.
        let body_width = model.width.map(|w| w.min((frame.width() - 32.0).max(0.0)));
        let body = model.body;

        // Backdrop and card are rendered as **siblings** under the
        // top-level layer-overlay rect. Freya bubbles press events up
        // the ancestor chain of the hit element, never across siblings
        // — so a click on the card can never trigger the backdrop's
        // dismiss handler, and nested `Modal::open` calls from inside
        // a card body install the new modal without the previous
        // card's backdrop re-closing it. Empty-space clicks still
        // land directly on the backdrop, which fires `close` when
        // `dismiss_on_backdrop` is set.
        //
        // Manual placement is a special case: the body positions
        // itself absolutely (e.g. FabMenu anchors its card to the
        // bottom-right), so we skip the card_container wrapper and
        // paint the body directly as backdrop's sibling. A wrapper
        // rect would cover the full area and swallow empty-space
        // clicks that should have dismissed the popover.
        let body_node = rect()
            .layer(Layer::OverlayLevel(level + 1))
            .maybe(body_width.is_some(), |r| {
                r.width(Size::px(body_width.unwrap_or_default()))
            })
            .offset_y(rise)
            .scale((scale, scale))
            .child(body);

        let card_layer = match placement {
            ModalPlacement::Manual => body_node.into_element(),
            ModalPlacement::Center | ModalPlacement::BottomSheet => {
                card_container(placement, frame, gutters)
                    .layer(Layer::OverlayLevel(level + 1))
                    .child(body_node)
                    .into_element()
            }
        };

        rect()
            .layer(Layer::OverlayLevel(level))
            .position(Position::new_absolute().top(0.0).left(0.0))
            .width(Size::px(width))
            .height(Size::px(height))
            .on_sized({
                let sink = chrome.on_sized;
                move |e: Event<SizedEventData>| {
                    if let Some(cb) = &sink {
                        cb.call(e.area);
                    }
                }
            })
            .maybe(dismiss_on_escape, |r| {
                r.on_global_key_down(|e: Event<KeyboardEventData>| {
                    if matches!(
                        e.key,
                        Key::Named(NamedKey::Escape | NamedKey::BrowserBack)
                    ) {
                        e.stop_propagation();
                        ModalController::get().close();
                    }
                })
            })
            .child(
                rect()
                    .position(Position::new_absolute().top(0.0).left(0.0))
                    .width(Size::px(width))
                    .height(Size::px(height))
                    .background(scrim)
                    .maybe(blur > 0.0, |r| r.backdrop_blur(blur))
                    .maybe(dismiss && self.is_top, |r| {
                        r.on_press(move |_| {
                            if opened_at.elapsed() >= DISMISS_GRACE {
                                ModalController::get().close();
                            }
                        })
                    }),
            )
            .child(card_layer)
    }
}

/// Placement-aware wrapper that positions the card without covering
/// the full backdrop area — the wrapper hugs the card so empty-space
/// clicks pass through to the backdrop sibling underneath. `frame` is
/// the window without its covered edges (`gutters`): centred cards sit in
/// its middle and sheets rest on its bottom, above the keyboard, so inputs
/// stay visible while typing.
fn card_container(placement: ModalPlacement, frame: Area, gutters: Gutters) -> Rect {
    match placement {
        ModalPlacement::Center => rect()
            .position(
                Position::new_absolute()
                    .top(frame.min_y())
                    .left(frame.min_x()),
            )
            .width(Size::px(frame.width()))
            .height(Size::px(frame.height()))
            .center(),
        // Bottom sheet spans the full width by convention — a click on
        // the sheet area is a click "on the card". Above the sheet is
        // still empty backdrop and dismisses when configured.
        ModalPlacement::BottomSheet => rect()
            .position(
                Position::new_absolute()
                    .bottom(gutters.bottom)
                    .left(frame.min_x()),
            )
            .width(Size::px(frame.width()))
            .cross_align(Alignment::Center),
        // Manual: body owns positioning, wrapper unused (handled by
        // caller emitting `body_node` directly as backdrop sibling).
        ModalPlacement::Manual => rect(),
    }
}

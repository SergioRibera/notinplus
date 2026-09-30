//! Overlay primitive shared by every dialog / sheet / popover in the
//! app.
//!
//! One controller ([`ModalController`]) lives at the root scope, one
//! portal ([`ModalPortal`]) sits as the last child of the app shell,
//! and every consumer calls
//! `ModalController::get().open(Modal::new(body).center())`.
//! Only one modal is visible at a time — opening a second replaces the
//! first — which keeps z-index reasoning trivial without a stack.
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

use freya::animation::*;
use freya::prelude::*;

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
    modal: Option<Modal>,
}

/// Root-scoped controller shared across the app. Look up with
/// [`ModalController::get`] — the first call inside the process
/// installs the state on `ScopeId::ROOT` so subsequent calls in any
/// scope observe the same slot.
#[derive(Clone, Copy, PartialEq)]
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

    /// Replace whatever modal is open with `modal`. Fires the previous
    /// modal's `on_close` before installing the new one.
    pub fn open(&mut self, modal: Modal) {
        self.take_on_close_cb();
        self.state.set(ModalState { modal: Some(modal) });
    }

    /// Dismiss the current modal (fires `on_close` if set). No-op when
    /// nothing is open.
    pub fn close(&mut self) {
        if !self.is_open() {
            return;
        }
        self.take_on_close_cb();
        self.state.set(ModalState { modal: None });
    }

    /// Whether a modal is currently open. Useful for suppressing
    /// hotkeys or backdrop hits from outside handlers.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state.read().modal.is_some()
    }

    fn take_on_close_cb(&mut self) -> Option<NoArgCallback<()>> {
        let cb = self
            .state
            .read()
            .modal
            .as_ref()
            .and_then(|m| m.on_close.clone());
        if let Some(cb) = &cb {
            cb.call();
        }
        cb
    }
}

/// Renders the currently open modal (if any) over the tracked `area`.
/// Place as the **last child** of the app shell so it stacks above
/// every route.
#[derive(Clone, PartialEq)]
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
        let model = controller.state.read().modal.clone();
        match model {
            Some(m) => ModalOverlay {
                model: m,
                area: self.area,
            }
            .into_element(),
            None => rect().into_element(),
        }
    }
}

#[derive(Clone, PartialEq)]
struct ModalOverlay {
    model: Modal,
    area: State<Area>,
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

        let animations = use_animation(|conf| {
            conf.on_creation(OnCreation::Run);
            (
                AnimNum::new(0., 1.)
                    .time(180)
                    .ease(Ease::Out)
                    .function(Function::Cubic),
                AnimNum::new(0.94, 1.)
                    .time(220)
                    .ease(Ease::Out)
                    .function(Function::Expo),
                AnimNum::new(0., 10.)
                    .time(180)
                    .ease(Ease::Out)
                    .function(Function::Cubic),
            )
        });
        let (opacity, scale, blur) = animations.get().value();

        let width = area.width();
        let height = area.height();
        let placement = model.placement;
        let dismiss = model.dismiss_on_backdrop;
        let dismiss_on_escape = model.dismiss_on_escape;
        let backdrop_alpha = model.backdrop_alpha;
        let body_width = model.width;
        let body = model.body.clone();

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
            .maybe(body_width.is_some(), |r| {
                r.width(Size::px(body_width.unwrap_or_default()))
            })
            .opacity(opacity as f32)
            .scale((scale as f32, scale as f32))
            .child(body);

        let card_layer = match placement {
            ModalPlacement::Manual => body_node.into_element(),
            ModalPlacement::Center | ModalPlacement::BottomSheet => {
                card_container(placement, width, height)
                    .child(body_node)
                    .into_element()
            }
        };

        rect()
            .layer(Layer::Overlay)
            .position(Position::new_absolute().top(0.0).left(0.0))
            .width(Size::px(width))
            .height(Size::px(height))
            .maybe(dismiss_on_escape, |r| {
                r.on_global_key_down(|e: Event<KeyboardEventData>| {
                    if matches!(
                        e.key,
                        Key::Named(NamedKey::Escape) | Key::Named(NamedKey::BrowserBack)
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
                    .background(Color::from_argb(backdrop_alpha, 0, 0, 0))
                    .blur(blur as f32)
                    .maybe(dismiss, |r| r.on_press(|_| ModalController::get().close())),
            )
            .child(card_layer)
    }
}

/// Placement-aware wrapper that positions the card without covering
/// the full backdrop area — the wrapper hugs the card so empty-space
/// clicks pass through to the backdrop sibling underneath.
fn card_container(placement: ModalPlacement, width: f32, height: f32) -> Rect {
    match placement {
        ModalPlacement::Center => rect()
            .position(Position::new_absolute().top(0.0).left(0.0))
            .width(Size::px(width))
            .height(Size::px(height))
            .center(),
        // Bottom sheet spans the full width by convention — a click on
        // the sheet area is a click "on the card". Above the sheet is
        // still empty backdrop and dismisses when configured.
        ModalPlacement::BottomSheet => rect()
            .position(Position::new_absolute().bottom(0.0).left(0.0))
            .width(Size::px(width))
            .cross_align(Alignment::Center),
        // Manual: body owns positioning, wrapper unused (handled by
        // caller emitting `body_node` directly as backdrop sibling).
        ModalPlacement::Manual => rect(),
    }
}

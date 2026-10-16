//! Slide-up bottom sheet modal. One `BottomSheet` instance renders a
//! backdrop + a card anchored to the bottom of the viewport; the card
//! animates in/out and the backdrop dismisses on tap when enabled.
//!
//! # Usage
//!
//! ```rust,no_run
//! # use freya::prelude::*;
//! # use notein_ui_kit::BottomSheet;
//! # fn page() -> impl IntoElement {
//! let open = use_state(|| false);
//! BottomSheet::new(label().text("hi"))
//!     .open(open)
//!     .height(360.0)
//! # }
//! ```
//!
//! For a navigable sheet (two pages that slide horizontally — e.g. a
//! book → chapter picker), compose with [`SheetPager`]:
//!
//! ```rust,no_run
//! # use freya::prelude::*;
//! # use notein_ui_kit::{BottomSheet, SheetPager};
//! # fn page() -> impl IntoElement {
//! let open = use_state(|| false);
//! let index = use_state(|| 0usize);
//! BottomSheet::new(
//!     SheetPager::new(index, vec![
//!         label().text("books").into_element(),
//!         label().text("chapters").into_element(),
//!     ]),
//! )
//! .open(open)
//! .height(420.0)
//! # }
//! ```
//!
//! Rendered as `Layer::OverlayLevel(5)` so it sits above every regular
//! paint sibling without the consumer threading layer hints.

use freya::animation::*;
use freya::prelude::*;

/// Backdrop + card shell. `body` is placed inside the card; the sheet
/// handles the slide-up animation + the semi-opaque backdrop tap.
#[derive(Clone, PartialEq)]
pub struct BottomSheet {
    open: Option<State<bool>>,
    height: f32,
    dismiss_on_backdrop: bool,
    backdrop_alpha: u8,
    body: Element,
}

impl std::fmt::Debug for BottomSheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BottomSheet")
            .field("height", &self.height)
            .field("dismiss_on_backdrop", &self.dismiss_on_backdrop)
            .finish()
    }
}

impl BottomSheet {
    pub fn new(body: impl IntoElement) -> Self {
        Self {
            open: None,
            height: 360.0,
            dismiss_on_backdrop: true,
            backdrop_alpha: 120,
            body: body.into_element(),
        }
    }

    pub fn open(mut self, open: State<bool>) -> Self {
        self.open = Some(open);
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    pub fn dismiss_on_backdrop(mut self, dismiss: bool) -> Self {
        self.dismiss_on_backdrop = dismiss;
        self
    }

    pub fn backdrop_alpha(mut self, alpha: u8) -> Self {
        self.backdrop_alpha = alpha;
        self
    }
}

impl Component for BottomSheet {
    fn render(&self) -> impl IntoElement {
        let Some(open) = self.open else {
            return rect().into_element();
        };
        if !*open.read() {
            return rect().into_element();
        }

        let height = self.height;
        let backdrop_alpha = self.backdrop_alpha;
        let dismiss_on_backdrop = self.dismiss_on_backdrop;
        let body = self.body.clone();

        // Window area captured via `on_sized` on the invisible probe
        // the sheet mounts first — gives absolute dimensions for the
        // backdrop + card, so the sheet doesn't depend on how the
        // parent sizes it. Positions below are global so the sheet
        // sits over the whole window, regardless of the ancestor it
        // was mounted under.
        let mut area: State<Area> = use_state(Area::default);

        // 0 → 1 drives both the fade of the backdrop + the slide-up of
        // the card from `height` px below its resting position.
        let animations = use_animation(|conf| {
            conf.on_creation(OnCreation::Run);
            (
                AnimNum::new(0., 1.)
                    .time(220)
                    .ease(Ease::Out)
                    .function(Function::Cubic),
                AnimNum::new(1., 0.)
                    .time(260)
                    .ease(Ease::Out)
                    .function(Function::Expo),
            )
        });
        let (fade, slide_remaining) = animations.get().value();

        let translate_y = -((slide_remaining as f32) * height);
        let alpha = ((fade as f64) * f64::from(backdrop_alpha)) as u8;

        let close = {
            let mut open = open;
            move |_: Event<PressEventData>| {
                if dismiss_on_backdrop {
                    open.set(false);
                }
            }
        };

        let a = *area.read();
        let w = a.width();
        let h = a.height();

        rect()
            .layer(Layer::OverlayLevel(5))
            .position(Position::new_global().top(0.).left(0.))
            .width(Size::fill())
            .height(Size::fill())
            .on_sized(move |e: Event<SizedEventData>| {
                area.set(e.data().area);
            })
            .child(
                // Backdrop — stretched to the captured window area so
                // taps anywhere outside the card dismiss it.
                rect()
                    .position(Position::new_global().top(0.).left(0.))
                    .width(Size::px(w.max(0.0)))
                    .height(Size::px(h.max(0.0)))
                    .background(Color::from_argb(alpha, 0, 0, 0))
                    .on_press(close),
            )
            .child(
                // Card pinned to the window's bottom edge, offset
                // down by `translate_y` during the slide-in.
                rect()
                    .position(
                        Position::new_global()
                            .left(0.)
                            .bottom(translate_y),
                    )
                    .width(Size::px(w.max(0.0)))
                    .height(Size::px(height))
                    .background(Color::from_rgb(26, 26, 30))
                    .corner_radius(CornerRadius::new(16.0, 16.0, 0.0, 0.0))
                    .padding((12., 16.))
                    .spacing(10.0)
                    .child(sheet_handle_bar())
                    .child(body),
            )
            .into_element()
    }
}

/// Thin grab-handle at the top of the card. Pure cosmetic — the
/// backdrop is where the dismiss gesture lives.
fn sheet_handle_bar() -> impl IntoElement {
    rect()
        .width(Size::fill())
        .height(Size::px(14.0))
        .main_align(Alignment::Center)
        .cross_align(Alignment::Center)
        .child(
            rect()
                .width(Size::px(42.0))
                .height(Size::px(4.0))
                .with_corner_radius(2.0)
                .background(Color::from_rgb(80, 80, 86)),
        )
}

/// Horizontally-sliding pager inside a bottom sheet. `index` controls
/// which page is visible; the pager animates between adjacent indices
/// over a short cubic ease. Pages that aren't the current index render
/// their body but shift off-screen (positive index → off right,
/// negative → off left).
#[derive(Clone, PartialEq)]
pub struct SheetPager {
    index: State<usize>,
    pages: Vec<Element>,
}

impl std::fmt::Debug for SheetPager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SheetPager")
            .field("page_count", &self.pages.len())
            .finish()
    }
}

impl SheetPager {
    pub fn new(index: State<usize>, pages: Vec<Element>) -> Self {
        Self { index, pages }
    }
}

impl Component for SheetPager {
    fn render(&self) -> impl IntoElement {
        let index = self.index;
        let current = *index.read();
        let pages = self.pages.clone();

        // Reactive dep: reading `index` inside the closure wires the
        // animation to re-run whenever the state changes. Also opt
        // into `OnCreation::Run` so the initial progress animates up
        // to 1 — without it, `AnimNum::new(0.0, 1.0)` stays frozen at
        // 0 and the body renders at opacity 0 (invisible).
        let animations = use_animation(move |conf| {
            conf.on_creation(OnCreation::Run);
            conf.on_change(OnChange::Rerun);
            let _ = *index.read();
            AnimNum::new(0.0, 1.0)
                .time(220)
                .ease(Ease::Out)
                .function(Function::Cubic)
        });
        let progress = animations.get().value() as f32;

        // Starting offset for the slide-in — positive index entering
        // from the right (+40px), smaller / zero for backward nav.
        let slide_from = 40.0;
        let offset_x = (1.0 - progress) * slide_from;

        let body = pages
            .get(current)
            .cloned()
            .unwrap_or_else(|| rect().into_element());

        rect().width(Size::fill()).height(Size::fill()).child(
            rect()
                .width(Size::fill())
                .height(Size::fill())
                .offset_x(offset_x)
                .opacity(progress)
                .child(body),
        )
    }
}

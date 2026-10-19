//! Horizontal row of mesas shown at the top of Home.
//!
//! Each chip draws a miniature of the mesa's pane layout tinted with
//! its colour. Tap opens the mesa; long press (or secondary click)
//! opens the menu the caller builds for it. The last chip creates a
//! new mesa.

use std::time::Duration;

use freya::icons::lucide;
use freya::prelude::*;

use crate::library::{PaneLayout, Rgba, SplitOrientation, WorkspaceId};

const CHIP_WIDTH: f32 = 148.0;
const CHIP_HEIGHT: f32 = 92.0;
const STRIP_HEIGHT: f32 = CHIP_HEIGHT + 16.0;
const THUMB_WIDTH: f32 = 72.0;
const THUMB_HEIGHT: f32 = 44.0;
const LONG_PRESS: Duration = Duration::from_millis(450);
/// Finger travel that turns a press into a scroll instead of a menu.
const LONG_PRESS_SLOP_PX: f64 = 8.0;

/// What the strip knows about one mesa.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceCardData {
    pub id: WorkspaceId,
    pub name: String,
    pub color: Option<Rgba>,
    pub layout: PaneLayout,
    pub active: bool,
}

/// Row of mesa chips.
#[derive(Clone)]
pub struct SpaceStrip {
    spaces: Vec<SpaceCardData>,
    on_open: Callback<WorkspaceId, ()>,
    on_create: NoArgCallback<()>,
    menu_for: Callback<WorkspaceId, Menu>,
}

impl PartialEq for SpaceStrip {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl std::fmt::Debug for SpaceStrip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpaceStrip")
            .field("spaces", &self.spaces.len())
            .finish_non_exhaustive()
    }
}

impl SpaceStrip {
    #[must_use]
    pub fn new(
        spaces: Vec<SpaceCardData>,
        on_open: impl Into<Callback<WorkspaceId, ()>>,
        on_create: impl Into<NoArgCallback<()>>,
        menu_for: impl Into<Callback<WorkspaceId, Menu>>,
    ) -> Self {
        Self {
            spaces,
            on_open: on_open.into(),
            on_create: on_create.into(),
            menu_for: menu_for.into(),
        }
    }
}

/// One slot of the virtual row: a mesa or the trailing "new" chip.
#[derive(Clone, Debug, PartialEq)]
enum Slot {
    Space(SpaceCardData),
    Create,
}

impl Component for SpaceStrip {
    fn render(&self) -> impl IntoElement {
        let mut slots: Vec<Slot> = self.spaces.iter().cloned().map(Slot::Space).collect();
        slots.push(Slot::Create);
        let length = slots.len();
        let on_open = self.on_open.clone();
        let on_create = self.on_create.clone();
        let menu_for = self.menu_for.clone();
        VirtualScrollView::new_with_data(slots, move |row, data: &Vec<Slot>| {
            match data[row.index].clone() {
                Slot::Space(space) => SpaceChip {
                    space,
                    on_open: on_open.clone(),
                    menu_for: menu_for.clone(),
                }
                .into_element(),
                Slot::Create => create_chip(on_create.clone()).into_element(),
            }
        })
        .direction(Direction::Horizontal)
        .length(length)
        .item_size(CHIP_WIDTH + 12.0)
        .width(Size::fill())
        .height(Size::px(STRIP_HEIGHT))
    }
}

#[derive(Clone)]
struct SpaceChip {
    space: SpaceCardData,
    on_open: Callback<WorkspaceId, ()>,
    menu_for: Callback<WorkspaceId, Menu>,
}

impl PartialEq for SpaceChip {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl std::fmt::Debug for SpaceChip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpaceChip")
            .field("id", &self.space.id)
            .finish_non_exhaustive()
    }
}

impl Component for SpaceChip {
    fn render(&self) -> impl IntoElement {
        let press_token = use_state(|| 0u64);
        let long_fired = use_state(|| false);
        let press_start = use_state(|| Option::<CursorPoint>::None);

        let id = self.space.id;
        let tint = self
            .space
            .color
            .map_or(Color::from_rgb(120, 130, 160), |c| {
                Color::from_rgb(c[0], c[1], c[2])
            });
        let bg = if self.space.active {
            Color::from_rgb(40, 42, 54)
        } else {
            Color::from_rgb(26, 26, 32)
        };
        let border = if self.space.active {
            tint
        } else {
            Color::from_rgb(40, 40, 48)
        };
        let on_open = self.on_open.clone();
        let menu_for = self.menu_for.clone();
        let menu_for_long = menu_for.clone();

        rect()
            .width(Size::px(CHIP_WIDTH))
            .height(Size::px(CHIP_HEIGHT))
            .margin((8.0, 6.0))
            .padding((10.0, 12.0))
            .vertical()
            .spacing(8.0)
            .background(bg)
            .border(Border::new().width(1.5).fill(border))
            .with_corner_radius(12.0)
            .on_pointer_down({
                let mut long_fired = long_fired;
                let mut press_token = press_token;
                let mut press_start = press_start;
                move |e: Event<PointerEventData>| {
                    long_fired.set(false);
                    press_start.set(Some(e.global_location()));
                    let token = press_token.peek().wrapping_add(1);
                    press_token.set(token);
                    let menu_for = menu_for_long.clone();
                    spawn(async move {
                        async_io::Timer::after(LONG_PRESS).await;
                        if *press_token.peek() == token {
                            long_fired.set(true);
                            ContextMenu::open(menu_for.call(id));
                        }
                    });
                }
            })
            .on_pointer_move({
                let mut press_token = press_token;
                let mut press_start = press_start;
                move |e: Event<PointerEventData>| {
                    let Some(start) = *press_start.peek() else {
                        return;
                    };
                    let current = e.global_location();
                    let dx = current.x - start.x;
                    let dy = current.y - start.y;
                    if (dx * dx + dy * dy).sqrt() >= LONG_PRESS_SLOP_PX {
                        press_token.set(0);
                        press_start.set(None);
                    }
                }
            })
            .on_pointer_leave({
                let mut press_token = press_token;
                let mut press_start = press_start;
                move |_| {
                    press_token.set(0);
                    press_start.set(None);
                }
            })
            .on_press({
                let mut press_token = press_token;
                move |_: Event<PressEventData>| {
                    press_token.set(0);
                    if *long_fired.peek() {
                        return;
                    }
                    on_open.call(id);
                }
            })
            .on_secondary_down(move |_: Event<PressEventData>| {
                ContextMenu::open(menu_for.call(id));
            })
            .child(layout_thumb(&self.space.layout, tint))
            .child(
                label()
                    .color(Color::from_rgb(230, 230, 240))
                    .font_size(12.0)
                    .max_lines(1)
                    .text_overflow(TextOverflow::Ellipsis)
                    .text(self.space.name.clone()),
            )
    }
}

/// Miniature of the pane tree: one tinted block per leaf, split by
/// the real ratios.
fn layout_thumb(layout: &PaneLayout, tint: Color) -> Rect {
    rect()
        .width(Size::px(THUMB_WIDTH))
        .height(Size::px(THUMB_HEIGHT))
        .with_corner_radius(4.0)
        .overflow(Overflow::Clip)
        .child(thumb_node(layout, tint))
}

fn thumb_node(node: &PaneLayout, tint: Color) -> Rect {
    match node {
        PaneLayout::Leaf(pane) => {
            let fill = if pane.tabs.is_empty() {
                Color::from_argb(60, tint.r(), tint.g(), tint.b())
            } else {
                tint
            };
            rect()
                .width(Size::fill())
                .height(Size::fill())
                .margin(1.0)
                .with_corner_radius(2.0)
                .background(fill)
        }
        PaneLayout::Split {
            orientation,
            ratio,
            first,
            second,
        } => {
            let ratio = ratio.clamp(0.1, 0.9);
            let first = thumb_node(first, tint);
            let second = thumb_node(second, tint);
            match orientation {
                SplitOrientation::Horizontal => rect()
                    .width(Size::fill())
                    .height(Size::fill())
                    .horizontal()
                    .child(first.width(Size::flex(ratio)))
                    .child(second.width(Size::flex(1.0 - ratio))),
                SplitOrientation::Vertical => rect()
                    .width(Size::fill())
                    .height(Size::fill())
                    .vertical()
                    .child(first.height(Size::flex(ratio)))
                    .child(second.height(Size::flex(1.0 - ratio))),
            }
        }
    }
}

fn create_chip(on_create: NoArgCallback<()>) -> Rect {
    rect()
        .width(Size::px(CHIP_WIDTH))
        .height(Size::px(CHIP_HEIGHT))
        .margin((8.0, 6.0))
        .center()
        .vertical()
        .spacing(6.0)
        .background(Color::from_rgb(22, 22, 28))
        .border(Border::new().width(1.5).fill(Color::from_rgb(50, 50, 60)))
        .with_corner_radius(12.0)
        .on_press(move |_| on_create.call())
        .child(
            SvgViewer::new(lucide::plus())
                .color(Color::from_rgb(200, 200, 210))
                .width(Size::px(20.0))
                .height(Size::px(20.0)),
        )
        .child(
            label()
                .color(Color::from_rgb(200, 200, 210))
                .font_size(12.0)
                .text("Nueva mesa"),
        )
}

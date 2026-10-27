//! The bar over a text selection, the same in the Bible and PDFs: five
//! highlighters one tap away, a note and send to the notebook. Notes get the editor's formatting bar instead.
//!
//! A sticky bar (a selection that grows tap by tap) docks at the bottom
//! without a backdrop, so taps keep reaching the text under it.

use std::rc::Rc;

use freya::prelude::*;
use notein_plugin::ui::SelectionMenu;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{BarItem, Modal, ModalController, SelectionBar, width_class};

use crate::library::ItemId;

/// Widest a sticky bar grows on large screens.
const STICKY_MAX_WIDTH: f32 = 640.0;

/// The five highlighter inks of the theme, as sRGB.
#[must_use]
pub fn inks() -> [[u8; 3]; 5] {
    palette().highlighters.map(|c| [c.r(), c.g(), c.b()])
}

/// The sticky bar showing, shared by [`show`] and [`StickySelection`].
#[derive(Clone, Copy)]
struct Sticky(State<Option<SelectionMenu>>);

fn sticky() -> State<Option<SelectionMenu>> {
    try_consume_root_context::<Sticky>()
        .unwrap_or_else(|| {
            provide_root_context(Sticky(State::create_in_scope(None, ScopeId::ROOT)))
        })
        .0
}

/// Show the bar for `menu`.
pub fn show(menu: SelectionMenu) {
    if menu.sticky {
        sticky().set(Some(menu));
        return;
    }
    let dismiss = Rc::clone(&menu.on_dismiss);
    ModalController::get().open(
        Modal::new(Bar { menu })
            .manual()
            .dismiss_on_backdrop(true)
            .backdrop_alpha(0)
            .on_close(move || dismiss()),
    );
}

/// Take the sticky bar away without dismissing its selection.
pub fn close_sticky() {
    let mut showing = sticky();
    if showing.peek().is_some() {
        showing.set(None);
    }
}

/// Close the bar the way `menu` was shown, before (`dismiss = false`) or
/// after (`true`) an action that is done with the selection.
fn close(menu: &SelectionMenu, dismiss: bool) {
    if menu.sticky {
        close_sticky();
        if dismiss {
            (menu.on_dismiss)();
        }
        return;
    }
    if !dismiss {
        ModalController::get().clear_top_on_close();
    }
    ModalController::get().close();
}

#[derive(Clone, Debug, PartialEq)]
struct Bar {
    menu: SelectionMenu,
}

impl Bar {
    fn items(&self) -> Vec<BarItem> {
        use freya::icons::lucide;
        let menu = &self.menu;
        let inks = inks();
        let mut items = Vec::new();
        if let Some(caption) = &menu.caption {
            items.push(BarItem::Caption(caption.clone().into()));
            items.push(BarItem::Divider);
        }
        let (on_color, picked) = (Rc::clone(&menu.on_color), menu.clone());
        items.push(BarItem::Swatches {
            colors: inks
                .iter()
                .map(|c| Color::from_rgb(c[0], c[1], c[2]))
                .collect(),
            active: menu.active,
            on_pick: EventHandler::new(move |i: usize| {
                close(&picked, false);
                on_color(inks.get(i).copied());
            }),
        });
        items.push(BarItem::Divider);
        if let Some(note) = menu.on_note.clone() {
            let noted = menu.clone();
            items.push(BarItem::action(
                "Nota",
                lucide::message_square_text(),
                move |()| {
                    close(&noted, false);
                    note();
                },
            ));
        }
        if let (Some(target), Some(document)) = (menu.reference.clone(), menu.document) {
            let document = ItemId(document.0);
            items.push(BarItem::action(
                "Al cuaderno",
                lucide::notebook_pen(),
                self.finish(move || {
                    crate::references::insert_in_notebook(document, target.clone());
                }),
            ));
        }
        for (label, icon, run) in &menu.extra {
            let run = Rc::clone(run);
            items.push(BarItem::action(
                *label,
                icon.clone(),
                self.finish(move || run()),
            ));
        }
        if menu.sticky {
            let closing = menu.clone();
            items.push(BarItem::Divider);
            items.push(BarItem::icon(lucide::x(), move |()| close(&closing, true)));
        }
        items
    }

    /// An action that is done with the selection: it runs, then the bar
    /// closes as a dismissal, so the caller lets go of the selection.
    fn finish(&self, f: impl Fn() + 'static) -> impl Fn(()) + 'static {
        let menu = self.menu.clone();
        move |()| {
            f();
            close(&menu, true);
        }
    }
}

impl Component for Bar {
    fn render(&self) -> impl IntoElement {
        let size = *Platform::get().current_window().root_size.read();
        let bar = SelectionBar::new(self.items());
        let insets = *crate::hooks::use_safe_area_insets().read();
        if width_class().is_compact() {
            return rect()
                .position(Position::new_absolute().left(0.0).bottom(insets.bottom))
                .width(Size::px(size.width))
                .child(bar.docked(true))
                .into_element();
        }
        if self.menu.sticky {
            return rect()
                .position(
                    Position::new_absolute()
                        .left(0.0)
                        .bottom(insets.bottom + 24.0),
                )
                .width(Size::px(size.width))
                .main_align(Alignment::Center)
                .horizontal()
                .child(rect().max_width(Size::px(STICKY_MAX_WIDTH)).child(bar))
                .into_element();
        }
        // The line the selection ends on, roughly: the bar floats over it,
        // or under it when the top of the window is too close.
        let (x, y) = self.menu.at;
        let line = Area::new((x, y - 8.0).into(), (1.0, 28.0).into());
        notein_ui_kit::Floating::new(line, bar)
            .above()
            .centered()
            .into_element()
    }
}

/// Where a sticky selection bar shows; mounted once over the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StickySelection;

impl Component for StickySelection {
    fn render(&self) -> impl IntoElement {
        let menu = sticky().read().clone();
        menu.map_or_else(|| rect().into_element(), |menu| Bar { menu }.into_element())
    }
}

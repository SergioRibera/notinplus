//! The bar over a text selection, the same in the Bible and PDFs: five
//! highlighters one tap away, a note, copy as reference and send to the
//! notebook. Notes get the editor's formatting bar instead.

use std::rc::Rc;

use freya::prelude::*;
use notein_plugin::bookmark::SourceRef;
use notein_plugin::ui::SelectionMenu;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{BarItem, Modal, ModalController, SelectionBar, width_class};

use crate::library::ItemId;

/// The five highlighter inks of the theme, as sRGB.
#[must_use]
pub fn inks() -> [[u8; 3]; 5] {
    palette().highlighters.map(|c| [c.r(), c.g(), c.b()])
}

/// Show the bar for `menu`.
pub fn show(menu: SelectionMenu) {
    let dismiss = Rc::clone(&menu.on_dismiss);
    ModalController::get().open(
        Modal::new(Bar { menu })
            .manual()
            .dismiss_on_backdrop(true)
            .backdrop_alpha(0)
            .on_close(move || dismiss()),
    );
}

/// An action that hands the selection over to what it opens next (the
/// note popover): the menu closes without dismissing the selection.
fn act(f: impl Fn() + 'static) -> impl Fn(()) + 'static {
    move |()| {
        ModalController::get().clear_top_on_close();
        ModalController::get().close();
        f();
    }
}

/// An action that is done with the selection: it runs on the selection, then
/// the menu closes as a dismissal, so the caller lets go of it.
fn finish(f: impl Fn() + 'static) -> impl Fn(()) + 'static {
    move |()| {
        f();
        ModalController::get().close();
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Bar {
    menu: SelectionMenu,
}

impl Component for Bar {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let menu = &self.menu;
        let inks = inks();
        let on_color = Rc::clone(&menu.on_color);
        let mut items = vec![BarItem::Swatches {
            colors: inks
                .iter()
                .map(|c| Color::from_rgb(c[0], c[1], c[2]))
                .collect(),
            active: menu.active,
            on_pick: EventHandler::new(move |i: usize| {
                ModalController::get().clear_top_on_close();
                ModalController::get().close();
                on_color(inks.get(i).copied());
            }),
        }];
        items.push(BarItem::Divider);
        let note = Rc::clone(&menu.on_note);
        items.push(BarItem::action(
            "Nota",
            lucide::message_square_text(),
            act(move || note()),
        ));
        if let Some(target) = menu.reference.clone() {
            let label = label_of(&target);
            items.push(BarItem::action(
                "Copiar ref",
                lucide::copy(),
                finish(move || crate::references::copy(&target, &label)),
            ));
            if let Some(document) = menu.document {
                let (document, target) = (ItemId(document.0), menu.reference.clone());
                items.push(BarItem::action(
                    "Al cuaderno",
                    lucide::notebook_pen(),
                    finish(move || {
                        if let Some(target) = target.clone() {
                            crate::references::insert_in_notebook(document, target);
                        }
                    }),
                ));
            }
        }
        for (label, icon, run) in &menu.extra {
            let run = Rc::clone(run);
            items.push(BarItem::action(*label, icon.clone(), finish(move || run())));
        }
        let size = *Platform::get().current_window().root_size.read();
        let bar = SelectionBar::new(items);
        if width_class().is_compact() {
            let insets = *crate::hooks::use_safe_area_insets().read();
            return rect()
                .position(Position::new_absolute().left(0.0).bottom(insets.bottom))
                .width(Size::px(size.width))
                .child(bar.docked(true))
                .into_element();
        }
        // The line the selection ends on, roughly: the bar floats over it,
        // or under it when the top of the window is too close.
        let (x, y) = menu.at;
        let line = Area::new((x, y - 8.0).into(), (1.0, 28.0).into());
        notein_ui_kit::Floating::new(line, bar)
            .above()
            .centered()
            .into_element()
    }
}

/// The reference chip label of `target`.
#[must_use]
pub fn label_of(target: &SourceRef) -> String {
    let data = crate::state::LibraryView::try_get().and_then(|view| view.data());
    crate::references::display(target, data.as_deref()).label
}

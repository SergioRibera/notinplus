//! The bar over a text selection, the same in the Bible and PDFs: five
//! highlighters one tap away, a note, copy as reference and send to the
//! notebook. Notes get the editor's formatting bar instead.

use std::rc::Rc;

use freya::prelude::*;
use notein_plugin::bookmark::SourceRef;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{BarItem, Modal, ModalController, SelectionBar, width_class};

use crate::library::ItemId;

/// A kind specific button of the menu: label, icon and what it does.
pub type ExtraAction = (&'static str, Bytes, Rc<dyn Fn()>);

/// What the bar offers for one selection.
#[derive(Clone)]
pub struct SelectionMenu {
    /// Window point the selection ends at; the bar floats above it.
    pub at: (f32, f32),
    /// Index of the highlighter already on the selection.
    pub active: Option<usize>,
    /// Highlight with one of the five inks (`None` removes it).
    pub on_color: Rc<dyn Fn(Option<[u8; 3]>)>,
    /// Open the annotation for the selection.
    pub on_note: Rc<dyn Fn()>,
    /// The place as a reference, with its label, for Copiar ref.
    pub reference: Option<(SourceRef, String)>,
    /// The document the selection is in, for Al cuaderno.
    pub document: Option<ItemId>,
    /// Kind specific extras ("Marcador" in PDFs).
    pub extra: Vec<ExtraAction>,
    /// The bar went away without an action.
    pub on_dismiss: Rc<dyn Fn()>,
}

impl PartialEq for SelectionMenu {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at && self.active == other.active
    }
}

impl std::fmt::Debug for SelectionMenu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectionMenu")
            .field("at", &self.at)
            .finish_non_exhaustive()
    }
}

/// The five highlighter inks of the theme, as sRGB.
#[must_use]
pub fn inks() -> [[u8; 3]; 5] {
    palette().highlighters.map(|c| [c.r(), c.g(), c.b()])
}

impl SelectionMenu {
    /// Show the bar.
    pub fn show(self) {
        let dismiss = Rc::clone(&self.on_dismiss);
        ModalController::get().open(
            Modal::new(Bar { menu: self })
                .manual()
                .dismiss_on_backdrop(true)
                .backdrop_alpha(0)
                .on_close(move || dismiss()),
        );
    }
}

fn act(f: impl Fn() + 'static) -> impl Fn(()) + 'static {
    move |()| {
        ModalController::get().clear_top_on_close();
        ModalController::get().close();
        f();
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
        if let Some((target, label)) = menu.reference.clone() {
            items.push(BarItem::action(
                "Copiar ref",
                lucide::copy(),
                act(move || crate::references::copy(&target, &label)),
            ));
            if let Some(document) = menu.document {
                let target = menu.reference.as_ref().map(|(t, _)| t.clone());
                items.push(BarItem::action(
                    "Al cuaderno",
                    lucide::notebook_pen(),
                    act(move || {
                        if let Some(target) = target.clone() {
                            crate::references::insert_in_notebook(document, target);
                        }
                    }),
                ));
            }
        }
        for (label, icon, run) in &menu.extra {
            let run = Rc::clone(run);
            items.push(BarItem::action(*label, icon.clone(), act(move || run())));
        }
        let size = *Platform::get().root_size.read();
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
    let data = crate::state::LibraryView::get().data();
    crate::references::display(target, data.as_deref()).label
}

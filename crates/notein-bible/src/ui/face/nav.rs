//! The book and chapter picker opened from the bottom bar.

use freya::prelude::*;
use notein_i18n::tr;
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{ModalController, Sheet, tokens};

use super::Reader;

/// Books of the open Bible in the order its manifest lists them, so
/// each tradition keeps its own canon.
#[cfg_attr(feature = "sismo", sismo::function("bible::nav_books"))]
pub(super) fn open_books(reader: &Reader) {
    let books = reader.db.list_books(&reader.bible_id()).unwrap_or_default();
    let mut grid = grid();
    for book in books {
        let reader = reader.clone();
        grid = grid.child(cell(72.0, book.abbrev.clone(), move || {
            open_chapters(&reader, &book.book_id, &book.name);
        }));
    }
    Sheet::titled(tr!("reader-books-title")).child(grid).open();
}

/// Chapters of `book_id`; picking one moves the reader there.
#[cfg_attr(feature = "sismo", sismo::function("bible::nav_chapters"))]
fn open_chapters(reader: &Reader, book_id: &str, name: &str) {
    let count = reader
        .db
        .chapter_count(&reader.bible_id(), book_id)
        .unwrap_or(1);
    let mut grid = grid();
    for chapter in 1..=count {
        let reader = reader.clone();
        let book = book_id.to_owned();
        grid = grid.child(cell(48.0, chapter.to_string(), move || {
            reader.go(&book, chapter, 0);
            ModalController::get().close();
        }));
    }
    let back = reader.clone();
    Sheet::titled(name.to_owned())
        .on_back(EventHandler::new(move |()| open_books(&back)))
        .child(grid)
        .open();
}

fn grid() -> Rect {
    rect()
        .width(Size::fill())
        .horizontal()
        .spacing(8.0)
        .content(Content::wrap_spacing(8.))
}

fn cell(size: f32, text: String, mut on_press: impl FnMut() + 'static) -> Rect {
    rect()
        .width(Size::px(size))
        .height(Size::px(size))
        .center()
        .corner_radius(tokens::radius::MD)
        .background(palette().s2)
        .on_tap(move |_: Event<PressEventData>| on_press())
        .child(label().font_size(13.0).color(palette().t1).text(text))
}

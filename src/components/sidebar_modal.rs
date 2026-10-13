use std::sync::{Arc, Mutex};

use freya::animation::{AnimNum, Ease, Function, OnCreation, use_animation};
use freya::icons::lucide::{bookmark, search};
use freya::prelude::*;
use istmo::plugins::EdgeInsets;

use crate::app::ChromeState;
use crate::bookmark_ui::BookmarkSort;
use crate::canvas::{Board, lock};

use super::{FormInput, theme::*};
use super::{Modal, ModalController};

#[derive(Clone)]
pub struct SidebarModal {
    pad: EdgeInsets,
    state: ChromeState,
    board: Arc<Mutex<Board>>,
}

impl PartialEq for SidebarModal {
    fn eq(&self, other: &Self) -> bool {
        self.pad == other.pad && self.state == other.state
    }
}

impl SidebarModal {
    pub(crate) fn open(pad: EdgeInsets, state: ChromeState, board: Arc<Mutex<Board>>) {
        ModalController::get().open(
            Modal::new(Self { pad, state, board })
                .manual()
                .width(SIDEBAR_WIDTH)
                .dismiss_on_backdrop(true),
        );
    }
}

impl Component for SidebarModal {
    fn render(&self) -> impl freya::prelude::IntoElement {
        let pad = self.pad;
        let board = &self.board;
        let query = self.state.sidebar_query;
        let sort = self.state.sidebar_sort;

        let mut selected_bookmark = self.state.selected_bookmark;
        let mut body_buffer = self.state.bookmark_body;

        let pos_anim = use_animation(move |conf| {
            conf.on_creation(OnCreation::Run);
            AnimNum::new(-SIDEBAR_WIDTH, pad.left)
                .time(250)
                .ease(Ease::Out)
                .function(Function::Cubic)
        });
        let left_pos = pos_anim.get().value();

        let entries = {
            let guard = lock(board);
            let needle = query.read().trim().to_ascii_lowercase();
            let mut v = guard
                .bookmarks()
                .iter()
                .filter(|bm| {
                    if needle.is_empty() {
                        return true;
                    }
                    bm.body.to_ascii_lowercase().contains(&needle)
                })
                .cloned()
                .collect::<Vec<_>>();
            match *sort.read() {
                BookmarkSort::Oldest => v.sort_by_key(|e| e.created_at),
                BookmarkSort::Recent => v.sort_by_key(|e| std::cmp::Reverse(e.updated_at)),
            }
            v
        };

        let empty_label = if query.read().trim().is_empty() {
            "Sin marcadores. Mantén presionado en el lienzo para crear uno."
        } else {
            "Nada coincide con la búsqueda."
        };

        let sort_label = sort.read().label();
        let mut sort_toggle = sort;
        let on_sort_toggle = move |_: Event<PressEventData>| {
            let next = sort_toggle.peek().next();
            sort_toggle.set(next);
        };

        let mut header = rect()
            .direction(Direction::Horizontal)
            .cross_align(Alignment::Center)
            .main_align(Alignment::SpaceBetween)
            .width(Size::fill())
            .padding((8.0, 10.0))
            .background(Color::from_rgb(22, 22, 26))
            .child(
                rect()
                    .direction(Direction::Horizontal)
                    .cross_align(Alignment::Center)
                    .spacing(6.0)
                    .child(
                        SvgViewer::new(bookmark())
                            .fill(Color::from_rgb(220, 220, 220))
                            .width(Size::px(14.))
                            .height(Size::px(14.)),
                    )
                    .child("Marcadores"),
            )
            .child(
                rect()
                    .padding((4.0, 8.0))
                    .background(Color::from_rgb(50, 50, 56))
                    .with_corner_radius(6.0)
                    .on_press(on_sort_toggle)
                    .child(sort_label),
            );
        header = header.spacing(8.0);

        let mut list = rect()
            .direction(Direction::Vertical)
            .width(Size::fill())
            .padding((8.0, 8.0))
            .spacing(6.0);

        if entries.is_empty() {
            list = list.child(rect().padding((12.0, 12.0)).child(empty_label));
        } else {
            for entry in entries {
                let row_board = Arc::clone(board);
                let row_id = entry.id;
                let on_pick = move |_: Event<PressEventData>| {
                    let body = lock(&row_board)
                        .doc()
                        .bookmark(row_id)
                        .map(|bm| bm.body.clone())
                        .unwrap_or_default();
                    body_buffer.set(body);
                    selected_bookmark.set(Some(row_id));
                };
                let dot_color = entry
                    .color
                    .map(|[r, g, b, _]| Color::from_rgb(r, g, b))
                    .unwrap_or_else(|| Color::from_rgb(245, 158, 11));
                list = list.child(
                    rect()
                        .direction(Direction::Horizontal)
                        .cross_align(Alignment::Center)
                        .spacing(8.0)
                        .padding((8.0, 10.0))
                        .background(Color::from_rgb(34, 34, 40))
                        .with_corner_radius(6.0)
                        .on_press(on_pick)
                        .child(
                            rect()
                                .width(Size::px(10.0))
                                .height(Size::px(10.0))
                                .with_corner_radius(5.0)
                                .background(dot_color),
                        )
                        .child(
                            rect()
                                .direction(Direction::Vertical)
                                .width(Size::fill())
                                .spacing(2.0)
                                .child(
                                    entry
                                        .body
                                        .lines()
                                        .next()
                                        .map(|l| &l[..80])
                                        .unwrap_or("(sin texto)"),
                                )
                                .child(format!("{} ref(s)", entry.refs.len())),
                        ),
                );
            }
        }

        rect()
            .position(Position::new_absolute().top(self.pad.top).left(left_pos))
            .width(Size::px(SIDEBAR_WIDTH))
            .height(Size::percent(100.))
            .background(PANEL_BG)
            .overflow(Overflow::Clip)
            .color(TEXT_PRIMARY)
            .child(header)
            .child(
                FormInput::new(query)
                    .left_icon(search())
                    .placeholder("Buscar")
                    .width(Size::fill()),
            )
            .child(list)
    }
}

//! "Compartir": start sharing a document with other people, or manage
//! the share it already belongs to.
//!
//! Two steps when creating (what, then how), then the same management
//! view the sheet shows for an existing share: members, policy and
//! the invitation to hand out.

use freya::icons::lucide;
use freya::prelude::*;
use notein_sync::{ShareMode, SharePolicy};

use crate::components::theme::{
    ACCENT, BORDER, CARD_BG, ERROR, PANEL_BG, TEXT_MUTED, TEXT_PRIMARY,
};
use crate::library::{ItemId, WorkspaceId};
use crate::sync::{self, ShareRequest, ShareView};

use super::modal::{Modal, ModalController};
use super::pair_sheet::qr_bytes;

const SHEET_WIDTH: f32 = 440.0;
const QR_SIZE: f32 = 180.0;

/// Which documents go into a new share.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum What {
    Document,
    WithNotebooks,
    WholeMesa,
}

/// Builder: `ShareSheet::open(item, workspace)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShareSheet {
    item: ItemId,
    workspace: Option<WorkspaceId>,
}

impl ShareSheet {
    pub fn open(item: ItemId, workspace: Option<WorkspaceId>) {
        ModalController::get().open(
            Modal::new(Self { item, workspace })
                .center()
                .width(SHEET_WIDTH)
                .dismiss_on_backdrop(true),
        );
    }
}

impl Component for ShareSheet {
    fn render(&self) -> impl IntoElement {
        let tick = use_state(|| 0u64);
        let _ = *tick.read();
        use_hook({
            let mut tick = tick;
            move || {
                let changes = sync::changes();
                spawn(async move {
                    while changes.recv_async().await.is_ok() {
                        let next = tick.peek().wrapping_add(1);
                        tick.set(next);
                    }
                });
            }
        });
        let what = use_state(|| What::Document);
        let mode = use_state(|| ShareMode::Permanent);
        let read_only = use_state(|| false);
        let busy = use_state(|| false);

        let card = rect()
            .vertical()
            .width(Size::fill())
            .padding((20.0, 22.0))
            .spacing(12.0)
            .background(PANEL_BG)
            .with_corner_radius(16.0);

        match sync::share_of(self.item) {
            Some(view) => card.child(manage(&view)),
            None => card.child(create(
                self.item,
                self.workspace,
                what,
                mode,
                read_only,
                busy,
            )),
        }
    }
}

fn heading(text: &'static str) -> Label {
    label().color(TEXT_PRIMARY).font_size(17.0).text(text)
}

fn hint(text: impl Into<String>) -> Label {
    label().color(TEXT_MUTED).font_size(12.0).text(text.into())
}

fn divider() -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::px(1.0))
        .background(BORDER)
}

fn pill(
    text: &'static str,
    accent: Color,
    on_press: impl FnMut(Event<PressEventData>) + 'static,
) -> Rect {
    rect()
        .padding((8.0, 14.0))
        .background(CARD_BG)
        .with_corner_radius(8.0)
        .on_press(on_press)
        .child(label().color(accent).font_size(13.0).text(text))
}

fn choice_row(
    selected: bool,
    title: &'static str,
    subtitle: &'static str,
    on_press: impl FnMut(Event<PressEventData>) + 'static,
) -> Rect {
    let ring = if selected { ACCENT } else { BORDER };
    rect()
        .horizontal()
        .width(Size::fill())
        .cross_align(Alignment::Center)
        .spacing(12.0)
        .padding((6.0, 4.0))
        .on_press(on_press)
        .child(
            rect()
                .width(Size::px(18.0))
                .height(Size::px(18.0))
                .with_corner_radius(9.0)
                .border(Border::new().width(2.0).fill(ring))
                .center()
                .maybe_child(selected.then(|| {
                    rect()
                        .width(Size::px(8.0))
                        .height(Size::px(8.0))
                        .with_corner_radius(4.0)
                        .background(ACCENT)
                })),
        )
        .child(
            rect()
                .vertical()
                .width(Size::fill())
                .child(label().color(TEXT_PRIMARY).font_size(14.0).text(title))
                .child(hint(subtitle)),
        )
}

fn create(
    item: ItemId,
    workspace: Option<WorkspaceId>,
    what: State<What>,
    mode: State<ShareMode>,
    read_only: State<bool>,
    busy: State<bool>,
) -> Rect {
    let what_now = *what.read();
    let mode_now = *mode.read();
    let mut column = rect()
        .vertical()
        .width(Size::fill())
        .spacing(10.0)
        .child(heading("Compartir"))
        .child(hint("Qué"))
        .child(choice_row(
            what_now == What::Document,
            "Este documento",
            "Solo este documento.",
            {
                let mut what = what;
                move |_| what.set(What::Document)
            },
        ))
        .child(choice_row(
            what_now == What::WithNotebooks,
            "Documento y sus cuadernos",
            "El documento con todo lo escrito sobre él.",
            {
                let mut what = what;
                move |_| what.set(What::WithNotebooks)
            },
        ));
    if workspace.is_some() {
        column = column.child(choice_row(
            what_now == What::WholeMesa,
            "Toda la mesa",
            "Cada documento abierto en esta mesa, con sus cuadernos.",
            {
                let mut what = what;
                move |_| what.set(What::WholeMesa)
            },
        ));
    }
    column = column
        .child(divider())
        .child(hint("Cómo"))
        .child(choice_row(
            mode_now == ShareMode::Session,
            "Solo esta sesión",
            "Mientras estés presente. Al cerrar, nada queda del otro lado.",
            {
                let mut mode = mode;
                move |_| mode.set(ShareMode::Session)
            },
        ))
        .child(choice_row(
            mode_now == ShareMode::Permanent,
            "Permanente",
            "Cada persona guarda su copia y los cambios se encuentran cuando pueden.",
            {
                let mut mode = mode;
                move |_| mode.set(ShareMode::Permanent)
            },
        ))
        .child(divider())
        .child(choice_row(
            *read_only.read(),
            "Solo lectura",
            "Los demás miran; nadie más que vos edita.",
            {
                let mut read_only = read_only;
                move |_| {
                    let next = !*read_only.peek();
                    read_only.set(next);
                }
            },
        ));
    if let Some(error) = sync::snapshot().error {
        column = column.child(label().color(ERROR).font_size(12.0).text(error));
    }
    let is_busy = *busy.read();
    column.child(
        rect()
            .horizontal()
            .content(Content::Flex)
            .width(Size::fill())
            .spacing(10.0)
            .child(rect().width(Size::flex(1.0)))
            .child(pill("Cancelar", TEXT_MUTED, |_| {
                ModalController::get().close();
            }))
            .child(pill(
                if is_busy { "Creando…" } else { "Compartir" },
                ACCENT,
                move |_| {
                    if *busy.peek() {
                        return;
                    }
                    let mut busy = busy;
                    busy.set(true);
                    let policy = if *read_only.peek() {
                        SharePolicy::read_only()
                    } else {
                        SharePolicy::default()
                    };
                    spawn_forever(submit(item, workspace, *what.peek(), *mode.peek(), policy));
                },
            )),
    )
}

async fn submit(
    item: ItemId,
    workspace: Option<WorkspaceId>,
    what: What,
    mode: ShareMode,
    policy: SharePolicy,
) {
    let Ok(handle) = crate::home::open_library().await else {
        return;
    };
    let lib = handle.lock().await;
    let title = lib
        .item(item)
        .map_or_else(|| "Documento".to_owned(), |i| i.name.clone());
    let mut docs: Vec<ItemId> = match (what, workspace) {
        (What::WholeMesa, Some(ws)) => lib.workspace(ws).map_or_else(Vec::new, |ws| {
            ws.layout
                .leaves()
                .iter()
                .flat_map(|pane| pane.tabs.iter().filter_map(|tab| tab.item()))
                .collect()
        }),
        _ => vec![item],
    };
    if what != What::Document {
        let mut with_notebooks = docs.clone();
        for doc in &docs {
            with_notebooks.extend(lib.companions_of(*doc));
        }
        docs = with_notebooks;
    }
    docs.sort_unstable();
    docs.dedup();
    drop(lib);
    sync::create_share(ShareRequest {
        title,
        docs,
        mode,
        policy,
    });
}

fn manage(view: &ShareView) -> Rect {
    let id = view.id;
    let mode = match view.mode {
        ShareMode::Session => "Solo esta sesión",
        ShareMode::Permanent => "Permanente",
    };
    let mut column = rect()
        .vertical()
        .width(Size::fill())
        .spacing(10.0)
        .child(heading(if view.is_owner {
            "Compartido"
        } else {
            "Compartido con vos"
        }))
        .child(hint(format!(
            "{} · {mode} · {} documento{}",
            view.title,
            view.docs.len(),
            if view.docs.len() == 1 { "" } else { "s" }
        )));

    column = column.child(divider()).child(hint("Personas"));
    column = column.child(person_row(&view.owner.name, view.owner_online, true, None));
    for (member, online) in &view.members {
        let device = member.device;
        column = column.child(person_row(
            &member.name,
            *online,
            false,
            view.is_owner.then(|| {
                Box::new(move |_: Event<PressEventData>| sync::expel(id, device))
                    as Box<dyn FnMut(Event<PressEventData>)>
            }),
        ));
    }

    if view.is_owner {
        let read_only = view.policy.edit_content == notein_sync::Who::Nobody;
        column = column.child(divider()).child(choice_row(
            read_only,
            "Solo lectura",
            "Los demás miran; nadie más que vos edita.",
            move |_| {
                let policy = if read_only {
                    SharePolicy::default()
                } else {
                    SharePolicy::read_only()
                };
                sync::set_share_policy(id, policy);
            },
        ));
        column = column.child(divider()).child(hint("Invitación"));
        if let Some(svg) = qr_bytes(&view.invite_link) {
            column = column.child(
                rect()
                    .width(Size::fill())
                    .cross_align(Alignment::Center)
                    .child(
                        rect()
                            .width(Size::px(QR_SIZE))
                            .height(Size::px(QR_SIZE))
                            .with_corner_radius(12.0)
                            .background(Color::from_rgb(255, 255, 255))
                            .center()
                            .child(
                                SvgViewer::new(svg)
                                    .width(Size::px(QR_SIZE - 16.0))
                                    .height(Size::px(QR_SIZE - 16.0)),
                            ),
                    ),
            );
        }
        column = column.child(
            rect()
                .width(Size::fill())
                .padding((8.0, 12.0))
                .background(CARD_BG)
                .with_corner_radius(8.0)
                .child(
                    label()
                        .color(TEXT_MUTED)
                        .font_size(10.0)
                        .max_lines(3)
                        .text_overflow(TextOverflow::Ellipsis)
                        .text(view.invite_link.clone()),
                ),
        );
    } else if !view.can_edit {
        column = column.child(hint("Solo lectura: podés mirar, no editar."));
    }

    column.child(
        rect()
            .horizontal()
            .content(Content::Flex)
            .width(Size::fill())
            .spacing(10.0)
            .child(rect().width(Size::flex(1.0)))
            .child(pill("Cerrar", TEXT_MUTED, |_| {
                ModalController::get().close();
            }))
            .child(if view.is_owner {
                pill("Terminar", ERROR, move |_| {
                    sync::end_share(id);
                    ModalController::get().close();
                })
            } else {
                pill("Salir", ERROR, move |_| {
                    sync::leave_share(id);
                    ModalController::get().close();
                })
            }),
    )
}

fn person_row(
    name: &str,
    online: bool,
    is_owner: bool,
    on_remove: Option<Box<dyn FnMut(Event<PressEventData>)>>,
) -> Rect {
    let dot = if online {
        Color::from_rgb(90, 200, 120)
    } else {
        Color::from_rgb(90, 90, 100)
    };
    let mut row = rect()
        .horizontal()
        .content(Content::Flex)
        .width(Size::fill())
        .spacing(10.0)
        .cross_align(Alignment::Center)
        .child(
            rect()
                .width(Size::px(10.0))
                .height(Size::px(10.0))
                .with_corner_radius(5.0)
                .background(dot),
        )
        .child(
            label()
                .color(TEXT_PRIMARY)
                .font_size(13.0)
                .text(name.to_owned()),
        )
        .child(hint(if is_owner { "dueño" } else { "" }))
        .child(rect().width(Size::flex(1.0)));
    if let Some(on_remove) = on_remove {
        row = row.child(
            rect()
                .padding(6.0)
                .with_corner_radius(8.0)
                .on_press(on_remove)
                .child(
                    SvgViewer::new(lucide::x())
                        .color(TEXT_MUTED)
                        .width(Size::px(14.0))
                        .height(Size::px(14.0)),
                ),
        );
    }
    row
}

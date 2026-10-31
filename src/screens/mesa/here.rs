//! Who else has the document of a pane open, as a row of avatars.

use std::sync::Arc;

use freya::prelude::*;
use notein_plugin::{Peer, Presence};
use notein_sync::DeviceId;
use notein_ui_kit::Avatar;
use notein_ui_kit::theme::palette;

use crate::library::ItemId;
use crate::sync::presence::{self, DocPresence};

/// Avatars shown before the rest fold into `+N`.
const SHOWN: usize = 3;
const SIZE: f32 = 22.0;

/// The people in `item` with this device. Holding it is also what tells
/// them this device is here, for every kind of document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HereNow {
    pub(super) item: ItemId,
}

impl Component for HereNow {
    #[sismo::function("mesa::here_now")]
    fn render(&self) -> impl IntoElement {
        let item = self.item;
        // Kept for as long as the pane shows `item` (the parent keys this
        // by item), which is what says this device is here.
        let mine: Arc<DocPresence> = use_hook(move || presence::open(item));
        let mut people = use_state(move || mine.peers());
        use_hook(move || {
            let changes = presence::watch(item);
            spawn(traced!("task::mesa::here_people", async move {
                while changes.recv_async().await.is_ok() {
                    people.set(presence::here(item));
                }
            }));
        });
        let mut followed = use_state(presence::following);
        use_hook(move || {
            let changes = presence::watch_all();
            spawn(traced!("task::mesa::here_following", async move {
                while changes.recv_async().await.is_ok() {
                    followed.set_if_modified(presence::following());
                }
            }));
        });
        let people = people.read();
        if people.is_empty() {
            return rect().into_element();
        }
        let p = palette();
        let rest = people.len().saturating_sub(SHOWN);
        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .margin((0.0, 6.0))
            .spacing(-6.0)
            .children(people.iter().take(SHOWN).map(|peer| {
                let device = DeviceId(peer.id);
                let is_followed = *followed.read() == Some(device);
                rect()
                    .key(peer.id)
                    .corner_radius(SIZE)
                    .border(
                        Border::new()
                            .fill(if is_followed { color(peer) } else { p.bg })
                            .width(2.0)
                            .alignment(BorderAlignment::Outer),
                    )
                    .cursor(CursorIcon::Pointer)
                    // Following someone shows what they see; again, it stops.
                    .on_press(move |event: Event<PressEventData>| {
                        event.stop_propagation();
                        presence::follow((!is_followed).then_some(device));
                    })
                    .child(Avatar::new(initials(peer), color(peer)).size(SIZE))
                    .into_element()
            }))
            .maybe_child((rest > 0).then(|| {
                rect()
                    .height(Size::px(SIZE))
                    .padding((0.0, 6.0))
                    .margin((0.0, 0.0, 0.0, 8.0))
                    .corner_radius(SIZE)
                    .background(p.s2)
                    .center()
                    .child(
                        label()
                            .font_size(11.0)
                            .font_weight(FontWeight::BOLD)
                            .color(p.t2)
                            .text(format!("+{rest}")),
                    )
            }))
            .into_element()
    }
}

const fn color(peer: &Peer) -> Color {
    let [r, g, b] = peer.color;
    Color::from_rgb(r, g, b)
}

/// First letters of the first two words of their name.
fn initials(peer: &Peer) -> String {
    let letters: String = peer
        .name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() {
        "?".to_owned()
    } else {
        letters
    }
}

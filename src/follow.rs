//! Following someone: the window wears their color, a chip says whom,
//! and the documents they open open here too. Each face shows their
//! place inside a document (see `Presence::followed`).

use freya::prelude::*;
use notein_i18n::tr;
use notein_sync::DeviceId;
use notein_ui_kit::theme::palette;
use notein_ui_kit::tokens;

use crate::library::ItemId;
use crate::sync::presence;

/// Width of the colored frame around the window.
const FRAME: f32 = 3.0;

/// Mounted once at the window root, over everything.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FollowFrame {
    /// What the system bars cover at the top, so the chip clears them.
    pub top: f32,
}

impl Component for FollowFrame {
    #[sismo::function("follow::follow_frame")]
    fn render(&self) -> impl IntoElement {
        let mut followed = use_state(|| None::<(DeviceId, String, Color)>);
        use_hook(move || {
            let changes = presence::watch_all();
            spawn(traced!("task::follow::frame", async move {
                // The document this device last opened for them.
                let mut shown: Option<ItemId> = None;
                loop {
                    let now = presence::following().and_then(|device| {
                        let (item, peer) = presence::whereabouts(device)?;
                        let [r, g, b] = peer.color;
                        Some((device, item, peer.name, Color::from_rgb(r, g, b)))
                    });
                    if let Some((device, item, name, color)) = now {
                        if shown != Some(item) {
                            shown = Some(item);
                            crate::flows::open(item);
                        }
                        followed.set_if_modified(Some((device, name, color)));
                    } else {
                        shown = None;
                        followed.set_if_modified(None);
                    }
                    if changes.recv_async().await.is_err() {
                        return;
                    }
                }
            }));
        });
        let Some((_, name, color)) = followed.read().clone() else {
            return rect().into_element();
        };
        frame(&name, color, self.top)
    }
}

/// The colored frame and the chip, over a window.
#[sismo::function("follow::frame")]
fn frame(name: &str, color: Color, top: f32) -> Element {
    let p = palette();
    rect()
        .layer(Layer::Overlay)
        .position(Position::new_global().top(0.).left(0.))
        .width(Size::percent(100.))
        .height(Size::percent(100.))
        .border(
            Border::new()
                .fill(color)
                .width(FRAME)
                .alignment(BorderAlignment::Inner),
        )
        .on_global_key_down(|event: Event<KeyboardEventData>| {
            if event.key == Key::Named(NamedKey::Escape) {
                presence::follow(None);
            }
        })
        .main_align(Alignment::Start)
        .cross_align(Alignment::Center)
        .child(
            rect()
                .margin((FRAME + top, 0., 0., 0.))
                .padding((4., 6., 4., 12.))
                .corner_radius(tokens::radius::FULL)
                .background(color)
                .horizontal()
                .spacing(8.)
                .cross_align(Alignment::Center)
                .child(
                    label()
                        .font_size(12.5)
                        .font_weight(FontWeight::BOLD)
                        .color(Color::from_rgb(21, 21, 26))
                        .text(tr!("misc-follow-following", "name" => name)),
                )
                .child(
                    rect()
                        .height(Size::px(24.))
                        .padding((0., 10.))
                        .corner_radius(tokens::radius::FULL)
                        .background(p.bg.with_a(200))
                        .center()
                        .cursor(CursorIcon::Pointer)
                        .on_press(|event: Event<PressEventData>| {
                            event.stop_propagation();
                            presence::follow(None);
                        })
                        .child(
                            label()
                                .font_size(12.)
                                .color(p.t1)
                                .text(tr!("misc-follow-stop")),
                        ),
                ),
        )
        .into_element()
}

#[cfg(test)]
mod tests {
    use freya_testing::prelude::*;

    use super::*;

    fn app() -> impl IntoElement {
        rect().expanded().background(palette().bg).child(frame(
            "Ana",
            Color::from_rgb(96, 165, 250),
            0.,
        ))
    }

    #[test]
    fn frames_the_window_in_their_color() {
        let (mut runner, ()) = TestingRunner::new(app, (480., 300.).into(), |_| {}, 1.0);
        runner.sync_and_update();
        runner.render_to_file(std::env::temp_dir().join("follow_frame.png"));
    }
}

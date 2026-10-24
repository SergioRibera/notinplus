//! The + button and the two shortcuts the user pinned above it.

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Icon, tokens};

use crate::flows::create::{self, Context};
use crate::library::FolderId;
use crate::references::kind_color;
use crate::settings::SettingsStore;

/// Size of the + button.
const FAB: f32 = 60.0;
/// Size of a shortcut bubble.
const BUBBLE: f32 = 46.0;
/// Distance from the window edge.
pub const MARGIN: f32 = 24.0;

fn bubble(size: f32, fill: Color, icon: Bytes, icon_color: Color, label: String) -> Rect {
    let p = palette();
    rect()
        .width(Size::px(size))
        .height(Size::px(size))
        .corner_radius(tokens::radius::FULL)
        .background(fill)
        .shadow(Shadow::new().y(6.0).blur(18.0).color(p.shadow))
        .center()
        .a11y_role(AccessibilityRole::Button)
        .a11y_focusable(true)
        .a11y_alt(label)
        .child(Icon::new(icon).size(size * 0.42).color(icon_color))
}

/// The + button with its shortcuts, anchored bottom-right.
#[derive(Clone, Debug, PartialEq)]
pub struct Fab {
    pub folder: FolderId,
    /// Space reserved at the bottom (safe area).
    pub bottom: f32,
}

impl Component for Fab {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let shortcuts = SettingsStore::get().read().pen.fab_shortcuts;
        let folder = self.folder;
        let bottom = MARGIN + self.bottom;
        let context = move || Context {
            folder,
            selection: Vec::new(),
        };
        let bubbles = shortcuts
            .iter()
            .flatten()
            .filter_map(create::entry_for)
            .map(|entry| {
                let fill = kind_color(&p, entry.entry.color);
                let label = entry.entry.label.to_string();
                let icon = entry.entry.icon.clone();
                bubble(BUBBLE, p.s3, icon, fill, label)
                    .on_press(move |_| create::run(entry.clone(), context()))
                    .into_element()
            })
            .collect::<Vec<_>>();
        rect()
            .position(Position::new_absolute().right(MARGIN).bottom(bottom))
            .layer(Layer::Relative(10))
            .spacing(12.0)
            .cross_align(Alignment::Center)
            .children(bubbles)
            .child(
                bubble(
                    FAB,
                    p.accent,
                    lucide::plus(),
                    p.on_accent,
                    "Crear".to_owned(),
                )
                .on_press(move |_| {
                    let size = *Platform::get().root_size.read();
                    let anchor = Area::new(
                        (size.width - MARGIN - FAB, size.height - bottom - FAB).into(),
                        (FAB, FAB).into(),
                    );
                    create::open_menu(anchor, context());
                }),
            )
    }
}

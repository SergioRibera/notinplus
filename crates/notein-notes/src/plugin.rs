//! Notas as a Notein plugin: the kind and the note page.

use std::sync::Arc;

use freya::prelude::*;
use notein_sdk::face::{CoverRequest, Face, FaceContext, HeaderAction};
use notein_sdk::info::PluginInfo;
use notein_sdk::plugin::{Plugin, Registrar};
use notein_sdk::services::Host;
use notein_sdk::tr;

use crate::face;
use crate::kind::{KIND, NotesKind};

/// Nota: markdown notes with references.
#[derive(Debug, Default)]
pub struct NotesPlugin {
    kind: Arc<NotesKind>,
}

impl NotesPlugin {
    /// The kind, with the index of every note's title and headings.
    #[must_use]
    pub const fn kind(&self) -> &Arc<NotesKind> {
        &self.kind
    }
}

impl Plugin for NotesPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new(KIND, tr!("notes-plugin-name")).description(tr!("notes-plugin-description"))
    }

    fn register(&self, registrar: &mut Registrar) {
        let kind = registrar.kind(Arc::clone(&self.kind));
        registrar.face(NoteFace, kind);
    }

    fn install(&self, host: &Host) {
        self.kind.attach(host);
    }
}

/// The note page.
#[derive(Debug, Default)]
pub struct NoteFace;

impl Face for NoteFace {
    type Kind = NotesKind;

    fn header_actions(&self) -> Vec<HeaderAction> {
        face::header_actions()
    }

    fn render(&self, ctx: FaceContext<NotesKind>) -> Element {
        face::NotePage { ctx }.into_element()
    }

    /// The note's title and the start of its text.
    fn cover(&self, cover: &CoverRequest) -> Option<Element> {
        let p = notein_ui_kit::palette();
        let paper = cover
            .tint
            .map_or(p.paper, |c| Color::from_argb(c[3], c[0], c[1], c[2]));
        Some(
            rect()
                .width(Size::px(cover.width))
                .height(Size::px(cover.height))
                .background(paper)
                .padding((20.0, cover.width * 0.12))
                .spacing(6.0)
                .child(
                    label()
                        .text(cover.name.clone())
                        .font_size(11.0)
                        .font_weight(FontWeight::BOLD)
                        .color(p.paper_ink)
                        .max_lines(2),
                )
                .child(
                    label()
                        .text(cover.excerpt.clone().unwrap_or_default())
                        .font_size(8.5)
                        .line_height(1.4)
                        .color(Color::from_rgb(110, 106, 98))
                        .max_lines(9)
                        .text_overflow(TextOverflow::Ellipsis),
                )
                .into_element(),
        )
    }
}

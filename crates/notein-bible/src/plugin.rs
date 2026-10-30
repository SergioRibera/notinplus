//! Biblia as a Notein plugin: the kind, the reader and the Lora faces.

use std::sync::Arc;

use freya::prelude::*;
use notein_i18n::tr;
use notein_sdk::face::{CoverRequest, Face, FaceContext, HeaderAction};
use notein_sdk::info::PluginInfo;
use notein_sdk::plugin::{Font, Plugin, Registrar};
use notein_sdk::services::Host;

use crate::db::{Db, DbError};
use crate::kind::BibleKind;
use crate::ui::{catalog, face, typography};

/// Biblia: read, highlight and cite verses.
#[derive(Debug)]
pub struct BiblePlugin {
    kind: Arc<BibleKind>,
}

impl BiblePlugin {
    /// Open the translations database.
    ///
    /// # Errors
    /// [`DbError`] when the database cannot be opened.
    pub fn open() -> Result<Self, DbError> {
        let kind = Arc::new(BibleKind::new(Db::open_default()?, crate::new_shared()));
        catalog::register(&kind);
        Ok(Self { kind })
    }

    /// The kind, for shell code that reads Bibles headless (previews).
    #[must_use]
    pub const fn kind(&self) -> &Arc<BibleKind> {
        &self.kind
    }
}

impl Plugin for BiblePlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new(crate::kind::KIND, tr!("plugin-name"))
            .description(tr!("plugin-description"))
    }

    fn register(&self, registrar: &mut Registrar) {
        let kind = registrar.kind(Arc::clone(&self.kind));
        registrar.face(BibleFace, kind);
    }

    fn install(&self, host: &Host) {
        catalog::announce(Arc::clone(host));
    }

    fn fonts(&self) -> Vec<Font> {
        crate::fonts::FACES
            .into_iter()
            .map(|bytes| Font {
                family: crate::fonts::FAMILY,
                bytes,
            })
            .collect()
    }
}

/// The reader.
#[derive(Debug, Default)]
pub struct BibleFace;

impl Face for BibleFace {
    type Kind = BibleKind;

    fn header_actions(&self) -> Vec<HeaderAction> {
        face::header_actions()
    }

    fn render(&self, ctx: FaceContext<BibleKind>) -> Element {
        face::BiblePage { ctx }.into_element()
    }

    /// A leather cover with the translation's name.
    fn cover(&self, cover: &CoverRequest) -> Option<Element> {
        let p = notein_ui_kit::palette();
        let translation = cover
            .meta
            .split(" · ")
            .next()
            .unwrap_or_default()
            .to_uppercase();
        Some(
            rect()
                .width(Size::px(cover.width))
                .height(Size::px(cover.height))
                .background(Color::from_rgb(59, 47, 30))
                .padding((18.0, 16.0))
                .spacing(4.0)
                .child(
                    label()
                        .text(tr!("plugin-cover-title"))
                        .font_size(18.0)
                        .font_family(typography::FAMILY)
                        .color(Color::from_rgb(243, 226, 190)),
                )
                .child(
                    label()
                        .text(translation)
                        .font_size(10.0)
                        .font_weight(FontWeight::SEMI_BOLD)
                        .letter_spacing(1.0)
                        .color(p.kinds.bible),
                )
                .child(
                    rect()
                        .position(Position::new_absolute().bottom(36.0).left(16.0))
                        .width(Size::px(cover.width - 32.0))
                        .height(Size::px(2.0))
                        .background(p.kinds.bible),
                )
                .into_element(),
        )
    }
}

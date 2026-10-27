//! The translation menu of the header: installed Bibles, then the
//! catalog to add more.

use freya::prelude::*;
use notein_ui_kit::{Action, ActionEntry, ActionList};

use super::Reader;

/// Open the menu under the header button at `at`.
pub(super) fn open(reader: &Reader, at: (f32, f32)) {
    use freya::icons::lucide;
    let current = reader.bible_id();
    let mut entries: Vec<ActionEntry> = reader
        .db
        .list_bibles()
        .unwrap_or_default()
        .into_iter()
        .map(|bible| {
            let reader = reader.clone();
            let id = bible.id.clone();
            let name = if bible.name_local.is_empty() {
                bible.id.clone()
            } else {
                bible.name_local.clone()
            };
            ActionEntry::Item(
                Action::closing(name, move || reader.switch_bible(&id))
                    .subtitle(bible.abbrev.clone())
                    .checked(bible.id == current),
            )
        })
        .collect();
    entries.push(ActionEntry::Divider);
    entries.push(ActionEntry::Item(
        Action::closing("Agregar traducción", crate::ui::catalog::open).icon(lucide::plus()),
    ));
    ActionList::new(entries).show_at(Area::new(at.into(), (1.0, 1.0).into()));
}

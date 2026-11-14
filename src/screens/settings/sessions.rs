//! Settings › Sesiones: the name other people see, what a new
//! invitation does, and the lasting shares this device is in.

use freya::prelude::*;
use notin_ui_kit::{Group, InlineEdit, ListRow, Segmented, Text, TextRole, Trailing};

use notin_i18n::tr;
use notin_sync::ShareMode;

use crate::settings::{ApprovePref, EntryRolePref, SettingsStore, TrashRetention};
use crate::sync::ShareView;

/// The Sessions section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Sessions;

/// The name other people see, changed in place. Cleared, the device
/// name stands in for it.
#[sismo::function("settings::sessions_name_row")]
fn name_row(snapshot: &crate::sync::SyncSnapshot) -> InlineEdit {
    use freya::icons::lucide;
    InlineEdit::row(
        tr!("settings-sessions-name"),
        snapshot.user_name.clone().unwrap_or_default(),
        |text: String| crate::sync::set_user_name((!text.is_empty()).then_some(text)),
    )
    .placeholder(snapshot.name.clone())
    .edit_label(tr!("common-change"))
    .allow_empty()
    .icon(lucide::user_round())
    .hint(tr!("settings-sessions-name-hint", "device" => snapshot.name.as_str()))
}

/// One lasting share, as a row that opens its session sheet.
#[sismo::function("settings::sessions_share_row")]
fn share_row(share: &ShareView) -> ListRow {
    let id = share.id;
    let kind = if share.folder.is_some() {
        tr!("settings-sessions-kind-folder")
    } else {
        tr!("settings-sessions-kind-document")
    };
    let who = if share.is_owner {
        tr!("collab-you-organize")
    } else {
        tr!("collab-organized-by", "name" => share.owner.name.as_str())
    };
    ListRow::new(share.title.clone())
        .subtitle(tr!(
            "settings-sessions-share-meta",
            "kind" => kind,
            "who" => who,
            "count" => share.docs.len(),
        ))
        .leading(notin_ui_kit::Leading::Element(
            notin_ui_kit::Avatar::new(
                crate::screens::collab::initials(&share.owner.name),
                crate::screens::collab::person_color(share.owner.device),
            )
            .into_element(),
        ))
        .chevron()
        .on_press(move |_| crate::screens::collab::session(id))
}

impl Component for Sessions {
    #[sismo::function("settings::sessions")]
    fn render(&self) -> impl IntoElement {
        let snapshot = crate::sync::use_sync().read().clone();
        let mut store = SettingsStore::get();
        let settings = store.read().sessions;
        let lasting: Vec<Element> = snapshot
            .shares
            .iter()
            .filter(|s| s.mode == ShareMode::Permanent)
            .map(|s| share_row(s).into_element())
            .collect();
        let none_lasting = lasting.is_empty();
        let live = snapshot.shares.len();
        let waiting = snapshot.pending_joins.len();
        let recent = snapshot.recents.len();
        rect()
            .width(Size::fill())
            .spacing(24.0)
            .child(
                notin_ui_kit::Section::new(tr!("settings-sessions-you"))
                    .child(Group::default().child(name_row(&snapshot))),
            )
            .child(
                notin_ui_kit::Section::new(tr!("settings-sessions-defaults")).child(
                    Group::default()
                        .child(
                            ListRow::new(tr!("collab-entry-role"))
                                .subtitle(tr!("settings-sessions-entry-role-hint"))
                                .trailing(Trailing::Element(
                                    Segmented::new(
                                        EntryRolePref::ALL.map(EntryRolePref::label),
                                        EntryRolePref::ALL
                                            .iter()
                                            .position(|r| *r == settings.entry_role)
                                            .unwrap_or(1),
                                    )
                                    .on_select(move |i: usize| {
                                        store.update(|s| {
                                            s.sessions.entry_role = EntryRolePref::ALL[i];
                                        });
                                    })
                                    .into_element(),
                                )),
                        )
                        .child(
                            ListRow::new(tr!("collab-approve-entry"))
                                .subtitle(tr!("settings-sessions-approve-hint"))
                                .trailing(Trailing::Element(
                                    Segmented::new(
                                        ApprovePref::ALL.map(ApprovePref::label),
                                        ApprovePref::ALL
                                            .iter()
                                            .position(|a| *a == settings.approve)
                                            .unwrap_or(0),
                                    )
                                    .on_select(move |i: usize| {
                                        store.update(|s| s.sessions.approve = ApprovePref::ALL[i]);
                                    })
                                    .into_element(),
                                )),
                        )
                        .child(
                            ListRow::new(tr!("settings-sessions-keep-invites"))
                                .subtitle(tr!("settings-sessions-keep-invites-hint"))
                                .trailing(Trailing::Element(
                                    Segmented::new(
                                        TrashRetention::ALL.map(TrashRetention::label),
                                        TrashRetention::ALL
                                            .iter()
                                            .position(|r| *r == settings.recent_retention)
                                            .unwrap_or(1),
                                    )
                                    .on_select(move |i: usize| {
                                        store.update(|s| {
                                            s.sessions.recent_retention = TrashRetention::ALL[i];
                                        });
                                        crate::screens::collab::prune_recents();
                                    })
                                    .into_element(),
                                )),
                        ),
                ),
            )
            .child(
                notin_ui_kit::Section::new(tr!("settings-sessions-lasting"))
                    .maybe_child(none_lasting.then(|| {
                        Text::new(tr!("settings-sessions-lasting-empty")).role(TextRole::Hint)
                    }))
                    .maybe_child((!none_lasting).then(|| Group::default().children(lasting))),
            )
            .child(
                notin_ui_kit::Section::new(tr!("settings-sessions-now")).child(
                    Group::default().child(
                        ListRow::new(tr!(
                            "settings-sessions-now-summary",
                            "live" => live,
                            "waiting" => waiting,
                            "recent" => recent,
                        ))
                        .subtitle(tr!("settings-sessions-now-hint"))
                        .trailing(Trailing::Text(tr!("settings-sessions-see").into()))
                        .on_press(|_| crate::screens::collab::sessions()),
                    ),
                ),
            )
            .into_element()
    }
}

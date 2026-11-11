//! Settings › Sesiones: the name other people see, what a new
//! invitation does, and the lasting shares this device is in.

use freya::prelude::*;
use notin_ui_kit::{Group, ListRow, Segmented, Text, TextField, TextRole, Trailing};

use notin_i18n::tr;
use notin_sync::ShareMode;

use crate::settings::{ApprovePref, EntryRolePref, SettingsStore, TrashRetention};
use crate::sync::ShareView;

/// The Sessions section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Sessions;

/// The name row: shown, or being typed.
#[sismo::function("settings::sessions_name_row")]
fn name_row(
    snapshot: &crate::sync::SyncSnapshot,
    mut editing: State<bool>,
    draft: State<String>,
) -> ListRow {
    use freya::icons::lucide;
    let shown = snapshot
        .user_name
        .clone()
        .unwrap_or_else(|| snapshot.name.clone());
    if *editing.read() {
        let field = TextField::new(draft)
            .placeholder(snapshot.name.clone())
            .on_submit(move |text: String| {
                crate::sync::set_user_name(Some(text));
                editing.set(false);
            })
            .on_blur(move |()| editing.set(false));
        return ListRow::new(tr!("settings-sessions-name"))
            .subtitle(tr!("settings-sessions-name-hint", "device" => snapshot.name.as_str()))
            .trailing(Trailing::Element(field.into_element()));
    }
    ListRow::new(shown)
        .icon(lucide::user_round())
        .subtitle(tr!("settings-sessions-name-hint", "device" => snapshot.name.as_str()))
        .trailing(Trailing::Text(tr!("common-change").into()))
        .on_press(move |_| {
            let mut draft = draft;
            draft.set(snapshot_user_name());
            editing.set(true);
        })
}

fn snapshot_user_name() -> String {
    crate::sync::snapshot().user_name.unwrap_or_default()
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
        let editing = use_state(|| false);
        let draft = use_state(String::new);
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
                    .child(Group::default().child(name_row(&snapshot, editing, draft))),
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

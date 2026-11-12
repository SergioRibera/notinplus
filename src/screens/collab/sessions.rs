//! Every session this device is in, waiting for, or left lately: one
//! sheet reached from Home, Settings and the mesa bar.

use freya::prelude::*;
use notin_sync::ShareMode;
use notin_ui_kit::{
    Avatar, Button, EmptyState, Group, Leading, ListRow, ModalController, Section, Sheet,
    SheetFooter, Tag, Trailing,
};

use notin_i18n::tr;

use crate::sync::{PendingJoinView, RecentShare, ShareView, SyncSnapshot};

/// Open the sessions sheet.
#[sismo::function("collab::sessions")]
pub fn sessions() {
    sismo::marker("sheet: sessions");
    Sheet::titled(tr!("collab-sessions-title"))
        .width(600.0)
        .sticky()
        .child(Sessions)
        .footer(SheetFooter::new([
            Button::new(tr!("settings-devices-scan-or-paste"))
                .tonal()
                .on_press(|_| crate::screens::settings::open_link_sheet())
                .into_element(),
            Button::new(tr!("common-done"))
                .primary()
                .on_press(|_| ModalController::get().close())
                .into_element(),
        ]))
        .open();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sessions;

/// The state tag of a share, in words and a colour.
#[sismo::function("collab::state_tag")]
pub(super) fn state_tag(view: &ShareView, snapshot: &SyncSnapshot) -> Tag {
    let p = notin_ui_kit::theme::palette();
    let (text, dot) = state_of(view, snapshot);
    Tag::new(text).dot(dot.unwrap_or(p.t3))
}

/// What is going on in a share: live and how, paused, up to date, or
/// out of reach.
#[sismo::function("collab::state_of")]
pub(super) fn state_of(view: &ShareView, snapshot: &SyncSnapshot) -> (String, Option<Color>) {
    use notin_sync::Path;
    let p = notin_ui_kit::theme::palette();
    if view.paused() {
        return (
            tr!("collab-state-paused", "owner" => view.owner.name.as_str()),
            Some(p.warn),
        );
    }
    let here = view.anyone_here(snapshot.device);
    let path = if view.is_owner {
        view.members
            .iter()
            .filter_map(|m| m.path)
            .min_by_key(|p| match p {
                Path::Direct => 0,
                Path::Punched => 1,
                Path::Relay => 2,
            })
    } else {
        view.owner_path
    };
    match (view.mode, here, path) {
        (_, true, Some(Path::Direct) | None) => (tr!("collab-state-live-lan"), Some(p.ok)),
        (_, true, Some(Path::Punched)) => (tr!("collab-state-live-direct"), Some(p.ok)),
        (_, _, Some(Path::Relay)) if here || view.mode == ShareMode::Session => {
            (tr!("collab-state-live-relay"), Some(p.ok))
        }
        (ShareMode::Permanent, _, Some(Path::Relay)) => {
            (tr!("collab-state-up-to-date-relay"), None)
        }
        (ShareMode::Permanent, _, _) => (tr!("collab-state-up-to-date"), None),
        (ShareMode::Session, _, _) if view.is_owner => (tr!("collab-state-waiting-people"), None),
        (ShareMode::Session, _, _) => (tr!("collab-state-offline"), None),
    }
}

/// Who organizes, for the row subtitle.
fn who(view: &ShareView) -> String {
    if view.is_owner {
        tr!("collab-you-organize")
    } else {
        tr!("collab-organized-by", "name" => view.owner.name.as_str())
    }
}

#[sismo::function("collab::session_row")]
fn session_row(view: &ShareView, snapshot: &SyncSnapshot) -> ListRow {
    let id = view.id;
    let present = view.members.iter().filter(|m| m.online).count()
        + usize::from(!view.is_owner && view.owner_online);
    let mut parts = vec![who(view)];
    if present > 0 {
        parts.push(tr!("collab-people-now", "count" => present));
    }
    parts.push(if view.folder.is_some() {
        tr!("collab-kind-folder")
    } else {
        super::mode_label(view.mode)
    });
    if !view.can_edit {
        parts.push(tr!("collab-share-read-only"));
    }
    ListRow::new(view.title.clone())
        .subtitle(parts.join(" · "))
        .leading(Leading::Element(
            Avatar::new(
                super::initials(&view.owner.name),
                super::person_color(view.owner.device),
            )
            .presence(view.is_owner || view.owner_online)
            .into_element(),
        ))
        .trailing(Trailing::Element(state_tag(view, snapshot).into_element()))
        .chevron()
        .on_press(move |_| super::session(id))
}

#[sismo::function("collab::pending_row")]
fn pending_row(join: &PendingJoinView) -> ListRow {
    let share = join.share;
    ListRow::new(join.title.clone())
        .subtitle(if join.awaiting_approval {
            tr!("collab-waiting-approval", "owner" => join.owner.as_str())
        } else {
            tr!("collab-waiting-owner", "owner" => join.owner.as_str())
        })
        .leading(Leading::Element(
            Avatar::new(
                super::initials(&join.owner),
                notin_ui_kit::theme::ACCENT_SWATCHES[2],
            )
            .into_element(),
        ))
        .trailing(Trailing::Element(
            Button::new(tr!("common-cancel"))
                .ghost()
                .small()
                .on_press(move |_| crate::sync::cancel_join(share))
                .into_element(),
        ))
}

#[sismo::function("collab::recent_row")]
fn recent_row(recent: &RecentShare, now: u64) -> ListRow {
    use freya::icons::lucide;
    let id = recent.id;
    let when = crate::format::ago(recent.at, now);
    let sub = if recent.is_owner {
        tr!("collab-recent-ended", "when" => when.as_str(), "count" => recent.docs.len())
    } else {
        tr!("collab-recent-left", "when" => when.as_str(), "owner" => recent.owner.as_str())
    };
    let action = if recent.is_owner {
        Button::new(tr!("collab-recent-reopen"))
            .tonal()
            .small()
            .on_press(move |_| crate::sync::reopen(id))
    } else {
        Button::new(tr!("collab-recent-rejoin"))
            .tonal()
            .small()
            .on_press(move |_| {
                crate::sync::rejoin(id);
                ModalController::get().close();
                super::joining();
            })
    };
    ListRow::new(recent.title.clone())
        .subtitle(sub)
        .leading(Leading::Element(
            Avatar::new(
                super::initials(&recent.owner),
                notin_ui_kit::theme::ACCENT_SWATCHES[4],
            )
            .into_element(),
        ))
        .trailing(Trailing::Element(
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(4.0)
                .child(action)
                .child(
                    notin_ui_kit::IconButton::new(lucide::x(), tr!("common-remove"))
                        .on_press(move |_| crate::sync::forget_recent(id)),
                )
                .into_element(),
        ))
}

impl Component for Sessions {
    #[sismo::function("collab::render_sessions")]
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let snapshot = crate::sync::use_sync().read().clone();
        let now = crate::format::now();
        let (folders, live): (Vec<&ShareView>, Vec<&ShareView>) =
            snapshot.shares.iter().partition(|s| s.folder.is_some());
        let empty = snapshot.shares.is_empty()
            && snapshot.pending_joins.is_empty()
            && snapshot.recents.is_empty();
        if empty {
            return EmptyState::new(lucide::users(), tr!("collab-sessions-empty-title"))
                .body(tr!("collab-sessions-empty-body"))
                .into_element();
        }
        rect()
            .width(Size::fill())
            .spacing(18.0)
            .maybe_child((!live.is_empty()).then(|| {
                Section::new(tr!("collab-sessions-live", "count" => live.len())).child(
                    Group::default().children(
                        live.iter()
                            .map(|v| session_row(v, &snapshot).into_element()),
                    ),
                )
            }))
            .maybe_child((!folders.is_empty()).then(|| {
                Section::new(tr!("collab-sessions-folders", "count" => folders.len())).child(
                    Group::default().children(
                        folders
                            .iter()
                            .map(|v| session_row(v, &snapshot).into_element()),
                    ),
                )
            }))
            .maybe_child((!snapshot.pending_joins.is_empty()).then(|| {
                Section::new(tr!(
                    "collab-sessions-waiting",
                    "count" => snapshot.pending_joins.len()
                ))
                .child(
                    Group::default().children(
                        snapshot
                            .pending_joins
                            .iter()
                            .map(|j| pending_row(j).into_element()),
                    ),
                )
            }))
            .maybe_child((!snapshot.recents.is_empty()).then(|| {
                Section::new(tr!("collab-sessions-recent")).child(
                    Group::default().children(
                        snapshot
                            .recents
                            .iter()
                            .map(|r| recent_row(r, now).into_element()),
                    ),
                )
            }))
            .into_element()
    }
}

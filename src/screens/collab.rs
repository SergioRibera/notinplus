//! Working together: creating a share, the session sheet for whoever
//! is in it, the list of sessions, joining from an invitation, and what
//! happens when a session ends.

mod create;
mod session;
mod sessions;

use freya::prelude::*;
use notin_sync::{DeviceId, Removal, Role, ShareMode};
use notin_ui_kit::theme::palette;
use notin_ui_kit::{
    Button, ModalController, Notice, NoticeTone, ProgressBar, Sheet, SheetFooter, Text, TextRole,
    Toast, Toasts,
};

use notin_i18n::tr;

pub use create::{ShareTarget, share};
pub use session::{session, session_for};
pub use sessions::sessions;

use crate::session_end::SessionEnd;
use crate::sync::{LinkTarget, ShareNotice, ShareView, SyncSnapshot};

/// First letters of the first two words of a name.
#[must_use]
pub fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

/// A stable colour per device, for its avatar.
#[must_use]
pub fn person_color(device: DeviceId) -> Color {
    let swatches = notin_ui_kit::theme::ACCENT_SWATCHES;
    let hash = device.0.iter().fold(0usize, |h, b| {
        h.wrapping_mul(31).wrapping_add(usize::from(*b))
    });
    swatches[hash % swatches.len()]
}

/// What a role is called on screen.
#[must_use]
pub fn role_label(role: Role) -> String {
    match role {
        Role::Owner => tr!("collab-role-owner"),
        Role::CoOrganizer => tr!("collab-role-co-organizer"),
        Role::Editor => tr!("collab-role-editor"),
        Role::Viewer => tr!("collab-role-viewer"),
    }
}

/// What a mode is called on screen.
#[must_use]
pub fn mode_label(mode: ShareMode) -> String {
    match mode {
        ShareMode::Session => tr!("collab-mode-session"),
        ShareMode::Permanent => tr!("collab-mode-permanent"),
    }
}

#[sismo::function("collab::copy_link")]
pub(crate) fn copy_link(link: &str) {
    match freya::clipboard::Clipboard::set(link.to_owned()) {
        Ok(()) => {
            Toasts::get().show(Toast::new(tr!("collab-link-copied")));
        }
        Err(err) => log::warn!("copy invite: {err:?}"),
    }
}

#[sismo::function("collab::qr")]
pub(crate) fn qr(text: &str) -> Option<Bytes> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(160, 160)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    Some(Bytes::from(svg.into_bytes()))
}

/// Drop recents older than the retention chosen in settings.
#[sismo::function("collab::prune_recents")]
pub fn prune_recents() {
    let keep = crate::settings::SettingsStore::try_get()
        .map(|s| s.peek().sessions.recent_retention)
        .unwrap_or_default();
    if let Some(millis) = keep.millis() {
        crate::sync::prune_recents(std::time::Duration::from_millis(millis));
    }
}

/// The invitation card for a link: who invites, to what, and how.
#[sismo::function("collab::invitation")]
pub fn invitation(link: String) {
    match crate::sync::recognize_link(&link) {
        Some(LinkTarget::Join {
            title,
            owner,
            mode,
            role,
            ..
        }) => {
            Sheet::default()
                .child(
                    rect()
                        .width(Size::fill())
                        .cross_align(Alignment::Center)
                        .spacing(10.0)
                        .padding((8.0, 0.0))
                        .child(
                            notin_ui_kit::Avatar::new(
                                initials(&owner),
                                notin_ui_kit::theme::ACCENT_SWATCHES[0],
                            )
                            .size(56.0),
                        )
                        .child(
                            Text::new(tr!("collab-invite-from", "owner" => owner.as_str()))
                                .role(TextRole::Meta),
                        )
                        .child(
                            Text::new(title)
                                .role(TextRole::Heading)
                                .align(TextAlign::Center),
                        )
                        .child(
                            rect()
                                .horizontal()
                                .spacing(6.0)
                                .child(
                                    notin_ui_kit::Tag::new(match mode {
                                        ShareMode::Session => tr!("collab-mode-session"),
                                        ShareMode::Permanent => {
                                            tr!("collab-mode-permanent-copy")
                                        }
                                    })
                                    .icon(freya::icons::lucide::users()),
                                )
                                .child(notin_ui_kit::Tag::new(
                                    tr!("collab-invite-as", "role" => role_label(role)),
                                )),
                        ),
                )
                .footer(SheetFooter::new([
                    Button::new(tr!("collab-not-now"))
                        .ghost()
                        .on_press(|_| ModalController::get().close())
                        .into_element(),
                    Button::new(tr!("collab-join"))
                        .primary()
                        .on_press(move |_| {
                            ModalController::get().close();
                            if let Some(LinkTarget::Join { .. }) = crate::sync::open_link(&link) {
                                joining();
                            }
                        })
                        .into_element(),
                ]))
                .open();
        }
        Some(LinkTarget::AlreadyIn { share, .. }) => session(share),
        Some(LinkTarget::Pair { name }) => {
            Sheet::titled(tr!("collab-pair-title", "name" => name.as_str()))
                .child(Text::new(tr!("collab-pair-hint")).role(TextRole::Hint))
                .footer(SheetFooter::new([
                    Button::new(tr!("collab-not-now"))
                        .ghost()
                        .on_press(|_| ModalController::get().close())
                        .into_element(),
                    Button::new(tr!("collab-pair"))
                        .primary()
                        .on_press(move |_| {
                            ModalController::get().close();
                            if crate::sync::open_link(&link).is_some() {
                                Toasts::get().show(Toast::new(
                                    tr!("collab-pairing-toast", "name" => name.as_str()),
                                ));
                            }
                        })
                        .into_element(),
                ]))
                .open();
        }
        None => {}
    }
}

/// The "Joining…" sheet, following the join until it lands, waits for
/// an organizer, or fails.
#[sismo::function("collab::joining")]
pub fn joining() {
    Sheet::default().without_close().child(Joining).open();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Joining;

/// Why the join did not work, and Got it.
#[sismo::function("collab::join_failed")]
fn join_failed(failure: notin_sync::JoinFailure) -> Element {
    let why = match failure {
        notin_sync::JoinFailure::Refused => tr!("collab-join-refused"),
        notin_sync::JoinFailure::Unreachable => tr!("collab-join-unreachable"),
    };
    rect()
        .width(Size::fill())
        .spacing(14.0)
        .child(Notice::new(NoticeTone::Danger, tr!("collab-join-failed")).detail(why))
        .child(
            rect()
                .width(Size::fill())
                .horizontal()
                .main_align(Alignment::End)
                .child(
                    Button::new(tr!("collab-got-it"))
                        .primary()
                        .on_press(|_| ModalController::get().close()),
                ),
        )
        .into_element()
}

/// The owner heard us and an organizer has to say yes: the wait can
/// go on without this sheet.
#[sismo::function("collab::join_waiting")]
fn join_waiting(title: &str, owner: &str) -> Element {
    rect()
        .width(Size::fill())
        .spacing(14.0)
        .child(
            Notice::new(
                NoticeTone::Info,
                tr!("collab-waiting-approval", "owner" => owner),
            )
            .detail(tr!("collab-waiting-approval-hint", "title" => title)),
        )
        .child(
            rect()
                .width(Size::fill())
                .horizontal()
                .main_align(Alignment::End)
                .child(
                    Button::new(tr!("common-done"))
                        .primary()
                        .on_press(|_| ModalController::get().close()),
                ),
        )
        .into_element()
}

/// The share being joined: its title, and how many documents arrived.
#[sismo::function("collab::join_progress")]
fn join_progress(snapshot: &SyncSnapshot, joined: Option<&ShareView>) -> (String, usize, usize) {
    joined.map_or_else(
        || {
            (
                snapshot
                    .pending_joins
                    .first()
                    .map(|j| j.title.clone())
                    .unwrap_or_default(),
                0,
                0,
            )
        },
        |view| {
            let done = view
                .docs
                .iter()
                .filter(|d| snapshot.docs.get(d) == Some(&notin_sync::DocStatus::Synced))
                .count();
            (view.title.clone(), done, view.docs.len())
        },
    )
}

impl Component for Joining {
    fn render(&self) -> impl IntoElement {
        sismo::scope!("collab::render_joining");
        let snapshot = crate::sync::use_sync().read().clone();
        // Shares that existed before this join; the new one is the other.
        let known = use_hook(|| {
            crate::sync::snapshot()
                .shares
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>()
        });
        if let Some(failure) = snapshot
            .last_join_failure
            .filter(|_| snapshot.pending_joins.is_empty())
        {
            return join_failed(failure);
        }
        if let Some(waiting) = snapshot.pending_joins.iter().find(|j| j.awaiting_approval) {
            return join_waiting(&waiting.title, &waiting.owner);
        }
        let joined = snapshot
            .shares
            .iter()
            .find(|s| !s.is_owner && !known.contains(&s.id));
        let (title, done, total) = join_progress(&snapshot, joined);
        if let Some(view) = joined
            && done == total
        {
            ModalController::get().close();
            if view.desk && !view.docs.is_empty() {
                open_desk(view.title.clone(), view.docs.clone());
            } else if let Some(item) = view.docs.first().copied() {
                crate::flows::open(item);
            }
        }
        #[allow(clippy::cast_precision_loss)] // document counts
        let fraction = if total == 0 {
            0.05
        } else {
            done as f32 / total as f32
        };
        rect()
            .width(Size::fill())
            .spacing(12.0)
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(8.0)
                    .child(notin_ui_kit::Spinner::default())
                    .child(
                        Text::new(if title.is_empty() {
                            tr!("collab-joining")
                        } else {
                            tr!("collab-joining-title", "title" => title.as_str())
                        })
                        .role(TextRole::Strong),
                    ),
            )
            .child(ProgressBar::new(fraction))
            .child(
                Text::new(if total == 0 {
                    tr!("collab-joining-searching")
                } else {
                    tr!("collab-joining-progress", "done" => done, "total" => total)
                })
                .role(TextRole::Meta),
            )
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .main_align(Alignment::End)
                    .child(
                        Button::new(tr!("common-cancel"))
                            .ghost()
                            .on_press(move |_| {
                                let snapshot = crate::sync::snapshot();
                                if let Some(view) = snapshot
                                    .shares
                                    .iter()
                                    .find(|s| !s.is_owner && !known.contains(&s.id))
                                {
                                    crate::sync::leave_share(view.id);
                                }
                                for pending in &snapshot.pending_joins {
                                    crate::sync::cancel_join(pending.share);
                                }
                                ModalController::get().close();
                            }),
                    ),
            )
            .into_element()
    }
}

/// A desk shared with this device: a desk of its own with every
/// document as a tab, then go there.
#[sismo::function("collab::open_desk")]
fn open_desk(title: String, docs: Vec<crate::library::ItemId>) {
    spawn_in_window(async move {
        let Ok(handle) = crate::library::shared().await else {
            log::warn!("shared desk: library unavailable");
            return;
        };
        let workspace = {
            let mut lib = handle.lock().await;
            let workspace = match lib.create_workspace(&title, None).await {
                Ok(id) => id,
                Err(err) => {
                    log::warn!("shared desk: create: {err}");
                    return;
                }
            };
            let pane = lib
                .workspace(workspace)
                .and_then(|w| w.layout.leaves().first().map(|p| p.id));
            if let Some(pane) = pane {
                for doc in docs {
                    if let Err(err) = lib
                        .add_tab(workspace, pane, crate::library::TabTarget::Item(doc))
                        .await
                    {
                        log::warn!("shared desk: open {doc}: {err}");
                    }
                }
            }
            workspace
        };
        crate::space::notify_changed();
        let _ = crate::app::global_router().push(crate::route::Route::Space { id: workspace });
    });
}

/// A permanent share is no longer shared with this device; its copy
/// stays in the library as the person's own.
#[sismo::function("collab::share_stopped")]
pub fn share_stopped(title: &str, reason: Removal) {
    let message = match reason {
        Removal::Expelled => tr!("collab-unshared-expelled", "title" => title),
        Removal::Ended | Removal::Left => tr!("collab-unshared-by-owner", "title" => title),
    };
    Toasts::get().show(Toast::new(message));
}

/// "The session ended": keep a copy of what was open, or let it go.
#[sismo::function("collab::session_ended")]
pub fn session_ended(end: &SessionEnd) {
    let count = end.docs.len();
    let body = match end.reason {
        Removal::Ended => tr!("collab-ended-by-owner", "count" => count),
        Removal::Expelled => tr!("collab-ended-expelled", "count" => count),
        Removal::Left => tr!("collab-ended-left", "count" => count),
    };
    let keep = end.clone();
    let close = end.clone();
    Sheet::titled(tr!("collab-ended-title", "title" => end.title.as_str()))
        .without_close()
        .child(Text::new(body))
        .footer(SheetFooter::new([
            Button::new(tr!("common-close"))
                .ghost()
                .on_press(move |_| {
                    ModalController::get().close();
                    crate::session_end::resolve(close.clone());
                })
                .into_element(),
            Button::new(tr!("collab-ended-save-copy"))
                .primary()
                .icon(freya::icons::lucide::download())
                .on_press(move |_| {
                    ModalController::get().close();
                    save_copy(keep.clone());
                })
                .into_element(),
        ]))
        .push();
}

/// Keep a copy of the session's documents, then let the session go.
#[sismo::function("collab::save_copy")]
pub(crate) fn save_copy(end: SessionEnd) {
    spawn_in_window(async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let mut saved = 0usize;
        {
            let mut lib = handle.lock().await;
            for item in &end.docs {
                match lib.duplicate_item(*item).await {
                    Ok(_) => saved += 1,
                    Err(err) => log::warn!("session copy {item}: {err}"),
                }
            }
        }
        Toasts::get().show(Toast::new(tr!("collab-ended-saved", "count" => saved)));
        crate::session_end::resolve(end);
    });
}

/// Turn share notices into toasts, from the app root.
#[sismo::function("collab::listen")]
pub fn listen() {
    let notices = crate::sync::notices();
    spawn_in_window(async move {
        while let Ok(notice) = notices.recv_async().await {
            sismo::scope!("collab::notice");
            show_notice(notice);
        }
    });
}

#[sismo::function("collab::show_notice")]
fn show_notice(notice: ShareNotice) {
    let p = palette();
    let _ = p;
    let mut toasts = Toasts::get();
    match notice {
        ShareNotice::Joined { title, who, .. } => {
            toasts.show(Toast::new(
                tr!("collab-toast-joined", "who" => who.as_str(), "title" => title.as_str()),
            ));
        }
        ShareNotice::Left { title, who, .. } => {
            toasts.show(Toast::new(
                tr!("collab-toast-left", "who" => who.as_str(), "title" => title.as_str()),
            ));
        }
        ShareNotice::Renamed { title, .. } => {
            toasts.show(Toast::new(
                tr!("collab-toast-renamed", "title" => title.as_str()),
            ));
        }
        ShareNotice::EntryRequested { share, title, who } => {
            toasts.show(
                Toast::new(
                    tr!("collab-toast-wants-in", "who" => who.as_str(), "title" => title.as_str()),
                )
                .action(tr!("common-see"), move |()| session(share)),
            );
        }
        ShareNotice::Entered { share, title } => {
            toasts.show(
                Toast::new(tr!("collab-toast-entered", "title" => title.as_str()))
                    .action(tr!("common-open"), move |()| session(share)),
            );
        }
        ShareNotice::Refused { title, .. } => {
            toasts.show(
                Toast::new(tr!("collab-toast-refused", "title" => title.as_str()))
                    .tone(notin_ui_kit::ToastTone::Error),
            );
        }
        ShareNotice::AlreadyIn { share, title } => {
            toasts.show(
                Toast::new(tr!("collab-already-in", "title" => title.as_str()))
                    .action(tr!("common-open"), move |()| session(share)),
            );
        }
    }
}

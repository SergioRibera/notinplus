//! One session, for whoever is in it: what it is, who is here and how,
//! the documents, and the way out. Organizers also get the invitation
//! and the controls.

use std::time::Instant;

use freya::prelude::*;
use notin_sync::{DeviceId, Member, Role, ShareId, ShareMode};
use notin_ui_kit::theme::palette;
use notin_ui_kit::{
    Action, ActionEntry, ActionList, Avatar, Button, ButtonVariant, Group, HeaderTitle, Icon,
    InlineEdit, Leading, ListRow, ModalController, Notice, NoticeTone, SearchField, Section,
    Segmented, Sheet, SheetFooter, Tag, Text, TextRole, Trailing, tokens,
};

use notin_i18n::tr;

use crate::library::ItemId;
use crate::session_end::SessionEnd;
use crate::state::LibraryView;
use crate::sync::{MemberView, ShareView, SyncSnapshot};

/// Time a destructive person action waits for its second tap.
const CONFIRM_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
/// Side of the invitation QR.
const QR: f32 = 136.0;

/// Open the session sheet of share `id`.
#[sismo::function("collab::session")]
pub fn session(id: ShareId) {
    sismo::marker("sheet: session");
    Sheet::default()
        .title(HeaderTitle::element(SessionTitle { id }))
        .width(600.0)
        .sticky()
        .child(SessionSheet { id })
        .footer(SessionFooter { id })
        .open();
}

/// The session holding any of `docs`, if there is one.
#[sismo::function("collab::session_for")]
pub fn session_for(docs: &[ItemId]) {
    let snapshot = crate::sync::snapshot();
    if let Some(view) = snapshot
        .shares
        .iter()
        .find(|s| docs.iter().any(|d| s.docs.contains(d)))
    {
        session(view.id);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SessionSheet {
    id: ShareId,
}

/// The session's name in the sheet header, renamed in place by those
/// who may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SessionTitle {
    id: ShareId,
}

impl Component for SessionTitle {
    #[sismo::function("collab::render_session_title")]
    fn render(&self) -> impl IntoElement {
        let snapshot = crate::sync::use_sync().read().clone();
        let Some(view) = snapshot.shares.iter().find(|s| s.id == self.id) else {
            return rect().into_element();
        };
        let id = view.id;
        InlineEdit::title(view.title.clone(), move |text: String| {
            crate::sync::rename_share(id, text);
        })
        .edit_label(tr!("common-rename"))
        .enabled(view.can_rename)
        .into_element()
    }
}

/// Reach a member: on the same network, straight across the internet,
/// through the server, or not.
fn path_label(path: Option<notin_sync::Path>) -> Option<String> {
    path.map(super::path_label)
}

/// What a person row says under the name.
#[sismo::function("collab::person_status")]
fn person_status(role: Role, online: bool, path: Option<notin_sync::Path>, me: bool) -> String {
    let mut parts = vec![super::role_label(role)];
    if !me {
        match (online, path_label(path)) {
            (true, Some(p)) => parts.push(p),
            (false, Some(p)) => parts.push(tr!("collab-person-reachable", "path" => p)),
            (_, None) => parts.push(tr!("collab-person-offline-short")),
        }
    }
    parts.join(" · ")
}

/// The roles `actor` may hand out to someone below it.
fn roles_below(actor: Role) -> Vec<Role> {
    [Role::Viewer, Role::Editor, Role::CoOrganizer]
        .into_iter()
        .filter(|r| *r < actor)
        .collect()
}

/// A small sheet listing the roles `view` may give `device`, plus the
/// way out of the session for them.
#[sismo::function("collab::role_sheet")]
fn role_sheet(view: &ShareView, member: &Member) {
    let id = view.id;
    let device = member.device;
    let current = member.role;
    let mut entries: Vec<ActionEntry> = roles_below(view.my_role)
        .into_iter()
        .map(|role| {
            let label = super::role_label(role);
            let mut action = Action::closing(label, move || {
                crate::sync::set_role(id, device, role);
            });
            if role == current {
                action = action.icon(freya::icons::lucide::check());
            }
            ActionEntry::Item(action)
        })
        .collect();
    entries.push(ActionEntry::Divider);
    entries.push(ActionEntry::Item(
        Action::closing(tr!("collab-person-remove"), move || {
            crate::sync::expel(id, device);
        })
        .icon(freya::icons::lucide::user_round_x())
        .danger(),
    ));
    Sheet::titled(member.name.clone())
        .child(ActionList::new(entries))
        .push();
}

/// Let someone waiting in with one of the roles below one's own.
#[sismo::function("collab::approve_sheet")]
fn approve_sheet(view: &ShareView, member: &Member) {
    let id = view.id;
    let device = member.device;
    let entries: Vec<ActionEntry> = roles_below(view.my_role)
        .into_iter()
        .map(|role| {
            ActionEntry::Item(Action::closing(
                tr!("collab-let-in-as", "role" => super::role_label(role)),
                move || crate::sync::approve(id, device, role),
            ))
        })
        .collect();
    Sheet::titled(tr!("collab-wants-in", "who" => member.name.as_str()))
        .child(ActionList::new(entries))
        .push();
}

impl SessionSheet {
    #[sismo::function("collab::session_tags")]
    fn tags(view: &ShareView, snapshot: &SyncSnapshot) -> Element {
        let p = palette();
        rect()
            .width(Size::fill())
            .horizontal()
            .spacing(6.0)
            // Phones run out of width before the tags do.
            .content(Content::wrap_spacing(6.0))
            .child(super::sessions::state_tag(view, snapshot))
            .child(Tag::new(if view.folder.is_some() {
                tr!("collab-kind-folder")
            } else {
                super::mode_label(view.mode)
            }))
            .child(Tag::new(
                tr!("collab-share-document-count", "count" => view.docs.len()),
            ))
            .child(
                Tag::new(if view.is_owner {
                    tr!("collab-you-organize")
                } else {
                    tr!("collab-organized-by", "name" => view.owner.name.as_str())
                })
                .dot(p.accent),
            )
            .into_element()
    }

    /// QR, link, copy, and the role the link grants.
    #[sismo::function("collab::session_invitation")]
    fn invitation(view: &ShareView) -> Element {
        use freya::icons::lucide;
        let p = palette();
        let id = view.id;
        let link = view.invite_link.clone();
        let copy = link.clone();
        let roles = [Role::Viewer, Role::Editor, Role::CoOrganizer];
        let entry = label_and_control(
            tr!("collab-entry-role"),
            Segmented::new(
                roles.map(super::role_label),
                roles
                    .iter()
                    .position(|r| *r == view.entry_role)
                    .unwrap_or(1),
            )
            .on_select(move |i: usize| {
                crate::sync::set_entry_role(id, roles[i]);
            }),
        );
        let hints = rect()
            .width(Size::fill())
            .spacing(8.0)
            .child(Text::new(tr!("collab-invitation-hint")).role(TextRole::Hint))
            // No relay in the link: guests have to be nearby.
            .maybe_child(
                (!view.invite_relayed)
                    .then(|| Text::new(tr!("collab-invitation-lan-only")).role(TextRole::Meta)),
            );
        // Beside the QR on wide sheets; under it on phones, where the
        // column next to the QR is too narrow for the roles.
        let compact = notin_ui_kit::width_class().is_compact();
        let mut column = rect()
            .width(Size::flex(1.0))
            .spacing(8.0)
            .child(
                rect()
                    .width(Size::fill())
                    .padding((8.0, 10.0))
                    .corner_radius(tokens::radius::SM)
                    .background(p.s2)
                    .child(Text::new(link.clone()).role(TextRole::Hint).lines(2)),
            )
            .child(
                rect().horizontal().spacing(8.0).child(
                    Button::new(tr!("collab-copy-link"))
                        .tonal()
                        .small()
                        .icon(lucide::copy())
                        .on_press(move |_| super::copy_link(&copy)),
                ),
            );
        let mut below = Vec::new();
        if compact {
            below.push(entry.into_element());
            below.push(hints.into_element());
        } else {
            column = column.child(entry).child(hints);
        }
        Section::new(tr!("collab-invitation"))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .spacing(14.0)
                    .maybe_child(super::qr(&link).map(|svg| {
                        rect()
                            .width(Size::px(QR))
                            .height(Size::px(QR))
                            .corner_radius(tokens::radius::LG)
                            .background(Color::WHITE)
                            .center()
                            .child(
                                SvgViewer::new(svg)
                                    .width(Size::px(QR - 12.0))
                                    .height(Size::px(QR - 12.0)),
                            )
                    }))
                    .child(column),
            )
            .children(below)
            .into_element()
    }

    /// Approval and who may add documents; the owner's call.
    #[sismo::function("collab::session_rules")]
    fn rules(view: &ShareView) -> Element {
        let id = view.id;
        Group::default()
            .child(
                ListRow::new(tr!("collab-approve-entry"))
                    .subtitle(tr!("collab-approve-entry-hint"))
                    .trailing(Trailing::Switch {
                        on: view.approve_entry,
                        on_toggle: EventHandler::new(move |on| {
                            crate::sync::set_approve_entry(id, on);
                        }),
                    }),
            )
            .child(row_with_choice(
                ListRow::new(tr!("collab-who-adds-docs"))
                    .subtitle(tr!("collab-who-adds-docs-hint")),
                Segmented::new(
                    [tr!("collab-adds-organizers"), tr!("collab-adds-editors")],
                    usize::from(view.editors_add_docs),
                )
                .on_select(move |i: usize| {
                    crate::sync::set_editors_add_docs(id, i == 1);
                }),
            ))
            .into_element()
    }

    /// People waiting for an organizer.
    #[sismo::function("collab::session_pending")]
    fn pending(view: &ShareView) -> Element {
        let id = view.id;
        Section::new(tr!("collab-pending-people", "count" => view.pending.len()))
            .child(Group::default().children(view.pending.iter().map(|member| {
                let device = member.device;
                let view = view.clone();
                let who = member.clone();
                ListRow::new(member.name.clone())
                    .subtitle(tr!("collab-wants-in-from", "device" => member.device_name.as_str()))
                    .leading(Leading::Element(
                        Avatar::new(super::initials(&member.name), super::person_color(device))
                            .into_element(),
                    ))
                    .trailing(Trailing::Element(
                        rect()
                            .horizontal()
                            .spacing(6.0)
                            .child(
                                Button::new(tr!("collab-refuse"))
                                    .ghost()
                                    .small()
                                    .on_press(move |_| crate::sync::refuse(id, device)),
                            )
                            .child(
                                Button::new(tr!("collab-let-in"))
                                    .primary()
                                    .small()
                                    .on_press(move |_| approve_sheet(&view, &who)),
                            )
                            .into_element(),
                    ))
                    .into_element()
            })))
            .into_element()
    }

    /// Everyone in the session, with what organizers may do to each.
    #[sismo::function("collab::session_people")]
    fn people(view: &ShareView, me: Option<DeviceId>) -> Element {
        let owner_me = Some(view.owner.device) == me;
        let owner_name = view.owner.full_name();
        let owner_row = ListRow::new(if owner_me {
            tr!("collab-person-me", "name" => owner_name.as_str())
        } else {
            owner_name
        })
        .subtitle(person_status(
            Role::Owner,
            view.owner_online || view.is_owner,
            view.owner_path,
            owner_me,
        ))
        .leading(Leading::Element(
            Avatar::new(
                super::initials(&view.owner.name),
                super::person_color(view.owner.device),
            )
            .presence(view.owner_online || view.is_owner)
            .into_element(),
        ));
        let rows = view.members.iter().map(|m: &MemberView| {
            let mine = Some(m.member.device) == me;
            let can_touch = !mine && view.outranks(m.member.device);
            let role = m.member.role;
            let name = m.member.full_name();
            let row = ListRow::new(if mine {
                tr!("collab-person-me", "name" => name.as_str())
            } else {
                name
            })
            .subtitle(person_status(role, m.online || mine, m.path, mine))
            .leading(Leading::Element(
                Avatar::new(
                    super::initials(&m.member.name),
                    super::person_color(m.member.device),
                )
                .presence(m.online || mine)
                .into_element(),
            ));
            if can_touch {
                let view = view.clone();
                let member = m.member.clone();
                row.trailing(Trailing::Element(
                    Button::new(super::role_label(role))
                        .ghost()
                        .small()
                        .icon(freya::icons::lucide::chevron_down())
                        .on_press(move |_| role_sheet(&view, &member))
                        .into_element(),
                ))
                .into_element()
            } else {
                row.into_element()
            }
        });
        Section::new(tr!(
            "collab-session-people",
            "count" => view.members.len() + 1,
        ))
        .child(Group::default().child(owner_row).children(rows))
        .into_element()
    }

    /// The documents, with the way to add and remove for those who may.
    #[sismo::function("collab::session_docs")]
    fn docs(view: &ShareView) -> Element {
        let id = view.id;
        let data = LibraryView::get().data();
        let others = crate::sync::snapshot().shares;
        let rows = view.docs.iter().map(|item| {
            let item = *item;
            let (name, kind) = data.as_ref().and_then(|d| d.item(item)).map_or_else(
                || (tr!("collab-doc-on-its-way"), String::new()),
                |i| {
                    (
                        i.name.clone(),
                        crate::plugins::kind_info(&i.kind.plugin_id)
                            .map_or_else(String::new, |k| k.label.into_owned()),
                    )
                },
            );
            let also: Vec<String> = others
                .iter()
                .filter(|s| s.id != id && s.docs.contains(&item))
                .map(|s| s.title.clone())
                .collect();
            let sub = if also.is_empty() {
                kind
            } else {
                tr!("collab-doc-also-in", "kind" => kind.as_str(), "titles" => also.join(", "))
            };
            let row = ListRow::new(name).subtitle(sub).on_press(move |_| {
                ModalController::get().close();
                crate::flows::open(item);
            });
            if view.can_add_docs && view.docs.len() > 1 {
                row.trailing(Trailing::Element(
                    Button::new(tr!("collab-doc-remove"))
                        .ghost()
                        .small()
                        .on_press(move |_| crate::sync::remove_docs(id, &[item]))
                        .into_element(),
                ))
                .into_element()
            } else {
                row.into_element()
            }
        });
        let add = view.can_add_docs.then(|| {
            let view = view.clone();
            ListRow::new(tr!("collab-doc-add"))
                .subtitle(tr!("collab-doc-add-hint"))
                .icon(freya::icons::lucide::plus())
                .on_press(move |_| add_docs_sheet(&view))
        });
        Section::new(tr!("collab-session-docs", "count" => view.docs.len()))
            .child(Group::default().children(rows).maybe_child(add))
            .into_element()
    }

    /// One line on what is going on.
    #[sismo::function("collab::session_status")]
    fn status(view: &ShareView) -> String {
        if view.paused() {
            return tr!("collab-status-paused", "owner" => view.owner.name.as_str());
        }
        let here: Vec<String> = view
            .members
            .iter()
            .filter(|m| m.online)
            .map(|m| m.member.name.clone())
            .chain((!view.is_owner && view.owner_online).then(|| view.owner.name.clone()))
            .collect();
        let who = match here.len() {
            0 => tr!("collab-status-nobody"),
            1 => tr!("collab-status-one", "name" => here[0].as_str()),
            _ => tr!("collab-status-many", "names" => here.join(", ")),
        };
        if view.is_owner && view.mode == ShareMode::Session {
            format!("{who} {}", tr!("collab-status-owner-pause-hint"))
        } else {
            who
        }
    }
}

impl Component for SessionSheet {
    #[sismo::function("collab::render_session")]
    fn render(&self) -> impl IntoElement {
        let snapshot = crate::sync::use_sync().read().clone();
        let Some(view) = snapshot.shares.iter().find(|s| s.id == self.id).cloned() else {
            // Gone while open: ended, left or expelled. The end sheet, if
            // any, takes over.
            ModalController::get().close();
            return rect().into_element();
        };
        let me = snapshot.device;
        rect()
            .width(Size::fill())
            .spacing(18.0)
            .child(Self::tags(&view, &snapshot))
            .child(Text::new(Self::status(&view)).role(TextRole::Meta))
            .maybe_child(view.paused().then(|| {
                Notice::new(
                    NoticeTone::Warning,
                    tr!("collab-paused-notice", "owner" => view.owner.name.as_str()),
                )
                .detail(tr!("collab-paused-notice-hint"))
            }))
            .maybe_child(view.can_manage.then(|| Self::invitation(&view)))
            .maybe_child(view.is_owner.then(|| Self::rules(&view)))
            .maybe_child(
                (view.can_manage && !view.pending.is_empty()).then(|| Self::pending(&view)),
            )
            .child(Self::people(&view, me))
            .child(Self::docs(&view))
            .into_element()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SessionFooter {
    id: ShareId,
}

impl Component for SessionFooter {
    #[sismo::function("collab::render_session_footer")]
    fn render(&self) -> impl IntoElement {
        let snapshot = crate::sync::use_sync().read().clone();
        let confirming = use_state(|| None::<Instant>);
        let Some(view) = snapshot.shares.iter().find(|s| s.id == self.id).cloned() else {
            return rect().into_element();
        };
        let id = view.id;
        // The way out sits on the left; the header × closes.
        if view.is_owner {
            return SheetFooter::new([])
                .leading(end_button(id, view.mode, confirming))
                .into_element();
        }
        let leave = leave_button(&view, confirming);
        let mut buttons = Vec::new();
        if view.paused() {
            let end = SessionEnd {
                title: view.title,
                docs: view.docs,
                reason: notin_sync::Removal::Left,
            };
            buttons.push(
                Button::new(tr!("collab-ended-save-copy"))
                    .tonal()
                    .on_press(move |_| {
                        ModalController::get().close();
                        crate::sync::leave_share(id);
                        super::save_copy(end.clone());
                    })
                    .into_element(),
            );
        }
        SheetFooter::new(buttons).leading(leave).into_element()
    }
}

/// The owner's way out. A session ends for everyone; a permanent share
/// only stops being shared, every member keeps its copy, so that one
/// asks for a second tap first.
#[sismo::function("collab::end_button")]
fn end_button(id: ShareId, mode: ShareMode, mut confirming: State<Option<Instant>>) -> Element {
    let armed = confirming
        .read()
        .is_some_and(|at| at.elapsed() < CONFIRM_WINDOW);
    if mode == ShareMode::Session {
        return Button::new(tr!("collab-session-end"))
            .danger()
            .on_press(move |_| {
                crate::sync::end_share(id);
                ModalController::get().close();
                notin_ui_kit::Toasts::get()
                    .show(notin_ui_kit::Toast::new(tr!("collab-session-ended-toast")));
            })
            .into_element();
    }
    Button::new(if armed {
        tr!("collab-unshare-confirm")
    } else {
        tr!("collab-unshare")
    })
    .variant(if armed {
        ButtonVariant::DangerFill
    } else {
        ButtonVariant::Danger
    })
    .on_press(move |_| {
        if !armed {
            confirming.set(Some(Instant::now()));
            return;
        }
        crate::sync::end_share(id);
        ModalController::get().close();
        notin_ui_kit::Toasts::get().show(notin_ui_kit::Toast::new(tr!("collab-unshared-toast")));
    })
    .into_element()
}

/// A member's way out: a one-off session asks whether to keep a copy
/// afterwards; a permanent share leaves the copy as the person's own.
#[sismo::function("collab::leave_button")]
fn leave_button(view: &ShareView, mut confirming: State<Option<Instant>>) -> Element {
    let id = view.id;
    let armed = confirming
        .read()
        .is_some_and(|at| at.elapsed() < CONFIRM_WINDOW);
    let label = match (view.mode, armed) {
        (ShareMode::Permanent, false) => tr!("collab-leave-keep-copy"),
        (ShareMode::Session, false) => tr!("collab-leave"),
        (_, true) => tr!("collab-leave-confirm"),
    };
    let end = SessionEnd {
        title: view.title.clone(),
        docs: view.docs.clone(),
        reason: notin_sync::Removal::Left,
    };
    let mode = view.mode;
    Button::new(label)
        .variant(if armed {
            ButtonVariant::DangerFill
        } else {
            ButtonVariant::Danger
        })
        .on_press(move |_| {
            if !armed {
                confirming.set(Some(Instant::now()));
                return;
            }
            ModalController::get().close();
            crate::sync::leave_share(id);
            if mode == ShareMode::Session {
                super::session_ended(&end);
            }
        })
        .into_element()
}

/// Pick documents of the library to bring into `view`.
#[sismo::function("collab::add_docs_sheet")]
fn add_docs_sheet(view: &ShareView) {
    let id = view.id;
    let inside = view.docs.clone();
    Sheet::titled(tr!("collab-doc-add"))
        .child(AddDocs { id, inside })
        .push();
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AddDocs {
    id: ShareId,
    inside: Vec<ItemId>,
}

impl Component for AddDocs {
    #[sismo::function("collab::render_add_docs")]
    fn render(&self) -> impl IntoElement {
        let query = use_state(String::new);
        let Some(data) = LibraryView::get().data() else {
            return rect().into_element();
        };
        let id = self.id;
        let needle = query.read().trim().to_lowercase();
        let rows: Vec<Element> = data
            .live_items()
            .filter(|i| !self.inside.contains(&i.id))
            .filter(|i| needle.is_empty() || i.name.to_lowercase().contains(&needle))
            .take(60)
            .map(|i| {
                let item = i.id;
                let kind = crate::plugins::kind_info(&i.kind.plugin_id);
                let row = ListRow::new(i.name.clone()).chevron().on_press(move |_| {
                    crate::sync::add_docs(id, vec![item]);
                    ModalController::get().close();
                });
                match kind {
                    Some(k) => row
                        .kind(k.icon, crate::references::kind_color(&palette(), k.color))
                        .into_element(),
                    None => row.into_element(),
                }
            })
            .collect();
        let empty = rows.is_empty();
        rect()
            .width(Size::fill())
            .spacing(12.0)
            .child(SearchField::new(query, tr!("collab-doc-search")).height(38.0))
            .maybe_child((!empty).then(|| Group::default().children(rows)))
            .maybe_child(empty.then(|| {
                Text::new(tr!("collab-doc-none"))
                    .role(TextRole::Hint)
                    .color(palette().t3)
            }))
            .child(Icon::new(freya::icons::lucide::info()).size(0.0))
            .into_element()
    }
}

/// A label and its control on one line, or the control under the label
/// on phones, where both do not fit side by side.
fn label_and_control(label: String, control: Segmented) -> Element {
    if notin_ui_kit::width_class().is_compact() {
        return rect()
            .width(Size::fill())
            .spacing(8.0)
            .child(Text::new(label).role(TextRole::Strong))
            .child(control.full_width())
            .into_element();
    }
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::Flex)
        .spacing(8.0)
        .child(Text::new(label).role(TextRole::Strong).fill())
        .child(control)
        .into_element()
}

/// A settings row with a choice at its end, or under it on phones.
fn row_with_choice(row: ListRow, choice: Segmented) -> Element {
    if notin_ui_kit::width_class().is_compact() {
        return rect()
            .width(Size::fill())
            .child(row)
            .child(
                rect()
                    .width(Size::fill())
                    .padding((0.0, 12.0, 10.0, 12.0))
                    .child(choice.full_width()),
            )
            .into_element();
    }
    row.trailing(Trailing::Element(choice.into_element()))
        .into_element()
}

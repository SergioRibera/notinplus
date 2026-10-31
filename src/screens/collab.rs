//! Working together: the share sheet, joining from an invitation, and
//! what happens when a session ends.

use std::time::{Duration, Instant};

use freya::prelude::*;
use notein_sync::{DeviceId, Removal, ShareId, ShareMode, SharePolicy};
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, ButtonVariant, Group, Icon, ListRow, ModalController, Notice, NoticeTone, Person,
    PresenceList, ProgressBar, SearchField, Section, Segmented, Sheet, SheetFooter, Text, TextRole,
    Toast, Toasts, Trailing, tokens, width_class,
};

use notein_i18n::tr;

use crate::library::{ItemId, LibraryData, WorkspaceId};
use crate::session_end::SessionEnd;
use crate::state::LibraryView;
use crate::sync::{LinkTarget, ShareRequest, ShareView, SyncSnapshot};

/// Time a person action waits for its second tap.
const CONFIRM_WINDOW: Duration = Duration::from_secs(3);
/// Side of the invitation QR.
const QR: f32 = 168.0;

/// What a share covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    Document,
    WithNotebooks,
    Mesa,
}

/// Where the share sheet was opened from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShareTarget {
    pub item: Option<ItemId>,
    pub mesa: Option<WorkspaceId>,
}

impl ShareTarget {
    fn docs(self, scope: Scope, data: &LibraryData) -> Vec<ItemId> {
        match (scope, self.item, self.mesa) {
            (Scope::Mesa, _, Some(mesa)) => data.workspace(mesa).map_or_else(Vec::new, |w| {
                let mut docs: Vec<ItemId> = w
                    .layout
                    .leaves()
                    .into_iter()
                    .flat_map(|p| p.tabs.iter())
                    .filter_map(crate::library::PaneTab::item)
                    .collect();
                docs.dedup();
                docs
            }),
            (Scope::WithNotebooks, Some(item), _) => std::iter::once(item)
                .chain(data.live_notebooks_of(item))
                .collect(),
            (_, Some(item), _) => vec![item],
            (_, None, _) => Vec::new(),
        }
    }

    fn title(self, scope: Scope, data: &LibraryData) -> String {
        match (scope, self.item, self.mesa) {
            (Scope::Mesa, _, Some(mesa)) => data.workspace_title(mesa),
            (_, Some(item), _) => data.item(item).map_or_else(String::new, |i| i.name.clone()),
            (_, None, _) => String::new(),
        }
    }
}

/// The share of `docs` this device already runs, if any.
#[sismo::function("collab::existing")]
fn existing(docs: &[ItemId]) -> Option<ShareView> {
    crate::sync::snapshot()
        .shares
        .into_iter()
        .find(|s| s.is_owner && docs.iter().all(|d| s.docs.contains(d)))
}

/// Open the share sheet.
#[sismo::function("collab::share")]
pub fn share(target: ShareTarget) {
    let scope = if target.item.is_some() {
        Scope::Document
    } else {
        Scope::Mesa
    };
    let scope = State::create_in_scope(scope, ScopeId::ROOT);
    let permanent = State::create_in_scope(false, ScopeId::ROOT);
    let read_only = State::create_in_scope(false, ScopeId::ROOT);
    let created = State::create_in_scope(None::<ShareId>, ScopeId::ROOT);
    if let Some(data) = LibraryView::get().data()
        && let Some(view) = existing(&target.docs(*scope.peek(), &data))
    {
        let mut created = created;
        created.set(Some(view.id));
    }
    let form = ShareForm {
        target,
        scope,
        permanent,
        read_only,
        created,
    };
    Sheet::titled(tr!("common-share"))
        .width(600.0)
        .sticky()
        .child(form)
        .footer(ShareFooter {
            target,
            scope,
            permanent,
            read_only,
            created,
        })
        .open();
}

#[derive(Clone, Copy, PartialEq)]
struct ShareForm {
    target: ShareTarget,
    scope: State<Scope>,
    permanent: State<bool>,
    read_only: State<bool>,
    created: State<Option<ShareId>>,
}

impl std::fmt::Debug for ShareForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareForm")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

#[sismo::function("collab::card")]
fn card(
    on: bool,
    icon: Bytes,
    title: String,
    sub: String,
    enabled: bool,
    on_pick: impl Fn() + 'static,
) -> Element {
    let p = palette();
    rect()
        .width(Size::flex(1.0))
        .height(Size::fill())
        .padding(12.0)
        .corner_radius(tokens::radius::LG)
        .background(if on { p.accent_soft } else { p.s2 })
        .border(
            Border::new()
                .fill(if on { p.accent } else { p.line })
                .width(if on { 1.5 } else { 1.0 })
                .alignment(BorderAlignment::Inner),
        )
        .spacing(6.0)
        .opacity(if enabled { 1.0 } else { 0.45 })
        .a11y_role(AccessibilityRole::RadioButton)
        .a11y_alt(title.clone())
        .maybe(enabled, |r| r.on_tap(move |_| on_pick()))
        .child(
            Icon::new(icon)
                .size(18.0)
                .color(if on { p.accent_text } else { p.t2 }),
        )
        .child(Text::new(title).role(TextRole::Strong))
        .child(Text::new(sub).role(TextRole::Meta))
        .into_element()
}

impl ShareForm {
    /// What to share: the document, with its notebooks, or the mesa.
    fn what(&self, data: &LibraryData) -> Section {
        use freya::icons::lucide;
        let target = self.target;
        let scope = self.scope;
        let current = *scope.read();
        let notebooks = target.item.map_or(0, |i| data.live_notebooks_of(i).len());
        let mesa_docs = target
            .mesa
            .map_or(0, |_| target.docs(Scope::Mesa, data).len());
        let doc_name = target
            .item
            .and_then(|i| data.item(i))
            .map_or_else(String::new, |i| i.name.clone());
        let choose = move |chosen: Scope| {
            move || {
                let mut scope = scope;
                scope.set(chosen);
            }
        };
        Section::new(tr!("collab-share-what")).child(
            rect()
                .width(Size::fill())
                .height(Size::px(112.0))
                .horizontal()
                .content(Content::Flex)
                .spacing(10.0)
                .child(card(
                    current == Scope::Document,
                    lucide::file_text(),
                    tr!("collab-share-scope-document"),
                    doc_name,
                    target.item.is_some(),
                    choose(Scope::Document),
                ))
                .child(card(
                    current == Scope::WithNotebooks,
                    lucide::notebook_pen(),
                    tr!("collab-share-scope-notebooks"),
                    tr!("collab-share-notebook-count", "count" => notebooks),
                    notebooks > 0,
                    choose(Scope::WithNotebooks),
                ))
                .child(card(
                    current == Scope::Mesa,
                    lucide::layout_grid(),
                    tr!("collab-share-scope-desk"),
                    tr!("collab-share-document-count", "count" => mesa_docs),
                    mesa_docs > 0,
                    choose(Scope::Mesa),
                )),
        )
    }

    /// For this session or for good, and whether guests may write.
    fn how(&self) -> [Element; 2] {
        let (mut permanent, mut read_only) = (self.permanent, self.read_only);
        let lasting = *permanent.read();
        [
            Section::new(tr!("collab-share-how"))
                .child(
                    Segmented::new(
                        [tr!("collab-mode-session"), tr!("collab-mode-permanent")],
                        usize::from(lasting),
                    )
                    .full_width()
                    .on_select(move |i: usize| permanent.set(i == 1)),
                )
                .child(
                    Text::new(if lasting {
                        tr!("collab-share-permanent-hint")
                    } else {
                        tr!("collab-share-session-hint")
                    })
                    .role(TextRole::Hint),
                )
                .into_element(),
            Group::default()
                .child(
                    ListRow::new(tr!("collab-share-read-only"))
                        .subtitle(tr!("collab-share-read-only-hint"))
                        .trailing(Trailing::Switch {
                            on: *read_only.read(),
                            on_toggle: EventHandler::new(move |on| read_only.set(on)),
                        }),
                )
                .into_element(),
        ]
    }
}

impl Component for ShareForm {
    fn render(&self) -> impl IntoElement {
        sismo::scope!("collab::render_shareform");
        let snapshot = crate::sync::use_sync().read().clone();
        let Some(data) = LibraryView::get().data() else {
            return rect().into_element();
        };
        if let Some(id) = *self.created.read() {
            return snapshot.shares.iter().find(|s| s.id == id).map_or_else(
                || {
                    rect()
                        .padding(20.0)
                        .child(Text::new(tr!("collab-share-preparing")).role(TextRole::Meta))
                        .into_element()
                },
                |view| Session { view: view.clone() }.into_element(),
            );
        }
        rect()
            .width(Size::fill())
            .spacing(18.0)
            .child(self.what(&data))
            .children(self.how())
            .maybe_child(
                snapshot
                    .trusted
                    .is_empty()
                    .then(|| Notice::new(NoticeTone::Info, tr!("collab-share-relay-hint"))),
            )
            .into_element()
    }
}

#[derive(Clone, Copy, PartialEq)]
struct ShareFooter {
    target: ShareTarget,
    scope: State<Scope>,
    permanent: State<bool>,
    read_only: State<bool>,
    created: State<Option<ShareId>>,
}

impl std::fmt::Debug for ShareFooter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareFooter").finish_non_exhaustive()
    }
}

impl Component for ShareFooter {
    fn render(&self) -> impl IntoElement {
        sismo::scope!("collab::render_sharefooter");
        use freya::icons::lucide;
        let sync = crate::sync::use_sync();
        let snapshot = sync.read().clone();
        let confirming = use_state(|| false);
        let mut created = self.created;
        if let Some(id) = *created.read() {
            let view = snapshot.shares.iter().find(|s| s.id == id).cloned();
            let link = view
                .as_ref()
                .map(|v| v.invite_link.clone())
                .unwrap_or_default();
            return SheetFooter::new([
                end_button(id, view.as_ref().map(|v| v.mode), confirming),
                Button::new(tr!("common-done"))
                    .ghost()
                    .on_press(|_| ModalController::get().close())
                    .into_element(),
                Button::new(tr!("collab-copy-link"))
                    .primary()
                    .icon(lucide::copy())
                    .on_press(move |_| copy_link(&link))
                    .into_element(),
            ])
            .into_element();
        }
        let Some(data) = LibraryView::get().data() else {
            return rect().into_element();
        };
        let target = self.target;
        let scope = *self.scope.read();
        let docs = target.docs(scope, &data);
        let title = target.title(scope, &data);
        let mode = if *self.permanent.read() {
            ShareMode::Permanent
        } else {
            ShareMode::Session
        };
        let policy = if *self.read_only.read() {
            SharePolicy::read_only()
        } else {
            SharePolicy::default()
        };
        let enabled = !docs.is_empty();
        SheetFooter::new([
            Button::new(tr!("common-cancel"))
                .ghost()
                .on_press(|_| ModalController::get().close())
                .into_element(),
            Button::new(tr!("collab-share-create"))
                .primary()
                .icon(lucide::users())
                .enabled(enabled)
                .on_press(move |_| {
                    crate::sync::create_share(ShareRequest {
                        title: title.clone(),
                        docs: docs.clone(),
                        mode,
                        policy,
                    });
                    let docs = docs.clone();
                    spawn(async move {
                        let changes = crate::sync::changes();
                        loop {
                            if let Some(view) = existing(&docs) {
                                created.set(Some(view.id));
                                break;
                            }
                            if changes.recv_async().await.is_err() {
                                break;
                            }
                        }
                    });
                })
                .into_element(),
        ])
        .into_element()
    }
}

#[sismo::function("collab::copy_link")]
fn copy_link(link: &str) {
    match freya::clipboard::Clipboard::set(link.to_owned()) {
        Ok(()) => {
            Toasts::get().show(Toast::new(tr!("collab-link-copied")));
        }
        Err(err) => log::warn!("copy invite: {err:?}"),
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

fn person_color(device: DeviceId) -> Color {
    let swatches = notein_ui_kit::theme::ACCENT_SWATCHES;
    let hash = format!("{device:?}").bytes().fold(0usize, |h, b| {
        h.wrapping_mul(31).wrapping_add(usize::from(b))
    });
    swatches[hash % swatches.len()]
}

/// What a person may do in a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Owner,
    Editor,
    Viewer,
}

impl Role {
    fn label(self) -> String {
        match self {
            Self::Owner => tr!("collab-role-owner"),
            Self::Editor => tr!("collab-role-editor"),
            Self::Viewer => tr!("collab-role-viewer"),
        }
    }
}

/// One person of a session, in display order.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Participant {
    device: DeviceId,
    name: String,
    online: bool,
    me: bool,
    role: Role,
}

#[sismo::function("collab::participants")]
fn participants(view: &ShareView, me: Option<DeviceId>) -> Vec<Participant> {
    let read_only = view.policy.edit_content == notein_sync::Who::Nobody;
    let mut people: Vec<Participant> = std::iter::once(Participant {
        device: view.owner.device,
        name: view.owner.name.clone(),
        online: view.owner_online || view.is_owner,
        me: Some(view.owner.device) == me,
        role: Role::Owner,
    })
    .chain(view.members.iter().map(|(m, online)| Participant {
        device: m.device,
        name: m.name.clone(),
        online: *online || Some(m.device) == me,
        me: Some(m.device) == me,
        role: if read_only || view.viewers.contains(&m.device) {
            Role::Viewer
        } else {
            Role::Editor
        },
    }))
    .collect();
    people.sort_by_key(|p| (!p.me, p.role == Role::Viewer, !p.online));
    people
}

/// The active session: invitation and people.
#[derive(Clone, Debug, PartialEq)]
struct Session {
    view: ShareView,
}

impl Component for Session {
    fn render(&self) -> impl IntoElement {
        sismo::scope!("collab::render_session");
        use freya::icons::lucide;
        let p = palette();
        let me = crate::sync::snapshot().device;
        let view = &self.view;
        let people = participants(view, me);
        let capacity = if width_class().is_compact() { 4 } else { 7 };
        let all = people.clone();
        let list = PresenceList::new(
            people
                .iter()
                .map(|person| Person {
                    initials: initials(&person.name).into(),
                    name: if person.me {
                        tr!("collab-person-me", "name" => person.name.as_str()).into()
                    } else {
                        person.name.clone().into()
                    },
                    status: person.role.label().into(),
                    color: person_color(person.device),
                    live: person.online,
                })
                .collect(),
            capacity,
        )
        .on_more({
            let id = view.id;
            let is_owner = view.is_owner;
            move |()| people_sheet(id, all.clone(), is_owner)
        });
        let link = view.invite_link.clone();
        rect()
            .width(Size::fill())
            .spacing(18.0)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .spacing(16.0)
                    .maybe_child(qr(&link).map(|svg| {
                        rect()
                            .width(Size::px(QR))
                            .height(Size::px(QR))
                            .corner_radius(tokens::radius::LG)
                            .background(Color::WHITE)
                            .center()
                            .child(
                                SvgViewer::new(svg)
                                    .width(Size::px(QR - 14.0))
                                    .height(Size::px(QR - 14.0)),
                            )
                    }))
                    .child(
                        rect()
                            .width(Size::flex(1.0))
                            .spacing(8.0)
                            .child(
                                rect()
                                    .horizontal()
                                    .cross_align(Alignment::Center)
                                    .spacing(6.0)
                                    .child(Icon::new(lucide::circle_check()).size(16.0).color(p.ok))
                                    .child(
                                        Text::new(tr!("collab-invite-ready"))
                                            .role(TextRole::Strong),
                                    ),
                            )
                            .child(Text::new(view.title.clone()).role(TextRole::Title).lines(2))
                            .child(
                                Text::new(match view.mode {
                                    ShareMode::Session => tr!("collab-mode-session"),
                                    ShareMode::Permanent => tr!("collab-mode-permanent"),
                                })
                                .role(TextRole::Meta),
                            )
                            .child(
                                rect()
                                    .width(Size::fill())
                                    .padding((8.0, 10.0))
                                    .corner_radius(tokens::radius::SM)
                                    .background(p.s2)
                                    .child(Text::new(link.clone()).role(TextRole::Hint).lines(2)),
                            ),
                    ),
            )
            .child(
                Section::new(tr!(
                    "collab-session-people",
                    "count" => self.view.members.len() + 1,
                ))
                .child(list),
            )
    }
}

#[sismo::function("collab::qr")]
fn qr(text: &str) -> Option<Bytes> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(160, 160)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    Some(Bytes::from(svg.into_bytes()))
}

#[sismo::function("collab::people_sheet")]
fn people_sheet(share: ShareId, people: Vec<Participant>, is_owner: bool) {
    Sheet::titled(tr!("collab-people-title", "count" => people.len()))
        .child(People {
            share,
            participants: people,
            is_owner,
        })
        .push();
}

#[derive(Clone, Debug, PartialEq)]
struct People {
    share: ShareId,
    participants: Vec<Participant>,
    is_owner: bool,
}

impl Component for People {
    fn render(&self) -> impl IntoElement {
        sismo::scope!("collab::render_people");
        let p = palette();
        let query = use_state(String::new);
        let mut armed = use_state(|| None::<(String, Instant)>);
        let text = query.read().trim().to_lowercase();
        let share = self.share;
        let rows: Vec<Element> = self
            .participants
            .iter()
            .filter(|person| text.is_empty() || person.name.to_lowercase().contains(&text))
            .map(|person| {
                let device = person.device;
                let key = format!("{device:?}");
                let removing = armed
                    .read()
                    .as_ref()
                    .is_some_and(|(k, at)| *k == key && at.elapsed() < CONFIRM_WINDOW);
                let status = match (person.role, person.online) {
                    (Role::Owner, _) => Role::Owner.label(),
                    (role, true) => tr!("collab-person-online", "role" => role.label()),
                    (role, false) => tr!("collab-person-offline", "role" => role.label()),
                };
                let name = if person.me {
                    tr!("collab-person-me", "name" => person.name.as_str())
                } else {
                    person.name.clone()
                };
                let row =
                    ListRow::new(name)
                        .subtitle(status)
                        .leading(notein_ui_kit::Leading::Element(
                            notein_ui_kit::Avatar::new(
                                initials(&person.name),
                                person_color(device),
                            )
                            .presence(person.online)
                            .into_element(),
                        ));
                let row = if self.is_owner && person.role != Role::Owner {
                    let viewer = person.role == Role::Viewer;
                    row.trailing(Trailing::Element(
                        rect()
                            .horizontal()
                            .spacing(6.0)
                            .child(
                                Button::new(if viewer {
                                    tr!("collab-person-allow-edit")
                                } else {
                                    tr!("collab-person-view-only")
                                })
                                .ghost()
                                .small()
                                .on_press(move |_| crate::sync::set_viewer(share, device, !viewer)),
                            )
                            .child(
                                Button::new(if removing {
                                    tr!("collab-person-confirm")
                                } else {
                                    tr!("collab-person-remove")
                                })
                                .variant(if removing {
                                    ButtonVariant::DangerFill
                                } else {
                                    ButtonVariant::Ghost
                                })
                                .small()
                                .on_press(move |_| {
                                    if removing {
                                        armed.set(None);
                                        crate::sync::expel(share, device);
                                    } else {
                                        armed.set(Some((key.clone(), Instant::now())));
                                    }
                                }),
                            )
                            .into_element(),
                    ))
                } else {
                    row
                };
                row.into_element()
            })
            .collect();
        let empty = rows.is_empty();
        rect()
            .width(Size::fill())
            .spacing(12.0)
            .child(SearchField::new(query, tr!("collab-people-search")).height(38.0))
            .child(Group::default().children(rows))
            .maybe_child(empty.then(|| {
                Text::new(tr!("collab-people-empty"))
                    .role(TextRole::Hint)
                    .color(p.t3)
            }))
    }
}

/// The invitation card for a link: who invites, to what, and how.
#[sismo::function("collab::invitation")]
pub fn invitation(link: String) {
    match crate::sync::recognize_link(&link) {
        Some(LinkTarget::Join {
            title, owner, mode, ..
        }) => {
            Sheet::default()
                .child(
                    rect()
                        .width(Size::fill())
                        .cross_align(Alignment::Center)
                        .spacing(10.0)
                        .padding((8.0, 0.0))
                        .child(
                            notein_ui_kit::Avatar::new(
                                initials(&owner),
                                notein_ui_kit::theme::ACCENT_SWATCHES[0],
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
                            notein_ui_kit::Tag::new(match mode {
                                ShareMode::Session => tr!("collab-mode-session"),
                                ShareMode::Permanent => tr!("collab-mode-permanent-copy"),
                            })
                            .icon(freya::icons::lucide::users()),
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
                            if crate::sync::open_link(&link).is_some() {
                                joining();
                            }
                        })
                        .into_element(),
                ]))
                .open();
        }
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

/// The "Joining…" sheet, following the join until it lands or fails.
#[sismo::function("collab::joining")]
pub fn joining() {
    Sheet::default().without_close().child(Joining).open();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Joining;

/// Why the join did not work, and Got it.
#[sismo::function("collab::join_failed")]
fn join_failed(failure: notein_sync::JoinFailure) -> Element {
    let why = match failure {
        notein_sync::JoinFailure::Refused => {
            tr!("collab-join-refused")
        }
        notein_sync::JoinFailure::Unreachable => {
            tr!("collab-join-unreachable")
        }
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

/// The share being joined: its title, and how many documents arrived.
#[sismo::function("collab::join_progress")]
fn join_progress(snapshot: &SyncSnapshot, joined: Option<&ShareView>) -> (String, usize, usize) {
    joined.map_or_else(
        || {
            (
                snapshot.pending_joins.first().cloned().unwrap_or_default(),
                0,
                0,
            )
        },
        |view| {
            let done = view
                .docs
                .iter()
                .filter(|d| snapshot.docs.get(d) == Some(&notein_sync::DocStatus::Synced))
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
        let joined = snapshot
            .shares
            .iter()
            .find(|s| !s.is_owner && !known.contains(&s.id));
        let (title, done, total) = join_progress(&snapshot, joined);
        if let Some(view) = joined
            && done == total
        {
            let first = view.docs.first().copied();
            ModalController::get().close();
            if let Some(item) = first {
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
                    .child(notein_ui_kit::Spinner::default())
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
                                if let Some(view) = crate::sync::snapshot()
                                    .shares
                                    .into_iter()
                                    .find(|s| !s.is_owner && !known.contains(&s.id))
                                {
                                    crate::sync::leave_share(view.id);
                                }
                                ModalController::get().close();
                            }),
                    ),
            )
            .into_element()
    }
}

/// The owner's way out of a share it just created. A session ends for
/// everyone; a permanent share only stops being shared, every member
/// keeps its copy, so that one asks for a second tap first.
#[sismo::function("collab::end_button")]
fn end_button(id: ShareId, mode: Option<ShareMode>, mut confirming: State<bool>) -> Element {
    if mode != Some(ShareMode::Permanent) {
        return Button::new(tr!("collab-session-end"))
            .danger()
            .on_press(move |_| {
                crate::sync::end_share(id);
                ModalController::get().close();
                Toasts::get().show(Toast::new(tr!("collab-session-ended-toast")));
            })
            .into_element();
    }
    let armed = *confirming.read();
    Button::new(if armed {
        tr!("collab-unshare-confirm")
    } else {
        tr!("collab-unshare")
    })
    .danger()
    .on_press(move |_| {
        if !*confirming.peek() {
            confirming.set(true);
            return;
        }
        crate::sync::end_share(id);
        ModalController::get().close();
        Toasts::get().show(Toast::new(tr!("collab-unshared-toast")));
    })
    .into_element()
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

#[sismo::function("collab::save_copy")]
fn save_copy(end: SessionEnd) {
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

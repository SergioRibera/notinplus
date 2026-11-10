//! The share sheet: what to share, for how long, who gets in and how.
//! Opening it on something already shared goes to that session instead.

use freya::prelude::*;
use notein_sync::{Role, ShareMode};
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, Group, Icon, ListRow, ModalController, Notice, NoticeTone, Section, Segmented, Sheet,
    SheetFooter, Text, TextRole, Trailing, tokens,
};

use notein_i18n::tr;

use crate::library::{FolderId, ItemId, LibraryData, WorkspaceId};
use crate::settings::{EntryRolePref, SettingsStore};
use crate::state::LibraryView;
use crate::sync::ShareRequest;

/// What a share covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    Document,
    WithNotebooks,
    Mesa,
    Folder,
}

/// Where the share sheet was opened from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ShareTarget {
    pub item: Option<ItemId>,
    pub mesa: Option<WorkspaceId>,
    pub folder: Option<FolderId>,
}

impl ShareTarget {
    fn docs(self, scope: Scope, data: &LibraryData) -> Vec<ItemId> {
        match (scope, self.item, self.mesa, self.folder) {
            (Scope::Mesa, _, Some(mesa), _) => data.workspace(mesa).map_or_else(Vec::new, |w| {
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
            (Scope::Folder, _, _, Some(folder)) => data
                .live_items()
                .filter(|i| folder_contains(data, folder, i.folder))
                .map(|i| i.id)
                .collect(),
            (Scope::WithNotebooks, Some(item), _, _) => std::iter::once(item)
                .chain(data.live_notebooks_of(item))
                .collect(),
            (_, Some(item), _, _) => vec![item],
            (_, None, _, _) => Vec::new(),
        }
    }

    fn title(self, scope: Scope, data: &LibraryData) -> String {
        match (scope, self.item, self.mesa, self.folder) {
            (Scope::Mesa, _, Some(mesa), _) => data.workspace_title(mesa),
            (Scope::Folder, _, _, Some(folder)) => data
                .folder(folder)
                .map_or_else(String::new, |f| f.name.clone()),
            (_, Some(item), _, _) => data.item(item).map_or_else(String::new, |i| i.name.clone()),
            (_, None, _, _) => String::new(),
        }
    }

    /// The scope the sheet opens on.
    const fn first_scope(self) -> Scope {
        if self.folder.is_some() {
            Scope::Folder
        } else if self.item.is_some() {
            Scope::Document
        } else {
            Scope::Mesa
        }
    }
}

/// Whether `inner` is `folder` or sits below it.
fn folder_contains(data: &LibraryData, folder: FolderId, inner: FolderId) -> bool {
    let mut cursor = inner;
    for _ in 0..=data
        .folders_of(crate::library::ROOT_FOLDER)
        .len()
        .saturating_add(64)
    {
        if cursor == folder {
            return true;
        }
        if cursor == crate::library::ROOT_FOLDER {
            return false;
        }
        match data.folder(cursor) {
            Some(f) => cursor = f.parent,
            None => return false,
        }
    }
    false
}

/// The share already holding everything `target` points at, if any.
#[sismo::function("collab::existing_share")]
fn existing(target: ShareTarget, data: &LibraryData) -> Option<crate::sync::ShareView> {
    let snapshot = crate::sync::snapshot();
    if let Some(folder) = target.folder
        && let Some(view) = snapshot.shares.iter().find(|s| s.folder == Some(folder))
    {
        return Some(view.clone());
    }
    let docs = target.docs(target.first_scope(), data);
    if docs.is_empty() {
        return None;
    }
    snapshot
        .shares
        .into_iter()
        .find(|s| docs.iter().all(|d| s.docs.contains(d)))
}

/// Open the share sheet for `target`, or the session it is already in.
#[sismo::function("collab::share")]
pub fn share(target: ShareTarget) {
    let Some(data) = LibraryView::get().data() else {
        return;
    };
    if let Some(view) = existing(target, &data) {
        super::session(view.id);
        return;
    }
    let defaults = SettingsStore::try_get()
        .map(|s| s.peek().sessions)
        .unwrap_or_default();
    let scope = State::create_in_scope(target.first_scope(), ScopeId::ROOT);
    let permanent = State::create_in_scope(target.folder.is_some(), ScopeId::ROOT);
    let entry_role = State::create_in_scope(defaults.entry_role, ScopeId::ROOT);
    let approve = State::create_in_scope(None::<bool>, ScopeId::ROOT);
    let form = ShareForm {
        target,
        scope,
        permanent,
        entry_role,
        approve,
    };
    Sheet::titled(tr!("common-share"))
        .width(600.0)
        .sticky()
        .child(form)
        .footer(ShareFooter { form })
        .open();
}

#[derive(Clone, Copy, PartialEq)]
struct ShareForm {
    target: ShareTarget,
    scope: State<Scope>,
    permanent: State<bool>,
    entry_role: State<EntryRolePref>,
    /// The approval switch, once touched; else the setting decides.
    approve: State<Option<bool>>,
}

impl std::fmt::Debug for ShareForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareForm")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl ShareForm {
    fn mode(&self) -> ShareMode {
        if *self.permanent.peek() || *self.scope.peek() == Scope::Folder {
            ShareMode::Permanent
        } else {
            ShareMode::Session
        }
    }

    fn approves(&self) -> bool {
        self.approve.peek().unwrap_or_else(|| {
            SettingsStore::try_get()
                .map(|s| s.peek().sessions.approve)
                .unwrap_or_default()
                .for_mode(self.mode())
        })
    }
}

#[sismo::function("collab::scope_card")]
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
    /// What to share: the document, with its notebooks, the mesa, or
    /// the folder.
    fn what(&self, data: &LibraryData) -> Section {
        use freya::icons::lucide;
        let target = self.target;
        let scope = self.scope;
        let current = *scope.read();
        let notebooks = target.item.map_or(0, |i| data.live_notebooks_of(i).len());
        let mesa_docs = target
            .mesa
            .map_or(0, |_| target.docs(Scope::Mesa, data).len());
        let folder_docs = target
            .folder
            .map_or(0, |_| target.docs(Scope::Folder, data).len());
        let doc_name = target
            .item
            .and_then(|i| data.item(i))
            .map_or_else(String::new, |i| i.name.clone());
        let folder_name = target.title(Scope::Folder, data);
        let choose = move |chosen: Scope| {
            move || {
                let mut scope = scope;
                scope.set(chosen);
            }
        };
        let mut row = rect()
            .width(Size::fill())
            .height(Size::px(112.0))
            .horizontal()
            .content(Content::Flex)
            .spacing(10.0);
        if target.folder.is_some() {
            row = row.child(card(
                current == Scope::Folder,
                lucide::folder(),
                tr!("collab-share-scope-folder"),
                tr!("collab-share-folder-count", "name" => folder_name.as_str(), "count" => folder_docs),
                true,
                choose(Scope::Folder),
            ));
        } else {
            row = row
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
                ));
        }
        Section::new(tr!("collab-share-what")).child(row)
    }

    /// For this session or for good, who gets in and whether they wait.
    fn how(&self) -> Vec<Element> {
        let (mut permanent, mut entry_role, mut approve) =
            (self.permanent, self.entry_role, self.approve);
        let folder = *self.scope.read() == Scope::Folder;
        let lasting = folder || *permanent.read();
        let approves = self.approves();
        let role = *entry_role.read();
        let mut out = Vec::new();
        if folder {
            out.push(Notice::new(NoticeTone::Info, tr!("collab-share-folder-hint")).into_element());
        } else {
            out.push(
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
            );
        }
        out.push(
            Group::default()
                .child(
                    ListRow::new(tr!("collab-entry-role"))
                        .subtitle(tr!("collab-entry-role-hint"))
                        .trailing(Trailing::Element(
                            Segmented::new(
                                EntryRolePref::ALL.map(EntryRolePref::label),
                                EntryRolePref::ALL
                                    .iter()
                                    .position(|r| *r == role)
                                    .unwrap_or(1),
                            )
                            .on_select(move |i: usize| entry_role.set(EntryRolePref::ALL[i]))
                            .into_element(),
                        )),
                )
                .child(
                    ListRow::new(tr!("collab-approve-entry"))
                        .subtitle(tr!("collab-approve-entry-hint"))
                        .trailing(Trailing::Switch {
                            on: approves,
                            on_toggle: EventHandler::new(move |on| approve.set(Some(on))),
                        }),
                )
                .into_element(),
        );
        out
    }
}

impl Component for ShareForm {
    fn render(&self) -> impl IntoElement {
        sismo::scope!("collab::render_shareform");
        let snapshot = crate::sync::use_sync().read().clone();
        let Some(data) = LibraryView::get().data() else {
            return rect().into_element();
        };
        let others = {
            let docs = self.target.docs(*self.scope.read(), &data);
            snapshot
                .shares
                .iter()
                .filter(|s| docs.iter().any(|d| s.docs.contains(d)))
                .map(|s| s.title.clone())
                .collect::<Vec<_>>()
        };
        rect()
            .width(Size::fill())
            .spacing(18.0)
            .child(self.what(&data))
            .children(self.how())
            .maybe_child((!others.is_empty()).then(|| {
                Notice::new(
                    NoticeTone::Info,
                    tr!("collab-share-also-in", "titles" => others.join(", ")),
                )
            }))
            .maybe_child(
                (snapshot.trusted.is_empty() && snapshot.relay.is_none())
                    .then(|| Notice::new(NoticeTone::Info, tr!("collab-share-relay-hint"))),
            )
            .into_element()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct ShareFooter {
    form: ShareForm,
}

impl Component for ShareFooter {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        sismo::scope!("collab::render_sharefooter");
        let Some(data) = LibraryView::get().data() else {
            return rect().into_element();
        };
        let form = self.form;
        let target = form.target;
        let scope = *form.scope.read();
        let docs = target.docs(scope, &data);
        let title = target.title(scope, &data);
        let mode = form.mode();
        let entry_role: Role = form.entry_role.read().role();
        let approve_entry = form.approves();
        let folder = (scope == Scope::Folder).then_some(target.folder).flatten();
        let enabled = !docs.is_empty() || folder.is_some();
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
                        folder,
                        desk: scope == Scope::Mesa,
                        entry_role,
                        approve_entry,
                        editors_add_docs: folder.is_some(),
                    });
                    let docs = docs.clone();
                    let known: Vec<_> = crate::sync::snapshot()
                        .shares
                        .iter()
                        .map(|s| s.id)
                        .collect();
                    spawn(async move {
                        let changes = crate::sync::changes();
                        loop {
                            let fresh = crate::sync::snapshot().shares.into_iter().find(|s| {
                                s.is_owner
                                    && !known.contains(&s.id)
                                    && (s.folder == folder
                                        || docs.iter().all(|d| s.docs.contains(d)))
                            });
                            if let Some(view) = fresh {
                                ModalController::get().close();
                                super::session(view.id);
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

//! Copies in and out: Export (a backup to import again, or files
//! other apps read) and Import (review collisions before anything is
//! written).

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use freya::prelude::*;
use notein_i18n::tr;
use notein_plugin::UserMessage;
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, Group, ListRow, ModalController, Notice, NoticeTone, ProgressBar, Section, Segmented,
    Sheet, SheetFooter, Tag, Text, TextRole, Toast, ToastTone, Toasts, Trailing, tokens,
};

use crate::bundle::{ExportOptions, ExportSelection};
use crate::library::{
    FolderId, ImportPlan, ImportReceipt, ItemId, LibraryData, ROOT_FOLDER, Resolution, Review,
};
use crate::state::LibraryView;

/// What an export covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportScope {
    Library,
    Items(Vec<ItemId>),
    Folder(FolderId),
}

/// What the copy is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Purpose {
    #[default]
    Backup,
    Readable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Choices {
    purpose: Purpose,
    attachments: bool,
    mesas_and_marks: bool,
}

impl Default for Choices {
    fn default() -> Self {
        Self {
            purpose: Purpose::Backup,
            attachments: true,
            mesas_and_marks: true,
        }
    }
}

fn selection(scope: &ExportScope, data: &crate::library::LibraryData) -> ExportSelection {
    match scope {
        ExportScope::Library => ExportSelection {
            items: data.live_items().map(|i| i.id).collect(),
            folders: Vec::new(),
            workspaces: data.workspaces().iter().map(|w| w.id).collect(),
        },
        ExportScope::Items(items) => ExportSelection {
            items: items.clone(),
            ..ExportSelection::default()
        },
        ExportScope::Folder(folder) => ExportSelection {
            folders: vec![*folder],
            ..ExportSelection::default()
        },
    }
}

fn item_ids(scope: &ExportScope, data: &crate::library::LibraryData) -> Vec<ItemId> {
    match scope {
        ExportScope::Library => data.live_items().map(|i| i.id).collect(),
        ExportScope::Items(items) => items.clone(),
        ExportScope::Folder(folder) => {
            let mut out = Vec::new();
            let mut queue = vec![*folder];
            while let Some(f) = queue.pop() {
                out.extend(data.items_of(f).into_iter().map(|i| i.id));
                queue.extend(data.folders_of(f).into_iter().map(|c| c.id));
            }
            out
        }
    }
}

fn sizes(items: &[ItemId]) -> (u64, u64) {
    items.iter().fold((0, 0), |(all, pdfs), id| {
        let total = crate::library::bodies::disk_usage(*id);
        let pdf = std::fs::metadata(crate::library::bodies::pdf_path(*id)).map_or(0, |m| m.len());
        (all + total, pdfs + pdf)
    })
}

fn file_stamp() -> String {
    let now = chrono::Local::now();
    now.format("%Y-%m-%d").to_string()
}

/// Open the export sheet.
#[sismo::function("flows::export")]
pub fn export(scope: ExportScope) {
    let choices = State::create_in_scope(Choices::default(), ScopeId::ROOT);
    let footer_scope = scope.clone();
    Sheet::titled(tr!("flows-transfer-export-title"))
        .child(ExportForm { scope, choices })
        .footer(ExportFooter {
            scope: footer_scope,
            choices,
        })
        .push();
}

#[derive(Clone, PartialEq)]
struct ExportForm {
    scope: ExportScope,
    choices: State<Choices>,
}

/// A purpose card: icon, title and what it gives.
#[sismo::function("flows::purpose_card")]
fn purpose_card(
    mut choices: State<Choices>,
    purpose: Purpose,
    (title, sub): (String, String),
    icon: Bytes,
) -> Element {
    let p = palette();
    let on = choices.read().purpose == purpose;
    rect()
        .key(format!("{purpose:?}"))
        .width(Size::flex(1.0))
        .height(Size::px(100.0))
        .padding((12.0, 14.0))
        .corner_radius(tokens::radius::LG)
        .background(if on { p.accent_soft } else { p.s3 })
        .border(
            Border::new()
                .fill(if on { p.accent } else { Color::TRANSPARENT })
                .width(1.5)
                .alignment(BorderAlignment::Inner),
        )
        .spacing(4.0)
        .a11y_role(AccessibilityRole::RadioButton)
        .on_tap(move |_| choices.write().purpose = purpose)
        .child(
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(8.0)
                .child(notein_ui_kit::Icon::new(icon).size(16.0).color(if on {
                    p.accent_text
                } else {
                    p.t1
                }))
                .child(
                    label()
                        .text(title)
                        .font_size(tokens::text::BODY)
                        .font_weight(FontWeight::BOLD)
                        .color(if on { p.accent_text } else { p.t1 }),
                ),
        )
        .child(
            label()
                .text(sub)
                .font_size(tokens::text::LABEL)
                .color(p.t2)
                .line_height(1.4),
        )
        .into_element()
}

impl ExportForm {
    /// How the scope reads: the library, a document, a count, a folder.
    fn what(&self, data: &LibraryData) -> String {
        match &self.scope {
            ExportScope::Library => tr!("flows-transfer-whole-library"),
            ExportScope::Items(items) if items.len() == 1 => data.item(items[0]).map_or_else(
                || tr!("flows-transfer-documents", "count" => 1),
                |i| i.name.clone(),
            ),
            ExportScope::Items(items) => tr!("flows-transfer-documents", "count" => items.len()),
            ExportScope::Folder(folder) => data
                .folder(*folder)
                .map_or_else(|| tr!("flows-create-folder"), |f| f.name.clone()),
        }
    }

    /// What goes in, with the backup's options.
    #[sismo::function("flows::contents")]
    fn contents(&self, data: &LibraryData) -> Group {
        let mut choices = self.choices;
        let current = *choices.read();
        let items = item_ids(&self.scope, data);
        let (total, pdfs) = sizes(&items);
        let backup = current.purpose == Purpose::Backup;
        Group::default()
            .child(ListRow::new(self.what(data)).subtitle(tr!(
                "flows-transfer-contents",
                "count" => items.len(),
                "size" => crate::format::size(total)
            )))
            .maybe_child(backup.then(|| {
                ListRow::new(tr!("flows-transfer-include-pdfs"))
                    .subtitle(tr!(
                        "flows-transfer-include-pdfs-hint",
                        "size" => crate::format::size(pdfs)
                    ))
                    .trailing(Trailing::Switch {
                        on: current.attachments,
                        on_toggle: EventHandler::new(move |on| choices.write().attachments = on),
                    })
            }))
            .maybe_child(backup.then(|| {
                ListRow::new(tr!("flows-transfer-include-desks"))
                    .subtitle(tr!(
                        "flows-transfer-include-desks-hint",
                        "count" => data.workspaces().len()
                    ))
                    .trailing(Trailing::Switch {
                        on: current.mesas_and_marks,
                        on_toggle: EventHandler::new(move |on| {
                            choices.write().mesas_and_marks = on;
                        }),
                    })
            }))
    }
}

impl Component for ExportForm {
    #[sismo::function("flows::export_form")]
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let Some(data) = LibraryView::get().data() else {
            return rect().into_element();
        };
        rect()
            .width(Size::fill())
            .spacing(16.0)
            .child(
                Section::new(tr!("flows-transfer-purpose")).child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .content(Content::Flex)
                        .spacing(10.0)
                        .child(purpose_card(
                            self.choices,
                            Purpose::Backup,
                            (
                                tr!("flows-transfer-backup"),
                                tr!("flows-transfer-backup-hint"),
                            ),
                            lucide::archive(),
                        ))
                        .child(purpose_card(
                            self.choices,
                            Purpose::Readable,
                            (
                                tr!("flows-transfer-readable"),
                                tr!("flows-transfer-readable-hint"),
                            ),
                            lucide::folder(),
                        )),
                ),
            )
            .child(Section::new(tr!("flows-transfer-what")).child(self.contents(&data)))
            .into_element()
    }
}

#[derive(Clone, PartialEq)]
struct ExportFooter {
    scope: ExportScope,
    choices: State<Choices>,
}

impl Component for ExportFooter {
    #[sismo::function("flows::export_footer")]
    fn render(&self) -> impl IntoElement {
        let choices = *self.choices.read();
        let data = LibraryView::get().data();
        let items = data
            .as_deref()
            .map(|d| item_ids(&self.scope, d))
            .unwrap_or_default();
        let (total, pdfs) = sizes(&items);
        let size = if choices.purpose == Purpose::Backup && !choices.attachments {
            total.saturating_sub(pdfs)
        } else {
            total
        };
        let (name, label_text) = match choices.purpose {
            Purpose::Backup => (
                format!("notein-{}.notein", file_stamp()),
                tr!("flows-transfer-export-size", "size" => crate::format::size(size)),
            ),
            Purpose::Readable => (
                format!("notein-{}.zip", file_stamp()),
                tr!("flows-transfer-export"),
            ),
        };
        let scope = self.scope.clone();
        SheetFooter::new([
            Button::new(tr!("common-cancel"))
                .ghost()
                .on_press(|_| ModalController::get().close())
                .into_element(),
            Button::new(label_text)
                .primary()
                .icon(freya::icons::lucide::download())
                .on_press(move |_| {
                    ModalController::get().close();
                    run_export(scope.clone(), choices);
                })
                .into_element(),
        ])
        .hint(tr!("flows-transfer-saved-as", "name" => name))
    }
}

/// Export progress, readable by the progress sheet.
#[derive(Clone, Copy, PartialEq)]
struct Exporting {
    done: State<usize>,
    total: usize,
}

impl Component for Exporting {
    #[sismo::function("flows::exporting")]
    fn render(&self) -> impl IntoElement {
        let done = *self.done.read();
        #[allow(clippy::cast_precision_loss)]
        let fraction = if self.total == 0 {
            1.0
        } else {
            done as f32 / self.total as f32
        };
        rect()
            .width(Size::fill())
            .spacing(10.0)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .child(
                        Text::new(tr!(
                            "flows-transfer-progress",
                            "done" => done,
                            "total" => self.total
                        ))
                        .fill(),
                    )
                    .child(
                        Text::new(format!("{} %", (fraction * 100.0).round())).role(TextRole::Meta),
                    ),
            )
            .child(ProgressBar::new(fraction))
            .child(Text::new(tr!("flows-transfer-progress-hint")).role(TextRole::Hint))
    }
}

/// Why an export did not produce a file.
#[derive(Debug)]
enum ExportError {
    Bundle(crate::bundle::BundleError),
    Readable(crate::readable::ReadableError),
    Stopped,
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bundle(err) => write!(f, "{err}"),
            Self::Readable(err) => write!(f, "{err}"),
            Self::Stopped => f.write_str("the export worker stopped"),
        }
    }
}

impl std::error::Error for ExportError {}

impl UserMessage for ExportError {
    fn user_message(&self) -> String {
        match self {
            Self::Bundle(err) => err.user_message(),
            Self::Readable(err) => err.user_message(),
            Self::Stopped => tr!("errors-export-stopped"),
        }
    }
}

/// What the export worker reports back.
enum Progress {
    Item(usize),
    Finished(Result<Vec<u8>, ExportError>),
}

impl Purpose {
    const fn file(self) -> (&'static str, &'static str) {
        match self {
            Self::Backup => ("application/octet-stream", "notein"),
            Self::Readable => ("application/zip", "zip"),
        }
    }

    #[sismo::function("flows::write")]
    fn write(
        self,
        data: &crate::library::LibraryData,
        scope: &ExportScope,
        items: &[ItemId],
        choices: Choices,
        progress: impl FnMut(usize),
    ) -> Result<Vec<u8>, ExportError> {
        match self {
            Self::Readable => {
                crate::readable::build(data, items, progress).map_err(ExportError::Readable)
            }
            Self::Backup => {
                let options = ExportOptions {
                    include_attachments: choices.attachments,
                    include_external_links: false,
                    include_workspaces: choices.mesas_and_marks,
                    include_marks: choices.mesas_and_marks,
                };
                let device = crate::sync::snapshot().name;
                let mut out = Vec::new();
                crate::bundle::write_bundle(
                    &mut out,
                    data,
                    &selection(scope, data),
                    options,
                    &device,
                    progress,
                )
                .map(|()| out)
                .map_err(ExportError::Bundle)
            }
        }
    }
}

#[sismo::function("flows::run_export")]
fn run_export(scope: ExportScope, choices: Choices) {
    spawn_in_window(traced!("task::flows::export", async move {
        let Some(data) = LibraryView::get().data() else {
            return;
        };
        let items = item_ids(&scope, &data);
        let mut done = State::create_in_scope(0usize, ScopeId::ROOT);
        let mut showing = State::create_in_scope(true, ScopeId::ROOT);
        Sheet::titled(tr!("flows-transfer-exporting"))
            .child(Exporting {
                done,
                total: items.len(),
            })
            .footer(SheetFooter::new([Button::new(tr!(
                "flows-create-background"
            ))
            .on_press(|_| ModalController::get().close())
            .into_element()]))
            .open();
        ModalController::get().set_top_on_close(move || showing.set(false));
        let (tx, rx) = flume::unbounded();
        let worker_scope = scope.clone();
        std::thread::spawn(move || {
            sismo::scope!("flows::export_write");
            let progress = tx.clone();
            let result = choices
                .purpose
                .write(&data, &worker_scope, &items, choices, |n| {
                    let _ = progress.send(Progress::Item(n));
                });
            let _ = tx.send(Progress::Finished(result));
        });
        let result = loop {
            match rx.recv_async().await {
                Ok(Progress::Item(n)) => done.set(n),
                Ok(Progress::Finished(result)) => break result,
                Err(_) => break Err(ExportError::Stopped),
            }
        };
        if *showing.peek() {
            ModalController::get().close();
        }
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(err) => {
                log::error!("export: {err}");
                let text = tr!("errors-headline-detail", "headline" => tr!("flows-transfer-export-failed"), "detail" => err.user_message());
                Toasts::get().show(Toast::new(text).tone(ToastTone::Error));
                return;
            }
        };
        let (mime, ext) = choices.purpose.file();
        save_copy(
            format!("notein-{}.{ext}", file_stamp()),
            mime,
            ext,
            bytes,
            scope,
            choices,
        );
    }));
}

#[sismo::function("flows::save_copy")]
fn save_copy(
    name: String,
    mime: &'static str,
    ext: &'static str,
    bytes: Vec<u8>,
    scope: ExportScope,
    choices: Choices,
) {
    spawn_in_window(traced!("task::flows::save_copy", async move {
        match crate::files::save(&tr!("flows-transfer-save-copy"), &name, mime, ext, &bytes).await {
            Ok(Some(saved)) => done_sheet(saved, bytes, mime),
            Ok(None) => {}
            Err(crate::files::FileError::Io(err))
                if err.kind() == std::io::ErrorKind::StorageFull =>
            {
                no_space(bytes.len(), scope, choices);
            }
            Err(err) => {
                log::error!("save copy: {err}");
                let text = tr!("errors-headline-detail", "headline" => tr!("flows-transfer-save-failed"), "detail" => err.user_message());
                Toasts::get().show(Toast::new(text).tone(ToastTone::Error));
            }
        }
    }));
}

#[sismo::function("flows::done_sheet")]
fn done_sheet(saved: crate::files::Saved, bytes: Vec<u8>, mime: &'static str) {
    let size = crate::format::size(u64::try_from(bytes.len()).unwrap_or(0));
    let crate::files::Saved { name, path } = saved;
    let subtitle = tr!("flows-transfer-saved-subtitle", "name" => name.clone(), "size" => size);
    let mut buttons = vec![
        Button::new(tr!("common-done"))
            .ghost()
            .on_press(|_| ModalController::get().close())
            .into_element(),
    ];
    if let Some(folder) = path.as_deref().and_then(std::path::Path::parent) {
        let folder = folder.to_path_buf();
        buttons.push(
            Button::new(tr!("flows-transfer-show-in-folder"))
                .icon(freya::icons::lucide::folder_open())
                .on_press(move |_| {
                    ModalController::get().close();
                    crate::external::open_url(&format!("file://{}", folder.display()));
                })
                .into_element(),
        );
    }
    buttons.push(
        Button::new(tr!("common-share"))
            .primary()
            .icon(freya::icons::lucide::share_2())
            .on_press(move |_| {
                ModalController::get().close();
                super::share_file(name.clone(), mime.to_owned(), bytes.clone());
            })
            .into_element(),
    );
    Sheet::default()
        .without_close()
        .child(
            ListRow::new(tr!("flows-transfer-saved"))
                .subtitle(subtitle)
                .icon(freya::icons::lucide::circle_check()),
        )
        .footer(SheetFooter::new(buttons))
        .open();
}

#[sismo::function("flows::no_space")]
fn no_space(bytes: usize, scope: ExportScope, choices: Choices) {
    let missing = crate::format::size(u64::try_from(bytes).unwrap_or(0));
    let retry_scope = scope.clone();
    Sheet::default()
        .without_close()
        .child(
            Notice::new(NoticeTone::Warning, tr!("flows-transfer-no-space"))
                .detail(tr!("flows-transfer-no-space-hint", "missing" => missing)),
        )
        .footer(SheetFooter::new([
            Button::new(tr!("flows-transfer-without-pdfs"))
                .ghost()
                .on_press(move |_| {
                    ModalController::get().close();
                    run_export(
                        scope.clone(),
                        Choices {
                            attachments: false,
                            ..choices
                        },
                    );
                })
                .into_element(),
            Button::new(tr!("flows-transfer-pick-place"))
                .primary()
                .on_press(move |_| {
                    ModalController::get().close();
                    run_export(retry_scope.clone(), choices);
                })
                .into_element(),
        ]))
        .open();
}

/// How long an import can be undone from its folder's menu.
pub const UNDO_IMPORT: Duration = Duration::from_secs(30);

static LAST_IMPORT: Mutex<Option<(ImportReceipt, Instant)>> = Mutex::new(None);

/// The last import into `folder`, while it can still be undone.
#[must_use]
#[sismo::function("flows::undoable_import")]
pub fn undoable_import(folder: FolderId) -> Option<ImportReceipt> {
    LAST_IMPORT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .filter(|(r, at)| r.destination == folder && at.elapsed() < UNDO_IMPORT)
        .map(|(r, _)| r.clone())
}

/// Remove everything the last import wrote.
#[sismo::function("flows::undo_import")]
pub fn undo_import(receipt: ImportReceipt) {
    *LAST_IMPORT.lock().unwrap_or_else(PoisonError::into_inner) = None;
    crate::state::edit(crate::state::LibraryEdit::UndoImport, async move |lib| {
        lib.undo_import(&receipt).await
    });
}

/// Pick a `.notein` file and review it.
#[sismo::function("flows::import")]
pub fn import() {
    spawn_in_window(traced!("task::flows::import", async move {
        let picked = match crate::files::pick(
            &tr!("flows-transfer-import-picker"),
            &["notein"],
            &["application/octet-stream"],
        )
        .await
        {
            Ok(Some(picked)) => picked,
            Ok(None) => return,
            Err(err) => {
                log::error!("pick bundle: {err}");
                return;
            }
        };
        let reading = Sheet::default().without_close().child(
            ListRow::new(tr!("flows-transfer-reviewing", "name" => picked.name.clone())).leading(
                notein_ui_kit::Leading::Element(notein_ui_kit::Spinner::default().into_element()),
            ),
        );
        // Over whatever asked for it (Settings stays underneath).
        reading.push();
        let name = picked.name.clone();
        let (tx, rx) = flume::bounded(1);
        std::thread::spawn(move || {
            sismo::scope!("flows::import_parse");
            let _ = tx.send(crate::bundle::parse_bundle(std::io::Cursor::new(
                picked.bytes,
            )));
        });
        let parsed = rx.recv_async().await;
        ModalController::get().close();
        match parsed {
            Ok(Ok(bundle)) => review(&name, bundle),
            Ok(Err(crate::bundle::BundleError::UnknownVersion(v)))
                if v > crate::bundle::BUNDLE_VERSION =>
            {
                newer_version();
            }
            Ok(Err(err)) => {
                log::error!("bundle {name}: {err}");
                damaged(&name);
            }
            Err(_) => damaged(&name),
        }
    }));
}

#[sismo::function("flows::newer_version")]
fn newer_version() {
    Sheet::default()
        .without_close()
        .child(
            Notice::new(NoticeTone::Danger, tr!("flows-transfer-newer"))
                .detail(tr!("flows-transfer-newer-hint")),
        )
        .footer(SheetFooter::new([
            Button::new(tr!("flows-transfer-pick-another"))
                .ghost()
                .on_press(|_| {
                    ModalController::get().close();
                    import();
                })
                .into_element(),
            Button::new(tr!("flows-transfer-check-update"))
                .primary()
                .on_press(|_| {
                    ModalController::get().close();
                    crate::external::open_url("https://github.com/SergioRibera/notinplus/releases");
                })
                .into_element(),
        ]))
        .open();
}

#[sismo::function("flows::damaged")]
fn damaged(name: &str) {
    Sheet::default()
        .without_close()
        .child(
            Notice::new(
                NoticeTone::Danger,
                tr!("flows-transfer-damaged", "name" => name.to_owned()),
            )
            .detail(tr!("flows-transfer-damaged-hint")),
        )
        .footer(SheetFooter::new([Button::new(tr!(
            "flows-transfer-pick-another"
        ))
        .primary()
        .on_press(|_| {
            ModalController::get().close();
            import();
        })
        .into_element()]))
        .open();
}

#[derive(Clone)]
struct Pending {
    bundle: std::rc::Rc<crate::bundle::ParsedBundle>,
    review: Review,
}

impl PartialEq for Pending {
    fn eq(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.bundle, &other.bundle)
    }
}

#[sismo::function("flows::review")]
fn review(name: &str, bundle: crate::bundle::ParsedBundle) {
    let Some(data) = LibraryView::get().data() else {
        return;
    };
    let kinds: Vec<String> = crate::plugins::catalog()
        .kinds()
        .iter()
        .map(|k| k.info().id.into_owned())
        .collect();
    let review = data.review_import(&bundle, &kinds);
    let pending = Pending {
        bundle: std::rc::Rc::new(bundle),
        review,
    };
    let plan = State::create_in_scope(ImportPlan::default(), ScopeId::ROOT);
    let footer = pending.clone();
    Sheet::titled(tr!("flows-transfer-import-title", "name" => name.to_owned()))
        .width(600.0)
        .child(ImportReview { pending, plan })
        .footer(ImportFooter {
            pending: footer,
            plan,
        })
        .open();
}

#[derive(Clone, PartialEq)]
struct ImportReview {
    pending: Pending,
    plan: State<ImportPlan>,
}

const RESOLUTIONS: [Resolution; 3] = [Resolution::KeepBoth, Resolution::Replace, Resolution::Skip];

/// What the resolution segments read.
fn resolution_labels() -> [String; 3] {
    RESOLUTIONS.map(|resolution| match resolution {
        Resolution::KeepBoth => tr!("flows-transfer-keep-both"),
        Resolution::Replace => tr!("flows-transfer-replace"),
        Resolution::Skip => tr!("flows-transfer-skip"),
    })
}

impl Component for ImportReview {
    #[sismo::function("flows::import_review")]
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let mut plan = self.plan;
        let current = plan.read().clone();
        let review = &self.pending.review;
        let now = crate::format::now();
        let destination = imported_folder_name(&review.source);
        let new_count = review.documents - review.conflicts.len();
        rect()
            .width(Size::fill())
            .spacing(14.0)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::wrap_spacing(6.0))
                    .spacing(6.0)
                    .child(Tag::new(tr!("flows-transfer-documents", "count" => review.documents)).icon(lucide::file_text()))
                    .child(Tag::new(tr!("flows-transfer-desks", "count" => review.mesas)).icon(lucide::layout_grid()))
                    .child(Tag::new(tr!("flows-transfer-bookmarks", "count" => review.marks)).icon(lucide::bookmark()))
                    .child(Tag::new(crate::format::size(review.bytes)).icon(lucide::hard_drive())),
            )
            .maybe_child((!review.missing_kinds.is_empty()).then(|| {
                Notice::new(
                    NoticeTone::Warning,
                    tr!(
                        "flows-transfer-missing-plugins",
                        "count" => self
                            .pending
                            .bundle
                            .manifest
                            .items
                            .iter()
                            .filter(|i| review.missing_kinds.contains(&i.kind.plugin_id))
                            .count()
                    ),
                )
            }))
            .maybe_child((!review.conflicts.is_empty()).then(|| {
                Section::new(tr!("flows-transfer-conflicts", "count" => review.conflicts.len()))
                    .icon(lucide::triangle_alert())
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(10.0)
                            .child(Text::new(tr!("flows-transfer-for-all")).role(TextRole::Meta))
                            .child(
                                Segmented::new(resolution_labels(), RESOLUTIONS.iter().position(|r| *r == current.rule).unwrap_or(0))
                                    .height(32.0)
                                    .on_select(move |i: usize| {
                                        let mut plan = plan.write();
                                        plan.rule = RESOLUTIONS[i];
                                        plan.overrides.clear();
                                    }),
                            ),
                    )
                    .child(Group::default().children(review.conflicts.iter().map(|conflict| {
                        let id = conflict.incoming.id;
                        let subtitle = if conflict.identical {
                            tr!("flows-transfer-identical")
                        } else {
                            let newer = conflict.incoming.updated_at > conflict.existing.updated_at;
                            tr!(
                                "flows-transfer-conflict",
                                "mine" => crate::format::day(conflict.existing.updated_at, now),
                                "theirs" => crate::format::day(conflict.incoming.updated_at, now),
                                "newer" => if newer { "yes" } else { "no" }
                            )
                        };
                        let chosen = current.resolution(id);
                        let row = ListRow::new(conflict.incoming.name.clone()).subtitle(subtitle);
                        let row = if conflict.identical {
                            row
                        } else {
                            row.trailing(Trailing::Element(
                                Segmented::new(resolution_labels(), RESOLUTIONS.iter().position(|r| *r == chosen).unwrap_or(0))
                                    .height(30.0)
                                    .on_select(move |i: usize| {
                                        plan.write().overrides.insert(id, RESOLUTIONS[i]);
                                    })
                                    .into_element(),
                            ))
                        };
                        row.into_element()
                    })))
            }))
            .child(Group::default().child(
                ListRow::new(tr!("flows-transfer-new", "count" => new_count)).subtitle(tr!("flows-transfer-new-hint")).icon(lucide::plus()),
            ))
            .child(Group::default().child(
                ListRow::new(tr!("flows-transfer-save-in")).subtitle(tr!("flows-transfer-destination", "folder" => destination)).icon(lucide::folder()),
            ))
    }
}

#[derive(Clone, PartialEq)]
struct ImportFooter {
    pending: Pending,
    plan: State<ImportPlan>,
}

impl Component for ImportFooter {
    #[sismo::function("flows::import_footer")]
    fn render(&self) -> impl IntoElement {
        let plan = self.plan.read().clone();
        let review = self.pending.review.clone();
        let count = review.importing(&plan);
        let skipped = review.identical();
        let pending = self.pending.clone();
        SheetFooter::new([
            Button::new(tr!("common-cancel"))
                .ghost()
                .on_press(|_| ModalController::get().close())
                .into_element(),
            Button::new(tr!("flows-transfer-import-count", "count" => count))
                .primary()
                .icon(freya::icons::lucide::upload())
                .enabled(count > 0)
                .on_press(move |_| {
                    ModalController::get().close();
                    commit(pending.clone(), plan.clone());
                })
                .into_element(),
        ])
        .hint(if skipped > 0 {
            tr!("flows-transfer-skipped", "count" => skipped)
        } else {
            String::new()
        })
    }
}

/// Name of the folder an import lands in, stored in the active language.
fn imported_folder_name(source: &str) -> String {
    if source.is_empty() {
        tr!("flows-transfer-imported-folder")
    } else {
        tr!("flows-transfer-imported-folder-from", "source" => source.to_owned())
    }
}

#[sismo::function("flows::commit")]
fn commit(pending: Pending, mut plan: ImportPlan) {
    spawn_in_window(traced!("task::flows::import_commit", async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let mut lib = handle.lock().await;
        let name = imported_folder_name(&pending.review.source);
        let destination = match lib
            .create_folder(ROOT_FOLDER, &name, None, Vec::new())
            .await
        {
            Ok(folder) => folder,
            Err(err) => {
                log::error!("import folder: {err}");
                let text = tr!("errors-headline-detail", "headline" => tr!("flows-transfer-import-failed"), "detail" => err.user_message());
                Toasts::get().show(Toast::new(text).tone(ToastTone::Error));
                return;
            }
        };
        plan.destination = destination;
        match lib
            .import_planned(&pending.bundle, &pending.review, &plan)
            .await
        {
            Ok(receipt) => {
                drop(lib);
                let count = receipt.items.len();
                *LAST_IMPORT.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some((receipt, Instant::now()));
                crate::plugins::content::index_library();
                Toasts::get().show(
                    Toast::new(tr!("flows-transfer-imported", "count" => count)).action(
                        tr!("flows-transfer-view"),
                        move |()| {
                            crate::screens::home::open_folder(destination);
                        },
                    ),
                );
            }
            Err(err) => {
                log::error!("import: {err}");
                let text = tr!("errors-headline-detail", "headline" => tr!("flows-transfer-import-failed"), "detail" => err.user_message());
                Toasts::get().show(Toast::new(text).tone(ToastTone::Error));
            }
        }
    }));
}

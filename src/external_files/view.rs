//! The view of an opened file: its bar, the notices about where edits
//! go, and the body that shows it.

use std::rc::Rc;

use freya::icons::lucide;
use freya::prelude::*;
use notein_i18n::tr;
use notein_ui_kit::theme::{Palette, palette};
use notein_ui_kit::{
    Action, ActionEntry, ActionList, Button, IconButton, Notice, NoticeTone, Segmented, Tag, Text,
    TextRole, Toast, ToastHost, ToastTone, Toasts, tokens, width_class,
};

use super::{FileKind, Location, OWN_WINDOWS, Session};
use crate::files::Picked;
use crate::flows::create::{Context, Landing};
use crate::library::ItemId;

/// Whether the text of the file is on show or being changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Read,
    Edit,
}

/// How the last edits fared on their way to the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SaveState {
    /// Nothing changed yet.
    Idle,
    Saving,
    Saved,
    Failed,
}

/// The window of one opened file (desktop).
pub(super) struct ExternalWindow {
    pub session: Session,
}

impl App for ExternalWindow {
    fn render(&self) -> impl IntoElement {
        WindowRoot {
            session: self.session.clone(),
        }
    }
}

/// What the view needs from a window of its own: settings and theme, the
/// library for the import sheet, modals and toasts.
#[derive(Clone, Debug, PartialEq)]
struct WindowRoot {
    session: Session,
}

impl Component for WindowRoot {
    #[sismo::function("external_files::window_root")]
    fn render(&self) -> impl IntoElement {
        let area = use_state(Area::default);
        let settings = crate::settings::use_settings_root();
        crate::state::use_library_root();
        crate::screens::home::use_browse_root();
        let lang = use_memo(move || settings.read().appearance.interface_lang());
        let gutters = use_memo(notein_ui_kit::Gutters::default);
        use_provide_context(move || notein_ui_kit::ModalChrome {
            gutters: Some(gutters),
            ..Default::default()
        });
        provide_context(notein_ui_kit::ViewportArea(area));
        rect()
            .expanded()
            .background(palette().bg)
            .on_sized({
                let mut area = area;
                move |e: Event<SizedEventData>| area.set(e.area)
            })
            .child(rect().expanded().key(*lang.read()).child(ExternalView {
                session: self.session.clone(),
            }))
            .child(crate::components::ModalPortal::new(area))
            .child(ToastHost::new(0.0))
    }
}

/// An opened file, alone: no library, no mesas.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalView {
    pub session: Session,
}

/// The states the parts of the view share.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct ViewState {
    pub mode: State<Mode>,
    pub save: State<SaveState>,
    /// The text on show (Markdown and text files).
    pub text: State<String>,
    /// A `.txt` got formatting it can only keep as symbols.
    pub formatted: State<bool>,
    /// The `.txt` notice was answered.
    pub answered: State<bool>,
    /// The document the file became in the library.
    pub in_library: State<Option<ItemId>>,
}

impl ViewState {
    fn use_state(session: &Session) -> Self {
        let file = &session.file;
        Self {
            mode: use_state(|| Mode::Read),
            save: use_state(|| SaveState::Idle),
            text: use_state(|| {
                if file.kind.editable() {
                    String::from_utf8_lossy(file.bytes()).into_owned()
                } else {
                    String::new()
                }
            }),
            formatted: use_state(|| false),
            answered: use_state(|| false),
            in_library: use_state(|| None),
        }
    }

    /// The file as it is now: the text on show, or the bytes read.
    fn current(&self, session: &Session) -> Vec<u8> {
        if session.file.kind.editable() {
            self.text.peek().clone().into_bytes()
        } else {
            session.file.bytes().to_vec()
        }
    }

    /// Open the import sheet with the file as it is now.
    #[sismo::function("external_files::save_to_library")]
    fn save_to_library(&self, session: &Session) {
        let picked = Picked {
            name: session.file.name.clone(),
            bytes: self.current(session),
        };
        let in_library = self.in_library;
        let path = session.file.path.clone();
        let landing = Landing::Report(Rc::new(move |item| {
            let mut in_library = in_library;
            in_library.set(Some(item));
            super::recents::forget(&path);
            Toasts::get().show(Toast::new(tr!("external-files-saved-to-library")));
        }));
        let context = Context {
            folder: crate::screens::home::Browse::get().folder(),
            ..Context::default()
        };
        if !crate::flows::create::import_file_landing(picked, context, landing) {
            Toasts::get().show(Toast::new(tr!("opened-files-unsupported")).tone(ToastTone::Error));
        }
    }

    /// The `.txt` becomes a `.md` beside it, and the view moves to it.
    #[sismo::function("external_files::save_as_markdown")]
    fn save_as_markdown(&self, session: &Session) {
        let text = self.text.peek().clone();
        let target = sibling_markdown(&session.file.path);
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Err(err) = std::fs::write(&target, &text) {
            log::error!("save {}: {err}", target.display());
            Toasts::get().show(
                Toast::new(tr!("external-files-save-as-md-failed", "name" => name))
                    .tone(ToastTone::Error),
            );
            return;
        }
        let picked = Picked {
            name,
            bytes: text.into_bytes(),
        };
        if let Some(file) = super::ExternalFile::new(target, picked) {
            super::replace(session, file);
        }
    }
}

/// A `.md` path beside `path` that does not exist yet.
fn sibling_markdown(path: &std::path::Path) -> std::path::PathBuf {
    let first = path.with_extension("md");
    if !first.exists() {
        return first;
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    (1..1000)
        .map(|n| path.with_file_name(format!("{stem} ({n}).md")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

/// The ⋯ menu of the view.
#[sismo::function("external_files::menu")]
fn menu(session: &Session, anchor: Area) {
    let file = session.file.clone();
    let mut entries = Vec::new();
    if OWN_WINDOWS {
        if let Some(folder) = file.path.parent().map(std::path::Path::to_path_buf) {
            entries.push(ActionEntry::Item(
                Action::closing(tr!("external-files-show-in-folder"), move || {
                    crate::external::open_url(&format!("file://{}", folder.display()));
                })
                .icon(lucide::folder_open()),
            ));
        }
        let path = file.path;
        entries.push(ActionEntry::Item(
            Action::closing(tr!("external-files-open-with"), move || {
                crate::external::open_url(&format!("file://{}", path.display()));
            })
            .icon(lucide::external_link()),
        ));
    } else {
        let shared = file;
        entries.push(ActionEntry::Item(
            Action::closing(tr!("external-files-open-with"), move || {
                crate::flows::share_file(
                    shared.name.clone(),
                    shared.kind.mime().to_owned(),
                    shared.bytes().to_vec(),
                );
            })
            .icon(lucide::external_link()),
        ));
    }
    entries.push(ActionEntry::Divider);
    let closing = session.clone();
    entries.push(ActionEntry::Item(
        Action::closing(tr!("common-close"), move || super::close(&closing)).icon(lucide::x()),
    ));
    ActionList::new(entries).show_at(anchor);
}

/// The small square naming the type of the file.
fn badge(kind: FileKind, p: &Palette) -> Rect {
    let color = match kind {
        FileKind::Pdf => p.kinds.pdf,
        FileKind::Markdown | FileKind::Text => p.kinds.note,
    };
    rect()
        .height(Size::px(24.0))
        .padding((0.0, 6.0))
        .corner_radius(tokens::radius::XS)
        .background(color.with_a(46))
        .center()
        .child(
            label()
                .text(kind.badge())
                .font_size(tokens::text::CAPTION)
                .font_weight(FontWeight::BOLD)
                .color(color),
        )
}

/// Where the last edits went.
fn status(save: SaveState, location: Location, p: &Palette) -> Option<Rect> {
    let (text, color) = match save {
        SaveState::Idle => return None,
        SaveState::Saving => (tr!("external-files-saving"), p.t3),
        SaveState::Saved => (
            match location {
                Location::Original => tr!("external-files-saved-file"),
                Location::Copy => tr!("external-files-saved-copy"),
            },
            p.ok,
        ),
        SaveState::Failed => (tr!("external-files-save-failed"), p.danger),
    };
    Some(
        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(6.0)
            .child(
                rect()
                    .width(Size::px(7.0))
                    .height(Size::px(7.0))
                    .corner_radius(tokens::radius::FULL)
                    .background(color),
            )
            .child(Text::new(text).role(TextRole::Meta).color(color).lines(1)),
    )
}

impl ExternalView {
    fn mode_switch(state: &ViewState, full_width: bool) -> Segmented {
        let mut mode = state.mode;
        let selected = usize::from(*mode.read() == Mode::Edit);
        let switch = Segmented::new(
            [tr!("external-files-read"), tr!("external-files-edit")],
            selected,
        )
        .on_select(move |index: usize| {
            mode.set(if index == 1 { Mode::Edit } else { Mode::Read });
        });
        if full_width {
            switch.full_width()
        } else {
            switch
        }
    }

    fn primary(&self, state: &ViewState, full_width: bool) -> Button {
        let session = self.session.clone();
        let state = *state;
        let button = match *state.in_library.read() {
            Some(item) => Button::new(tr!("external-files-open-in-library"))
                .tonal()
                .icon(lucide::library())
                .on_press(move |_| {
                    if !OWN_WINDOWS {
                        super::close(&session);
                    }
                    super::in_main_window(move || crate::flows::open(item));
                }),
            None => Button::new(tr!("external-files-save-to-library"))
                .primary()
                .icon(lucide::library())
                .on_press(move |_| state.save_to_library(&session)),
        };
        if full_width {
            button.full_width()
        } else {
            button
        }
    }

    fn top_bar(&self, state: &ViewState, compact: bool, editable: bool, p: &Palette) -> Rect {
        let file = &self.session.file;
        let closing = self.session.clone();
        let menu_session = self.session.clone();
        rect()
            .width(Size::fill())
            .height(Size::px(tokens::size::BAR + 4.0))
            .padding((0.0, tokens::space::SM))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(tokens::space::SM)
            .border(
                Border::new()
                    .fill(p.line)
                    .width(BorderWidth {
                        bottom: 1.0,
                        ..BorderWidth::default()
                    })
                    .alignment(BorderAlignment::Inner),
            )
            .child(
                IconButton::new(lucide::x(), tr!("common-close"))
                    .on_press(move |_| super::close(&closing)),
            )
            .child(badge(file.kind, p))
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .spacing(2.0)
                    .child(Text::new(file.name.clone()).role(TextRole::Strong).lines(1))
                    .child(Text::new(file.details()).role(TextRole::Meta).lines(1)),
            )
            .maybe_child(
                (!compact)
                    .then(|| status(*state.save.read(), file.location, p))
                    .flatten(),
            )
            .maybe_child((!compact).then(|| Tag::new(tr!("external-files-pill"))))
            .maybe_child((!compact && editable).then(|| Self::mode_switch(state, false)))
            .maybe_child((!compact).then(|| self.primary(state, false)))
            .child(
                IconButton::new(lucide::ellipsis(), tr!("external-files-more")).on_press(
                    move |e: Event<PressEventData>| {
                        menu(&menu_session, notein_ui_kit::press_anchor(e.data()));
                    },
                ),
            )
    }

    /// The pill and where edits go, under the bar on phones.
    fn compact_strip(&self, state: &ViewState, p: &Palette) -> Rect {
        rect()
            .width(Size::fill())
            .padding((tokens::space::SM, tokens::space::GUTTER_COMPACT))
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(tokens::space::MD)
            .child(Tag::new(tr!("external-files-pill")))
            .maybe_child(status(*state.save.read(), self.session.file.location, p))
    }

    fn bottom_bar(&self, state: &ViewState, editable: bool, p: &Palette) -> Rect {
        rect()
            .width(Size::fill())
            .padding((tokens::space::SM, tokens::space::GUTTER_COMPACT))
            .spacing(tokens::space::SM)
            .background(p.s1)
            .border(
                Border::new()
                    .fill(p.line)
                    .width(BorderWidth {
                        top: 1.0,
                        ..BorderWidth::default()
                    })
                    .alignment(BorderAlignment::Inner),
            )
            .maybe_child(editable.then(|| Self::mode_switch(state, true)))
            .child(self.primary(state, true))
    }

    /// Notices about what can happen to edits, above the body.
    fn notices(&self, state: &ViewState, utf8: bool) -> Vec<Element> {
        let file = &self.session.file;
        let mut notices = Vec::new();
        if file.kind.editable() && !utf8 {
            notices
                .push(Notice::new(NoticeTone::Info, tr!("external-files-not-utf8")).into_element());
        } else if file.kind.editable() && !file.writable {
            notices.push(
                Notice::new(NoticeTone::Info, tr!("external-files-read-only"))
                    .detail(tr!("external-files-read-only-detail"))
                    .into_element(),
            );
        } else if file.kind.editable()
            && file.location == Location::Copy
            && *state.mode.read() == Mode::Edit
            && state.in_library.read().is_none()
        {
            notices.push(
                Notice::new(NoticeTone::Info, tr!("external-files-copy-detail")).into_element(),
            );
        }
        if *state.formatted.read() && !*state.answered.read() {
            let mut answered = state.answered;
            let session = self.session.clone();
            let state = *state;
            notices.push(
                Notice::new(NoticeTone::Warning, tr!("external-files-txt-notice"))
                    .action(
                        rect()
                            .horizontal()
                            .spacing(tokens::space::SM)
                            .child(
                                Button::new(tr!("external-files-got-it"))
                                    .ghost()
                                    .small()
                                    .on_press(move |_| answered.set(true)),
                            )
                            .child(
                                Button::new(tr!("external-files-save-as-md"))
                                    .tonal()
                                    .small()
                                    .on_press(move |_| {
                                        answered.set(true);
                                        state.save_as_markdown(&session);
                                    }),
                            ),
                    )
                    .into_element(),
            );
        }
        notices
    }
}

impl Component for ExternalView {
    #[sismo::function("external_files::view")]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let compact = width_class().is_compact();
        let insets = *crate::hooks::use_safe_area_insets().read();
        let state = ViewState::use_state(&self.session);
        let file = &self.session.file;
        let utf8 = use_hook(|| std::str::from_utf8(file.bytes()).is_ok());
        let editable = file.kind.editable() && file.writable && utf8;
        let notices = self.notices(&state, utf8);
        let gutter = if compact {
            tokens::space::GUTTER_COMPACT
        } else {
            tokens::space::LG
        };
        let body = match file.kind {
            FileKind::Markdown | FileKind::Text => super::text::TextBody {
                session: self.session.clone(),
                state,
                editable,
            }
            .into_element(),
            FileKind::Pdf => super::pdf::PdfBody {
                session: self.session.clone(),
            }
            .into_element(),
        };
        rect()
            .expanded()
            .background(p.bg)
            .padding((insets.top, insets.right, insets.bottom, insets.left))
            .content(Content::Flex)
            .child(self.top_bar(&state, compact, editable, &p))
            .maybe_child(compact.then(|| self.compact_strip(&state, &p)))
            .maybe_child((!notices.is_empty()).then(|| {
                rect()
                    .width(Size::fill())
                    .padding((tokens::space::SM, gutter))
                    .spacing(tokens::space::SM)
                    .children(notices)
            }))
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.0))
                    .child(body),
            )
            .maybe_child(compact.then(|| self.bottom_bar(&state, editable, &p)))
    }
}

//! Creating: the create menu (four tiles chosen by use plus a short
//! list), and what each entry does until the new document is open.

use std::sync::{Mutex, OnceLock, PoisonError};

use freya::prelude::*;
use notein_plugin::catalog::CatalogEntry;
use notein_plugin::create::{CreationFlow, CreationRequest, ImportedFile, Paper};
use notein_plugin::doc::DocMeta;
use notein_plugin::payload::Payload;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Action, ActionEntry, ActionList, Button, Group, Icon, ListRow, ModalController, Popover,
    PopoverAlign, PopoverSide, ProgressBar, Sheet, SheetFooter, Text, TextRole, Toast, ToastTone,
    Toasts, tokens, width_class,
};

use super::forms::{self, FormSpec, FormValues};
use crate::creation::{TILES, UsageLog};
use crate::library::{BackgroundStyle, DocKind, FolderId, ItemId, ScratchId, TabTarget};
use crate::persist::Persisted;
use crate::plugins::content;
use crate::references::kind_color;
use crate::settings::EntryKey;
use crate::space::OpenHint;

/// Where a creation lands and what it may take along.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Context {
    pub folder: FolderId,
    /// Documents selected in Home (a new folder can take them).
    pub selection: Vec<ItemId>,
}

fn key(entry: &CatalogEntry) -> EntryKey {
    EntryKey::new(entry.plugin.clone(), entry.entry.id.to_string())
}

/// Ranked entries in starting order: starters first by rank, then the
/// rest in catalog order.
fn ranked_entries() -> Vec<CatalogEntry> {
    let mut entries: Vec<CatalogEntry> = crate::plugins::catalog()
        .entries()
        .into_iter()
        .filter(|e| e.entry.ranked)
        .collect();
    entries.sort_by_key(|e| e.entry.starter_rank.unwrap_or(u8::MAX));
    entries
}

static SESSION_TILES: OnceLock<Mutex<Vec<EntryKey>>> = OnceLock::new();

/// The four tiles of this session, chosen once when the app opens.
fn session_tiles() -> Vec<EntryKey> {
    SESSION_TILES
        .get_or_init(|| {
            let available: Vec<EntryKey> = ranked_entries().iter().map(key).collect();
            let mut log = UsageLog::load();
            let tiles = log.choose_tiles(&available, crate::format::now());
            if let Err(err) = log.save() {
                log::warn!("usage log: {err}");
            }
            Mutex::new(tiles)
        })
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn record_use(entry: &CatalogEntry) {
    let mut log = UsageLog::load();
    log.record(key(entry), crate::format::now());
    if let Err(err) = log.save() {
        log::warn!("usage log: {err}");
    }
}

/// The tiles and whether the log is still in its starting state.
fn tiles() -> (Vec<CatalogEntry>, bool) {
    let entries = ranked_entries();
    let wanted = session_tiles();
    let mut tiles: Vec<CatalogEntry> = wanted
        .iter()
        .filter_map(|k| entries.iter().find(|e| key(e) == *k).cloned())
        .collect();
    for extra in &entries {
        if tiles.len() >= TILES {
            break;
        }
        if !tiles.iter().any(|t| key(t) == key(extra)) {
            tiles.push(extra.clone());
        }
    }
    (tiles, UsageLog::load().is_cold())
}

/// Open the create menu from the keyboard (Ctrl+N): near the top centre.
pub fn open_menu_from_keyboard(context: Context) {
    let size = *Platform::get().root_size.read();
    open_menu(
        Area::new((size.width / 2.0, 80.0).into(), (1.0, 1.0).into()),
        context,
    );
}

/// Open the create menu hanging from `anchor` (the + button).
pub fn open_menu(anchor: Area, context: Context) {
    let body = Menu { context };
    if width_class().is_compact() {
        Sheet::titled("Crear").child(body).open();
    } else {
        Popover::new(anchor, body)
            .width(440.0)
            .side(PopoverSide::Above)
            .align(PopoverAlign::End)
            .estimated_height(470.0)
            .open();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Menu {
    context: Context,
}

/// "Lo que más creás" (or "Para empezar" before there is history).
fn tiles_heading(cold: bool) -> Rect {
    let p = palette();
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::Flex)
        .spacing(6.0)
        .padding((0.0, 4.0))
        .child(Icon::new(freya::icons::lucide::sparkles()).size(14.0).color(p.t3))
        .child(
            Text::new(if cold {
                "Para empezar"
            } else {
                "Lo que más creás"
            })
            .role(TextRole::Label)
            .fill(),
        )
        .child(Text::new("Se ordena solo").role(TextRole::Meta))
}

impl Menu {
    /// The frequent entries as tiles, two per row.
    fn tile_rows(&self, tiles: &[CatalogEntry]) -> Vec<Element> {
        tiles
            .chunks(2)
            .map(|pair| {
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .spacing(8.0)
                    .children(pair.iter().map(|entry| {
                        Tile {
                            entry: entry.clone(),
                            context: self.context.clone(),
                        }
                        .into_element()
                    }))
                    .into_element()
            })
            .collect()
    }

    /// Carpeta, every other entry, Mesa nueva and Importar….
    fn list(&self, others: Vec<CatalogEntry>, imports: Vec<CatalogEntry>) -> Vec<ActionEntry> {
        use freya::icons::lucide;
        let p = palette();
        let context = &self.context;
        let mut list = vec![ActionEntry::Item(
            Action::new(
                if context.selection.is_empty() {
                    "Carpeta".to_owned()
                } else {
                    format!("Carpeta con {} seleccionados", context.selection.len())
                },
                {
                    let context = context.clone();
                    EventHandler::new(move |()| {
                        ModalController::get().close();
                        new_folder(context.clone());
                    })
                },
            )
            .icon(lucide::folder())
            .kind_color(p.kinds.folder),
        )];
        list.extend(others.into_iter().map(|entry| {
            let info = entry.entry.clone();
            let context = context.clone();
            ActionEntry::Item(
                Action::new(
                    info.label.clone(),
                    EventHandler::new(move |()| {
                        ModalController::get().close();
                        run(entry.clone(), context.clone());
                    }),
                )
                .subtitle(info.subtitle.clone())
                .icon(info.icon.clone())
                .kind_color(kind_color(&p, info.color)),
            )
        }));
        list.push(ActionEntry::Item(
            Action::new(
                "Mesa nueva",
                EventHandler::new(|()| {
                    ModalController::get().close();
                    new_mesa();
                }),
            )
            .icon(lucide::layout_grid())
            .kind_color(p.highlighters[2]),
        ));
        let import_names: Vec<String> = imports
            .iter()
            .map(|e| e.entry.label.to_string())
            .chain(std::iter::once("Bundle .notein".to_owned()))
            .collect();
        let context = context.clone();
        list.push(ActionEntry::Item(
            Action::new(
                "Importar…",
                EventHandler::new(move |()| import_submenu(&imports, &context)),
            )
            .subtitle(import_names.join(" · "))
            .icon(lucide::file_up())
            .submenu(),
        ));
        list
    }
}

impl Component for Menu {
    fn render(&self) -> impl IntoElement {
        let (tiles, cold) = tiles();
        let rest: Vec<CatalogEntry> = crate::plugins::catalog()
            .entries()
            .into_iter()
            .filter(|e| !tiles.iter().any(|t| key(t) == key(e)))
            .collect();
        let (imports, others): (Vec<_>, Vec<_>) = rest
            .into_iter()
            .partition(|e| matches!(e.entry.flow, CreationFlow::Import { .. }));
        rect()
            .width(Size::fill())
            .padding(if width_class().is_compact() { 0.0 } else { 12.0 })
            .spacing(10.0)
            .child(tiles_heading(cold))
            .children(self.tile_rows(&tiles))
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(1.0))
                    .background(palette().line),
            )
            .child(ActionList::new(self.list(others, imports)))
    }
}

fn import_submenu(imports: &[CatalogEntry], context: &Context) {
    use freya::icons::lucide;
    let p = palette();
    let mut entries: Vec<ActionEntry> = imports
        .iter()
        .map(|entry| {
            let (entry, info) = (entry.clone(), entry.entry.clone());
            let context = context.clone();
            ActionEntry::Item(
                Action::new(
                    info.label.clone(),
                    EventHandler::new(move |()| {
                        ModalController::get().close();
                        run(entry.clone(), context.clone());
                    }),
                )
                .subtitle(info.subtitle.clone())
                .icon(info.icon.clone())
                .kind_color(kind_color(&p, info.color)),
            )
        })
        .collect();
    entries.push(ActionEntry::Item(
        Action::new(
            "Bundle .notein",
            EventHandler::new(|()| {
                ModalController::get().close();
                super::transfer::import();
            }),
        )
        .subtitle("Restaurar o recibir una copia")
        .icon(lucide::package()),
    ));
    Sheet::titled("Importar")
        .on_back(|()| ModalController::get().close())
        .child(ActionList::new(entries))
        .push();
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Tile {
    entry: CatalogEntry,
    context: Context,
}

impl Component for Tile {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut hover = use_state(|| false);
        let info = &self.entry.entry;
        let color = kind_color(&p, info.color);
        let entry = self.entry.clone();
        let context = self.context.clone();
        rect()
            .width(Size::flex(1.0))
            .height(Size::px(if width_class().is_compact() {
                92.0
            } else {
                96.0
            }))
            .padding((12.0, 14.0))
            .corner_radius(tokens::radius::LG)
            .background(color.with_a(if hover() { 60 } else { 41 }))
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(info.label.to_string())
            .on_pointer_enter(move |_| hover.set(true))
            .on_pointer_leave(move |_| hover.set(false))
            .on_press(move |_| {
                ModalController::get().close();
                run(entry.clone(), context.clone());
            })
            .spacing(4.0)
            .main_align(Alignment::End)
            .child(Icon::new(info.icon.clone()).size(22.0).color(color))
            .child(
                label()
                    .text(info.label.clone())
                    .font_size(tokens::text::BODY_LG)
                    .font_weight(FontWeight::BOLD)
                    .color(p.t1),
            )
            .child(
                label()
                    .text(info.subtitle.clone())
                    .font_size(tokens::text::LABEL)
                    .color(p.t2)
                    .max_lines(1),
            )
    }
}

/// Run `entry`: show what its flow needs, then create and open.
pub fn run(entry: CatalogEntry, context: Context) {
    let kind_id = entry.kind.clone();
    if kind_id == crate::plugins::builtin::canvas::KIND {
        match entry.entry.id.as_ref() {
            crate::plugins::builtin::canvas::ENTRY_SCRATCH => {
                record_use(&entry);
                crate::space::open(TabTarget::Scratch(ScratchId::fresh()), OpenHint::Focused);
                return;
            }
            crate::plugins::builtin::canvas::ENTRY_NOTEBOOK => {
                super::pickers::document_picker(super::pickers::Purpose::NotebookFor);
                record_use(&entry);
                return;
            }
            _ => {}
        }
    }
    if crate::plugins::kind_info(&kind_id).is_some_and(|k| k.singleton) {
        let existing = crate::state::LibraryView::get()
            .data()
            .and_then(|d| d.live_items().find(|i| i.kind.is(&kind_id)).map(|i| i.id));
        if let Some(existing) = existing {
            record_use(&entry);
            super::open(existing);
            return;
        }
    }
    match entry.entry.flow.clone() {
        CreationFlow::Direct | CreationFlow::Custom => create(entry, context, Request::default()),
        CreationFlow::Form => form(entry, context),
        CreationFlow::Choose { title, options } => {
            choose(&entry, &context, title.to_string(), options);
        }
        CreationFlow::Import {
            title,
            extensions,
            mime_types,
        } => {
            spawn_forever(async move {
                let extensions: Vec<&str> = extensions.iter().map(AsRef::as_ref).collect();
                let mimes: Vec<&str> = mime_types.iter().map(AsRef::as_ref).collect();
                match crate::files::pick(&title, &extensions, &mimes).await {
                    Ok(Some(picked)) => import_sheet(entry, context, picked),
                    Ok(None) => {}
                    Err(err) => {
                        log::error!("pick: {err}");
                        Toasts::get().show(
                            Toast::new("No se pudo abrir el selector de archivos")
                                .tone(ToastTone::Error),
                        );
                    }
                }
            });
        }
    }
}

/// What the create flow collected.
#[derive(Clone, Debug, PartialEq, Default)]
struct Request {
    values: FormValues,
    file: Option<crate::files::Picked>,
}

const fn paper_of(style: BackgroundStyle) -> Paper {
    match style {
        BackgroundStyle::Blank => Paper::Blank,
        BackgroundStyle::Line => Paper::Lines,
        BackgroundStyle::Grid => Paper::Grid,
        BackgroundStyle::DotGrid => Paper::Dots,
    }
}

/// Last paper and colour picked for a new canvas.
#[istmo::message]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LastPaper {
    background: BackgroundStyle,
    color: Option<[u8; 4]>,
}

impl Persisted for LastPaper {
    const FILE: &'static str = "last-paper.v2.bin";
}

fn form(entry: CatalogEntry, context: Context) {
    let p = palette();
    let info = crate::plugins::kind_info(&entry.kind);
    let has_paper = info.as_ref().is_some_and(|k| k.has_paper);
    let is_note = entry.kind == notein_notes::kind::KIND;
    let last = LastPaper::load();
    let spec = FormSpec {
        title: if is_note {
            "Nueva nota".to_owned()
        } else {
            format!("Nuevo {}", entry.entry.label.to_lowercase())
        },
        kind_icon: Some(entry.entry.icon.clone()),
        kind_color: Some(kind_color(&p, entry.entry.color)),
        paper: has_paper,
        color: true,
        tags: true,
        location: !is_note,
        placeholder: "Sin título".to_owned(),
        hint: if is_note {
            "Vacío = toma el nombre de la primera línea".to_owned()
        } else {
            "Enter crea · Esc cancela".to_owned()
        },
        submit: "Crear y abrir".to_owned(),
        ..FormSpec::default()
    };
    let initial = FormValues {
        paper: if has_paper {
            last.background
        } else {
            BackgroundStyle::Blank
        },
        color: if has_paper { last.color } else { None },
        folder: context.folder,
        ..FormValues::default()
    };
    forms::open(spec, initial, move |values| {
        if has_paper {
            let remembered = LastPaper {
                background: values.paper,
                color: values.color,
            };
            if let Err(err) = remembered.save() {
                log::warn!("last paper: {err}");
            }
        }
        create(
            entry.clone(),
            context.clone(),
            Request { values, file: None },
        );
    });
}

fn choose(
    entry: &CatalogEntry,
    context: &Context,
    title: String,
    options: Vec<notein_plugin::create::CreationChoice>,
) {
    let mut sheet = Sheet::titled(title).kind(
        entry.entry.icon.clone(),
        kind_color(&palette(), entry.entry.color),
    );
    let rows: Vec<Element> = options
        .into_iter()
        .map(|option| {
            let entry = entry.clone();
            let context = context.clone();
            let id = option.id.to_string();
            ListRow::new(option.label.clone())
                .subtitle(option.subtitle.clone())
                .chevron()
                .on_press(move |_| {
                    ModalController::get().close();
                    create(
                        entry.clone(),
                        context.clone(),
                        Request {
                            values: FormValues {
                                choice: Some(id.clone()),
                                folder: context.folder,
                                ..FormValues::default()
                            },
                            file: None,
                        },
                    );
                })
                .into_element()
        })
        .collect();
    sheet = sheet.child(Group::default().children(rows));
    if entry.kind == notein_bible::kind::KIND {
        sheet = sheet.child(
            Button::new("Agregar traducción")
                .icon(freya::icons::lucide::plus())
                .ghost()
                .on_press(|_| {
                    ModalController::get().close();
                    crate::screens::bible::open_catalog();
                }),
        );
    }
    sheet
        .footer(SheetFooter::new([Button::new("Cancelar")
            .ghost()
            .on_press(|_| ModalController::get().close())
            .into_element()]))
        .open();
}

fn import_sheet(entry: CatalogEntry, context: Context, picked: crate::files::Picked) {
    let p = palette();
    let is_pdf = entry.kind == crate::plugins::builtin::pdf::KIND;
    let is_markdown = entry.kind == notein_notes::kind::KIND;
    let (name, file_line, preview) = if is_pdf {
        match crate::plugins::builtin::pdf::PdfKind::read_file(&picked.name, &picked.bytes) {
            Ok((_, pdf)) => (
                crate::plugins::builtin::pdf::name_from_file(&picked.name),
                Some(format!(
                    "{} · {} · {}",
                    crate::format::count(pdf.page_count as usize, "página", "páginas"),
                    crate::format::size(pdf.size_bytes),
                    picked.name
                )),
                None,
            ),
            Err(err) => {
                import_error(&entry, context, &picked.name, &err.to_string());
                return;
            }
        }
    } else if is_markdown {
        let text = String::from_utf8_lossy(&picked.bytes).into_owned();
        let headings = freya_markdown_editor::document::Document::parse(&text)
            .outline()
            .len();
        let words = text.split_whitespace().count();
        (
            notein_notes::import_name(Some(&picked.name), &text),
            Some(format!(
                "{} · {} · {}",
                picked.name,
                crate::format::count(headings, "título", "títulos"),
                crate::format::count(words, "palabra", "palabras")
            )),
            Some(text.lines().take(8).collect::<Vec<_>>().join("\n")),
        )
    } else {
        (picked.name.clone(), Some(picked.name.clone()), None)
    };
    let spec = FormSpec {
        title: format!("Importar {}", entry.entry.label),
        kind_icon: Some(entry.entry.icon.clone()),
        kind_color: Some(kind_color(&p, entry.entry.color)),
        color: !is_pdf,
        tags: true,
        location: true,
        notebook_toggle: is_pdf,
        file_line,
        preview,
        placeholder: "Sin título".to_owned(),
        submit: "Importar y abrir".to_owned(),
        ..FormSpec::default()
    };
    let initial = FormValues {
        name,
        folder: context.folder,
        ..FormValues::default()
    };
    forms::open(spec, initial, move |values| {
        create(
            entry.clone(),
            context.clone(),
            Request {
                values,
                file: Some(picked.clone()),
            },
        );
    });
}

fn import_error(entry: &CatalogEntry, context: Context, file: &str, why: &str) {
    let entry = entry.clone();
    Sheet::titled(format!("Importar {}", entry.entry.label))
        .child(
            notein_ui_kit::Notice::new(
                notein_ui_kit::NoticeTone::Danger,
                format!("No se pudo leer «{file}»"),
            )
            .detail(why.to_owned()),
        )
        .footer(SheetFooter::new([
            Button::new("Cancelar")
                .ghost()
                .on_press(|_| ModalController::get().close())
                .into_element(),
            Button::new("Elegir otro archivo")
                .primary()
                .on_press(move |_| {
                    ModalController::get().close();
                    run(entry.clone(), context.clone());
                })
                .into_element(),
        ]))
        .open();
}

/// Sheet shown while a large import prepares its pages.
#[derive(Clone, Copy, PartialEq)]
struct Preparing {
    done: State<u32>,
    total: u32,
}

impl Component for Preparing {
    fn render(&self) -> impl IntoElement {
        let done = *self.done.read();
        #[allow(clippy::cast_precision_loss)]
        let fraction = if self.total == 0 {
            0.0
        } else {
            done as f32 / self.total as f32
        };
        rect()
            .width(Size::fill())
            .spacing(10.0)
            .child(
                Text::new(format!("Preparando páginas · {done} de {}", self.total))
                    .role(TextRole::Meta),
            )
            .child(ProgressBar::new(fraction).color(palette().kinds.pdf))
    }
}

/// Pages above which the import shows its progress sheet.
const LARGE_PDF_PAGES: u32 = 40;

fn create(entry: CatalogEntry, context: Context, request: Request) {
    spawn_forever(async move {
        let Some(kind) = crate::plugins::kind(&entry.kind) else {
            return;
        };
        let values = request.values;
        let file = request.file;
        let created = {
            let req = CreationRequest {
                name: &values.name,
                paper: paper_of(values.paper),
                paper_color: values.color.map(|[r, g, b, _]| [r, g, b]),
                file: file.as_ref().map(|f| ImportedFile {
                    name: &f.name,
                    bytes: &f.bytes,
                }),
                choice: values.choice.as_deref(),
            };
            kind.create(&entry.entry.id, req)
        };
        let created = match created {
            Ok(created) => created,
            Err(err) => {
                if let Some(file) = &file {
                    import_error(&entry, context, &file.name, &err.to_string());
                } else {
                    Toasts::get().show(Toast::new(err.to_string()).tone(ToastTone::Error));
                }
                return;
            }
        };
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let item = {
            let mut lib = handle.lock().await;
            let tags = forms::resolve_tags(&mut lib, &values.tags).await;
            let name = if created.name.trim().is_empty() {
                "Sin título".to_owned()
            } else {
                created.name.clone()
            };
            let item = match lib
                .create_item(
                    values.folder,
                    DocKind::plugin(entry.kind.clone()),
                    &name,
                    values.color,
                    tags,
                    values.paper,
                )
                .await
            {
                Ok(item) => item,
                Err(err) => {
                    log::error!("create item: {err}");
                    Toasts::get().show(Toast::new("No se pudo crear").tone(ToastTone::Error));
                    return;
                }
            };
            content::write(item, &entry.kind, &created.content);
            if let Some(attachment) = &created.attachment
                && let Err(err) = lib.attach_pdf(item, attachment).await
            {
                log::error!("attach: {err}");
            }
            drop(lib);
            item
        };
        record_use(&entry);
        if let Some(attachment) = created.attachment {
            prepare_attachment(item, &entry.kind, attachment);
        } else {
            crate::plugins::content::reindex(item);
        }
        if values.notebook {
            let notebook = {
                let mut lib = handle.lock().await;
                crate::notebooks::create_notebook(&mut lib, item).await
            };
            super::open(item);
            if let Ok(notebook) = notebook {
                crate::space::open(TabTarget::Item(notebook), OpenHint::Beside);
            }
        } else {
            super::open(item);
        }
    });
}

/// Index an attachment's text and render its cover in the background,
/// showing progress for large files.
fn prepare_attachment(item: ItemId, kind: &str, bytes: Vec<u8>) {
    if kind != crate::plugins::builtin::pdf::KIND {
        crate::plugins::content::reindex(item);
        return;
    }
    let Ok(pdf) = freya_pdf::PdfDocument::open_bytes(bytes) else {
        return;
    };
    let total = u32::try_from(pdf.page_count()).unwrap_or(u32::MAX);
    let progress = if total >= LARGE_PDF_PAGES {
        let done = State::create_in_scope(0u32, ScopeId::ROOT);
        Sheet::titled("Importando…")
            .child(Preparing { done, total })
            .footer(SheetFooter::new([Button::new("Seguir en segundo plano")
                .on_press(|_| ModalController::get().close())
                .into_element()]))
            .open();
        Some(done)
    } else {
        None
    };
    let (tx, rx) = flume::unbounded::<u32>();
    std::thread::spawn(move || {
        if let Some(cover) = crate::thumbnail::pdf(&pdf)
            && let Ok(handle) = crate::library::shared_blocking()
        {
            let mut lib = pollster::block_on(handle.lock());
            if let Err(err) = pollster::block_on(lib.set_item_thumbnail(item, Some(cover))) {
                log::error!("pdf cover: {err}");
            }
        }
        let kind = crate::plugins::builtin::pdf_kind();
        let pages = kind.extract_text(crate::plugins::content::doc_id(item), &pdf, |done, _| {
            let _ = tx.send(done);
        });
        kind.store_text(crate::plugins::content::doc_id(item), pages);
    });
    if let Some(mut done) = progress {
        spawn_forever(async move {
            while let Ok(value) = rx.recv_async().await {
                done.set(value);
            }
            if ModalController::get().is_open() {
                ModalController::get().close();
            }
        });
    }
}

/// Swap the file under PDF `item` for another one. Ink, bookmarks and
/// notebooks stay: they belong to the document, not to the file.
pub fn replace_pdf(item: ItemId) {
    use crate::plugins::builtin::pdf::{KIND, PdfContent, PdfKind};
    spawn(async move {
        let picked = match crate::files::pick("Reemplazar PDF", &["pdf"], &["application/pdf"]).await
        {
            Ok(Some(picked)) => picked,
            Ok(None) => return,
            Err(err) => {
                log::warn!("pick pdf: {err}");
                return;
            }
        };
        let mut fresh = match PdfKind::read_file(&picked.name, &picked.bytes) {
            Ok((_, fresh)) => fresh,
            Err(err) => {
                replace_failed(item, &picked.name, &err.to_string());
                return;
            }
        };
        if let Some(old) = content::read(item, KIND).and_then(|b| PdfContent::from_bytes(&b).ok()) {
            fresh.marks = old.marks;
        }
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        if let Err(err) = handle.lock().await.attach_pdf(item, &picked.bytes).await {
            log::error!("replace pdf: {err}");
            Toasts::get().show(Toast::new("No se pudo reemplazar el archivo").tone(ToastTone::Error));
            return;
        }
        content::write(item, KIND, &fresh.to_bytes());
        crate::builtin::pdf_face::reload(item);
        prepare_attachment(item, KIND, picked.bytes);
        Toasts::get().show(Toast::new(format!("Ahora muestra «{}»", picked.name)));
    });
}

fn replace_failed(item: ItemId, file: &str, why: &str) {
    Sheet::titled("Reemplazar PDF")
        .child(
            notein_ui_kit::Notice::new(
                notein_ui_kit::NoticeTone::Danger,
                format!("No se pudo leer «{file}»"),
            )
            .detail(why.to_owned()),
        )
        .footer(SheetFooter::new([
            Button::new("Cancelar")
                .ghost()
                .on_press(|_| ModalController::get().close())
                .into_element(),
            Button::new("Elegir otro archivo")
                .primary()
                .on_press(move |_| {
                    ModalController::get().close();
                    replace_pdf(item);
                })
                .into_element(),
        ]))
        .open();
}

/// A new folder, optionally taking the selected documents along.
pub fn new_folder(context: Context) {
    let p = palette();
    let selected = context.selection.len();
    let spec = FormSpec {
        title: "Nueva carpeta".to_owned(),
        kind_icon: Some(freya::icons::lucide::folder()),
        kind_color: Some(p.kinds.folder),
        color: true,
        tags: true,
        location: true,
        move_selected: (selected > 0).then_some(selected),
        placeholder: "Sin nombre".to_owned(),
        submit: "Crear".to_owned(),
        ..FormSpec::default()
    };
    let initial = FormValues {
        folder: context.folder,
        move_selected: selected > 0,
        ..FormValues::default()
    };
    forms::open(spec, initial, move |values| {
        let selection = context.selection.clone();
        crate::state::edit("crear la carpeta", async move |lib| {
            let tags = forms::resolve_tags(lib, &values.tags).await;
            let name = if values.name.trim().is_empty() {
                "Carpeta nueva"
            } else {
                values.name.trim()
            };
            let folder = lib
                .create_folder(values.folder, name, values.color, tags)
                .await?;
            if values.move_selected {
                for item in selection {
                    lib.move_item(item, folder).await?;
                }
            }
            Ok(())
        });
    });
}

/// How a new mesa starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum StartWith {
    #[default]
    Empty,
    OpenNow,
    Pick,
}

#[derive(Clone, Copy, PartialEq)]
struct MesaForm {
    name: State<String>,
    start: State<StartWith>,
    picked: State<Vec<ItemId>>,
}

impl Component for MesaForm {
    fn render(&self) -> impl IntoElement {
        let mut start = self.start;
        let mut picked = self.picked;
        let current = *start.read();
        let data = crate::state::LibraryView::get().data();
        let recent: Vec<(ItemId, String)> = data
            .as_deref()
            .map(|d| {
                let mut items: Vec<_> = d
                    .live_items()
                    .map(|i| (i.updated_at, i.id, i.name.clone()))
                    .collect();
                items.sort_by_key(|(at, _, _)| std::cmp::Reverse(*at));
                items
                    .into_iter()
                    .take(12)
                    .map(|(_, id, name)| (id, name))
                    .collect()
            })
            .unwrap_or_default();
        let chosen = picked.read().clone();
        rect()
            .width(Size::fill())
            .spacing(16.0)
            .child(
                notein_ui_kit::TextField::new(self.name)
                    .label("Nombre")
                    .placeholder("Toma el nombre de lo que abras")
                    .auto_focus(true),
            )
            .child(
                notein_ui_kit::Section::new("Empezar con").child(
                    notein_ui_kit::Segmented::new(
                        ["Vacía", "Lo abierto ahora", "Elegir documentos"],
                        match current {
                            StartWith::Empty => 0,
                            StartWith::OpenNow => 1,
                            StartWith::Pick => 2,
                        },
                    )
                    .full_width()
                    .on_select(move |i: usize| {
                        start.set(match i {
                            1 => StartWith::OpenNow,
                            2 => StartWith::Pick,
                            _ => StartWith::Empty,
                        });
                    }),
                ),
            )
            .maybe_child((current == StartWith::Pick).then(|| {
                Group::default().children(recent.into_iter().map(|(id, name)| {
                    let on = chosen.contains(&id);
                    ListRow::new(name)
                        .leading(notein_ui_kit::Leading::Check(on))
                        .on_press(move |_| {
                            let mut list = picked.write();
                            if let Some(at) = list.iter().position(|x| *x == id) {
                                list.remove(at);
                            } else {
                                list.push(id);
                            }
                        })
                        .into_element()
                }))
            }))
    }
}

/// A new mesa: empty, with what is open now, or with chosen documents.
pub fn new_mesa() {
    let name = State::create_in_scope(String::new(), ScopeId::ROOT);
    let start = State::create_in_scope(StartWith::Empty, ScopeId::ROOT);
    let picked = State::create_in_scope(Vec::<ItemId>::new(), ScopeId::ROOT);
    Sheet::titled("Nueva mesa")
        .kind(
            freya::icons::lucide::layout_grid(),
            palette().highlighters[2],
        )
        .subtitle("Guardá un grupo de documentos abiertos juntos.")
        .child(MesaForm {
            name,
            start,
            picked,
        })
        .footer(SheetFooter::new([
            Button::new("Cancelar")
                .ghost()
                .on_press(|_| ModalController::get().close())
                .into_element(),
            Button::new("Crear mesa")
                .primary()
                .on_press(move |_| {
                    ModalController::get().close();
                    let title = name.peek().clone();
                    let how = *start.peek();
                    let chosen = picked.peek().clone();
                    spawn_forever(async move {
                        let Ok(handle) = crate::library::shared().await else {
                            return;
                        };
                        let mut lib = handle.lock().await;
                        let open_now: Vec<TabTarget> = lib
                            .active_workspace()
                            .and_then(|w| lib.workspace(w))
                            .map(|w| {
                                w.layout
                                    .leaves()
                                    .into_iter()
                                    .flat_map(|p| p.tabs.iter().map(|t| t.target))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let workspace = match lib.create_workspace(&title, None).await {
                            Ok(w) => w,
                            Err(err) => {
                                log::error!("new mesa: {err}");
                                return;
                            }
                        };
                        let targets: Vec<TabTarget> = match how {
                            StartWith::Empty => Vec::new(),
                            StartWith::OpenNow => open_now,
                            StartWith::Pick => chosen.into_iter().map(TabTarget::Item).collect(),
                        };
                        if let Err(err) = lib.set_active_workspace(Some(workspace)).await {
                            log::error!("activate mesa: {err}");
                        }
                        for target in targets {
                            if let Err(err) =
                                crate::space::open_in(&mut lib, target, OpenHint::Focused).await
                            {
                                log::error!("mesa tab: {err}");
                            }
                        }
                        drop(lib);
                        crate::space::open_space(workspace);
                    });
                })
                .into_element(),
        ]))
        .open();
}

/// Change the translation the Bible document reads.
pub fn set_bible_translation(item: ItemId, bible_id: &str) {
    let kind = notein_bible::kind::KIND;
    let mut content = content::read(item, kind)
        .and_then(|b| notein_bible::BibleContent::from_bytes(&b).ok())
        .unwrap_or_default();
    if content.bible_id == bible_id {
        return;
    }
    bible_id.clone_into(&mut content.bible_id);
    content::write(item, kind, &content.to_bytes());
    content::notify_remote(item);
}

/// Miniature of a paper pattern for the paper picker.
#[must_use]
pub fn paper_swatch(style: BackgroundStyle) -> Element {
    let line = Color::from_rgb(200, 195, 184);
    match style {
        BackgroundStyle::Blank => rect().into_element(),
        BackgroundStyle::Line => rect()
            .padding((10.0, 0.0))
            .width(Size::fill())
            .spacing(9.0)
            .children((0..5).map(|i| {
                rect()
                    .key(i)
                    .width(Size::fill())
                    .height(Size::px(1.0))
                    .background(line)
                    .into_element()
            }))
            .into_element(),
        BackgroundStyle::Grid => rect()
            .width(Size::fill())
            .spacing(9.0)
            .padding(4.0)
            .children((0..6).map(|i| {
                rect()
                    .key(i)
                    .width(Size::fill())
                    .height(Size::px(9.0))
                    .border(
                        Border::new()
                            .fill(line)
                            .width(1.0)
                            .alignment(BorderAlignment::Inner),
                    )
                    .into_element()
            }))
            .into_element(),
        BackgroundStyle::DotGrid => rect()
            .width(Size::fill())
            .padding(8.0)
            .spacing(9.0)
            .children((0..5).map(|r| {
                rect()
                    .key(r)
                    .horizontal()
                    .spacing(9.0)
                    .children((0..8).map(|c| {
                        rect()
                            .key(c)
                            .width(Size::px(2.0))
                            .height(Size::px(2.0))
                            .corner_radius(1.0)
                            .background(line)
                            .into_element()
                    }))
                    .into_element()
            }))
            .into_element(),
    }
}

/// Every creation entry by key, for the FAB shortcut bubbles.
#[must_use]
pub fn entry_for(shortcut: &EntryKey) -> Option<CatalogEntry> {
    crate::plugins::catalog()
        .entries()
        .into_iter()
        .find(|e| e.plugin == shortcut.plugin && e.entry.id == shortcut.entry)
}

/// The doc meta of an item for kind calls.
#[must_use]
pub fn meta(item: &crate::library::Item) -> DocMeta<'_> {
    DocMeta {
        id: content::doc_id(item.id),
        name: &item.name,
        updated_at: item.updated_at,
    }
}

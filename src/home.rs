//! Home / library grid rendered by [`crate::route::Route::Home`].
//!
//! Layout mirrors Noteshelf's landing page:
//!
//! * top action bar (menu / notifications / sync on the left, home crest
//!   in the middle, multiselect / search / settings on the right),
//! * breadcrumb + toolbar row (`Inicio > Folder > …` on the left, view
//!   / sort affordances on the right),
//! * a wrapping grid of tinted "notebook" cards — folders and items
//!   sit side by side and sort by most-recently-touched first,
//! * a floating FAB stack (pen = new blank canvas, `+` = new folder)
//!   anchored bottom-right.
//!
//! Navigation is single-view: a `nav` [`Vec<FolderId>`] tracks the
//! stack of entered folders. Tapping a folder pushes it; a breadcrumb
//! crumb truncates the stack to that depth. Tapping an item pushes
//! [`Route::CanvasView`] (both canvas and PDF-backed items go there
//! for now — per-item body loading is a follow-up).

use std::sync::Arc;

use async_lock::Mutex as AsyncMutex;
use freya::prelude::*;
use freya::router::*;
use istmo::plugins::EdgeInsets;

use crate::components::{auto_color, CanvasCreateSheet, CreateCanvasRequest};
use crate::hooks::use_safe_area_insets;
use crate::library::{
    BackgroundStyle, Folder, FolderId, Item, ItemKind, Library, LibraryIndex, ROOT_FOLDER, Rgba,
    TagId,
};
use crate::route::Route;

/// Shared library handle. Wrapped in an async-aware mutex so every
/// `spawn`ed handler can `.await` the lock without blocking the freya
/// executor.
type LibHandle = Arc<AsyncMutex<Library>>;

/// Bring-up state for the async library open. Split from the loaded
/// handle so `Loading` / `Error` branches don't need a dummy Arc.
#[derive(Clone)]
enum LibState {
    Loading,
    Ready(LibHandle),
    Error(String),
}

/// Landing page.
#[derive(Debug, PartialEq)]
pub struct Home;

impl Component for Home {
    fn render(&self) -> impl IntoElement {
        let lib_state = use_state(|| LibState::Loading);
        let snap = use_state(|| Option::<LibraryIndex>::None);
        let nav = use_state(Vec::<FolderId>::new);
        let pad = *use_safe_area_insets().read();

        use_hook({
            let mut lib_state = lib_state;
            let mut snap = snap;
            move || {
                spawn(async move {
                    match open_library().await {
                        Ok(handle) => {
                            let index = handle.lock().await.index().clone();
                            snap.set(Some(index));
                            lib_state.set(LibState::Ready(handle));
                        }
                        Err(msg) => {
                            log::error!("library open failed: {msg}");
                            lib_state.set(LibState::Error(msg));
                        }
                    }
                });
            }
        });

        let bg = Color::from_rgb(15, 15, 18);
        let state_val = lib_state.read().clone();
        let snapshot = snap.read().clone();
        let nav_stack = nav.read().clone();
        let current = nav_stack.last().copied().unwrap_or(ROOT_FOLDER);

        let mut column = rect()
            .vertical()
            .expanded()
            .padding((pad.top, pad.right, 0.0, pad.left))
            .child(top_bar());
        let mut fab_slot: Option<Rect> = None;

        match (state_val, snapshot) {
            (LibState::Loading, _) => {
                column = column.child(centered("Cargando biblioteca…"));
            }
            (LibState::Error(msg), _) => {
                column = column.child(centered(&format!("No pude abrir la biblioteca: {msg}")));
            }
            (LibState::Ready(handle), Some(index)) => {
                column = column
                    .child(breadcrumb_bar(&index, &nav_stack, nav))
                    .child(grid(&index, current, snap, nav));
                fab_slot = Some(fab_stack(handle, snap, current, pad));
            }
            (LibState::Ready(_), None) => {
                column = column.child(centered("Preparando…"));
            }
        }

        let mut root = rect().expanded().background(bg).child(column);
        if let Some(fab) = fab_slot {
            root = root.child(fab);
        }
        root
    }
}

// ---------------------------------------------------------------------------
// Top bar
// ---------------------------------------------------------------------------

fn top_bar() -> impl IntoElement {
    let bg = Color::from_rgb(15, 15, 18);
    rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(56.0))
        .padding((10.0, 16.0))
        .background(bg)
        .cross_align(Alignment::Center)
        .child(icon_button("☰"))
        .child(spacer_px(6.0))
        .child(icon_button("🔔"))
        .child(spacer_px(6.0))
        .child(icon_button("☁"))
        .child(flexible_spacer())
        .child(icon_button("🏠"))
        .child(flexible_spacer())
        .child(icon_button("✔"))
        .child(spacer_px(6.0))
        .child(icon_button("🔍"))
        .child(spacer_px(6.0))
        .child(icon_button("⚙"))
}

fn icon_button(glyph: &'static str) -> impl IntoElement {
    rect()
        .width(Size::px(32.0))
        .height(Size::px(32.0))
        .center()
        .child(
            label()
                .color(Color::from_rgb(210, 210, 220))
                .font_size(16.0)
                .text(glyph),
        )
}

fn flexible_spacer() -> impl IntoElement {
    rect().width(Size::fill()).height(Size::px(1.0))
}

fn spacer_px(px: f32) -> impl IntoElement {
    rect().width(Size::px(px)).height(Size::px(1.0))
}

// ---------------------------------------------------------------------------
// Breadcrumb + view toolbar
// ---------------------------------------------------------------------------

fn breadcrumb_bar(
    index: &LibraryIndex,
    nav_stack: &[FolderId],
    nav: State<Vec<FolderId>>,
) -> impl IntoElement {
    let mut row = rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(44.0))
        .padding((6.0, 20.0))
        .cross_align(Alignment::Center);

    row = row.child(crumb("Inicio", nav_stack.is_empty(), move |()| {
        let mut nav = nav;
        nav.set(Vec::new());
    }));

    for (i, folder_id) in nav_stack.iter().enumerate() {
        let name = index
            .folder(*folder_id)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "?".to_owned());
        let is_last = i + 1 == nav_stack.len();
        let depth = i + 1;
        row = row.child(crumb_separator());
        row = row.child(crumb(&name, is_last, move |()| {
            let mut nav = nav;
            nav.write().truncate(depth);
        }));
    }

    row.child(flexible_spacer())
        .child(icon_button("🎨"))
        .child(spacer_px(6.0))
        .child(icon_button("≡"))
        .child(spacer_px(6.0))
        .child(sort_pill())
}

fn crumb<F: Fn(()) + 'static>(text: &str, active: bool, handler: F) -> impl IntoElement {
    let color = if active {
        Color::from_rgb(240, 240, 245)
    } else {
        Color::from_rgb(150, 150, 165)
    };
    rect()
        .padding((4.0, 6.0))
        .child(label().color(color).font_size(13.0).text(text.to_owned()))
        .on_press(move |_| handler(()))
}

fn crumb_separator() -> impl IntoElement {
    rect().padding((4.0, 4.0)).child(
        label()
            .color(Color::from_rgb(90, 90, 100))
            .font_size(13.0)
            .text("›"),
    )
}

fn sort_pill() -> impl IntoElement {
    rect()
        .horizontal()
        .padding((6.0, 12.0))
        .background(Color::from_rgb(30, 30, 36))
        .with_corner_radius(14.0)
        .cross_align(Alignment::Center)
        .child(
            label()
                .color(Color::from_rgb(210, 210, 220))
                .font_size(12.0)
                .text("⇅  Clasificar"),
        )
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

fn grid(
    index: &LibraryIndex,
    current: FolderId,
    snap: State<Option<LibraryIndex>>,
    nav: State<Vec<FolderId>>,
) -> impl IntoElement {
    let mut folders: Vec<&Folder> = index.folders.iter().filter(|f| f.parent == current).collect();
    let mut items: Vec<&Item> = index.items.iter().filter(|i| i.folder == current).collect();
    folders.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    if folders.is_empty() && items.is_empty() {
        return rect()
            .width(Size::fill())
            .height(Size::fill())
            .center()
            .child(
                label()
                    .color(Color::from_rgb(120, 120, 135))
                    .font_size(13.0)
                    .text("Esta carpeta está vacía. Usa el botón + para crear una nota."),
            );
    }

    let mut wrap = rect()
        .width(Size::fill())
        .height(Size::fill())
        .padding((12.0, 24.0))
        .spacing(20.0)
        .content(Content::wrap());

    for folder in folders {
        let folder_id = folder.id;
        let child_count = index.items.iter().filter(|i| i.folder == folder_id).count()
            + index.folders.iter().filter(|f| f.parent == folder_id).count();
        let subtitle = format!("{} notas · {}", child_count, format_date(folder.updated_at));
        wrap = wrap.child(card(
            folder.name.clone(),
            subtitle,
            folder
                .color
                .unwrap_or_else(|| color_to_rgba(auto_color(&folder.name))),
            true,
            move |()| {
                let mut nav = nav;
                nav.write().push(folder_id);
            },
        ));
    }

    for item in items {
        let kind_label = match item.kind {
            ItemKind::Canvas => "Lienzo",
            ItemKind::PdfCanvas => "PDF",
        };
        let subtitle = format!("{} · {}", kind_label, format_date(item.updated_at));
        wrap = wrap.child(card(
            item.name.clone(),
            subtitle,
            item.color
                .unwrap_or_else(|| color_to_rgba(auto_color(&item.name))),
            false,
            move |_| {
                let _ = RouterContext::get().push(Route::CanvasView);
            },
        ));
    }

    let _ = snap;
    wrap
}

fn card<F: Fn(()) + 'static>(
    title: String,
    subtitle: String,
    tint: Rgba,
    is_folder: bool,
    handler: F,
) -> impl IntoElement {
    let cover = rect()
        .width(Size::px(130.0))
        .height(Size::px(160.0))
        .background(rgba_to_color(tint))
        .with_corner_radius(8.0)
        .center()
        .child(
            label()
                .color(Color::from_argb(180, 20, 20, 20))
                .font_size(32.0)
                .text(if is_folder { "📁" } else { "📓" }),
        );

    rect()
        .vertical()
        .width(Size::px(140.0))
        .padding((4.0, 4.0))
        .spacing(6.0)
        .child(cover)
        .child(
            label()
                .color(Color::from_rgb(230, 230, 235))
                .font_size(12.0)
                .text(title),
        )
        .child(
            label()
                .color(Color::from_rgb(130, 130, 145))
                .font_size(10.0)
                .text(subtitle),
        )
        .on_press(move |_| handler(()))
}

fn centered(text: &str) -> impl IntoElement {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .center()
        .child(
            label()
                .color(Color::from_rgb(180, 180, 195))
                .font_size(13.0)
                .text(text.to_owned()),
        )
}

// ---------------------------------------------------------------------------
// FAB overlay
// ---------------------------------------------------------------------------

fn fab_stack(
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    current: FolderId,
    pad: EdgeInsets,
) -> Rect {
    use crate::components::{
        CanvasCreateSheet, CreateCanvasRequest, CreateFolderRequest, FabMenu, FabMenuEntry,
        FolderCreateSheet,
    };

    let pen_bg = Color::from_rgb(80, 130, 175);
    let plus_bg = Color::from_rgb(65, 105, 220);

    let pen = fab_button("✎", pen_bg, {
        let handle = handle.clone();
        move |_| {
            let handle = handle.clone();
            let mut snap = snap;
            spawn_forever(async move {
                let mut lib = handle.lock().await;
                match lib
                    .create_item(
                        current,
                        ItemKind::Canvas,
                        "Borrador",
                        None,
                        Vec::new(),
                        BackgroundStyle::default(),
                    )
                    .await
                {
                    Ok(_) => {
                        snap.set(Some(lib.index().clone()));
                        let _ = RouterContext::get().push(Route::CanvasView);
                    }
                    Err(err) => log::error!("create_item: {err}"),
                }
            });
        }
    });

    // Anchor of the FAB stack, reused when placing the popup above it.
    let anchor_right = 24.0 + pad.right;
    let anchor_bottom = 24.0 + pad.bottom;
    // Pen + plus each 52px tall, 12px stack spacing, 12px extra breathing
    // room between the stack and the menu card.
    let menu_bottom = anchor_bottom + 52.0 + 12.0 + 52.0 + 12.0;

    let plus = fab_button("+", plus_bg, move |_| {
        let handle = handle.clone();
        FabMenu::new()
            .anchor(anchor_right, menu_bottom)
            .entry(FabMenuEntry::new("▢", "Lienzo infinito", {
                let handle = handle.clone();
                move || {
                    let handle = handle.clone();
                    let available = current_tag_names(snap);
                    CanvasCreateSheet::new(move |req: CreateCanvasRequest| {
                        log::info!(
                            "canvas confirm: name={:?} kind={:?} tags={:?}",
                            req.name,
                            req.kind,
                            req.tag_names
                        );
                        let handle = handle.clone();
                        let mut snap = snap;
                        spawn_forever(async move {
                            let mut lib = handle.lock().await;
                            let tag_ids = resolve_tag_names(&mut lib, &req.tag_names).await;
                            log::info!("canvas create → parent={current:?} tag_ids={tag_ids:?}");
                            match lib
                                .create_item(
                                    current,
                                    req.kind,
                                    &req.name,
                                    Some(color_to_rgba(req.color)),
                                    tag_ids,
                                    req.background,
                                )
                                .await
                            {
                                Ok(id) => {
                                    log::info!("canvas created id={id:?}");
                                    snap.set(Some(lib.index().clone()));
                                    let _ = RouterContext::get().push(Route::CanvasView);
                                }
                                Err(err) => log::error!("create_item: {err}"),
                            }
                        });
                    })
                    .kind(ItemKind::Canvas)
                    .available_tags(available)
                    .open();
                }
            }))
            .entry(FabMenuEntry::new("🗂", "Crear nueva carpeta", {
                let handle = handle.clone();
                move || {
                    let handle = handle.clone();
                    let available = current_tag_names(snap);
                    FolderCreateSheet::new(move |req: CreateFolderRequest| {
                        log::info!(
                            "folder confirm: name={:?} tags={:?}",
                            req.name,
                            req.tag_names
                        );
                        let handle = handle.clone();
                        let mut snap = snap;
                        spawn_forever(async move {
                            let mut lib = handle.lock().await;
                            let tag_ids = resolve_tag_names(&mut lib, &req.tag_names).await;
                            log::info!("folder create → parent={current:?} tag_ids={tag_ids:?}");
                            match lib
                                .create_folder(
                                    current,
                                    &req.name,
                                    Some(color_to_rgba(req.color)),
                                    tag_ids,
                                )
                                .await
                            {
                                Ok(id) => {
                                    log::info!("folder created id={id:?}");
                                    snap.set(Some(lib.index().clone()));
                                }
                                Err(err) => log::error!("create_folder: {err}"),
                            }
                        });
                    })
                    .available_tags(available)
                    .open();
                }
            }))
            .entry(
                FabMenuEntry::new("📄", "Importar PDF", {
                    let handle = handle.clone();
                    move || {
                        let handle = handle.clone();
                        spawn_forever(pick_and_open_pdf(handle, snap, current));
                    }
                })
                .with_divider_above(),
            )
            .open();
    });

    rect()
        .vertical()
        .spacing(12.0)
        .position(
            Position::new_absolute()
                .right(anchor_right)
                .bottom(anchor_bottom),
        )
        .child(pen)
        .child(plus)
}

fn fab_button<F: Fn(()) + 'static>(glyph: &'static str, bg: Color, handler: F) -> impl IntoElement {
    rect()
        .width(Size::px(52.0))
        .height(Size::px(52.0))
        .background(bg)
        .with_corner_radius(26.0)
        .center()
        .child(label().color(Color::WHITE).font_size(22.0).text(glyph))
        .on_press(move |_| handler(()))
}

// ---------------------------------------------------------------------------
// Library plumbing
// ---------------------------------------------------------------------------

async fn open_library() -> Result<LibHandle, String> {
    let runtime = istmo::Runtime::global().map_err(|e| format!("runtime not started: {e}"))?;
    let lib = Library::open(&runtime)
        .await
        .map_err(|e| format!("open library: {e}"))?;
    Ok(Arc::new(AsyncMutex::new(lib)))
}

/// Launch the platform file-picker, read the picked PDF into memory,
/// and open the canvas-create sheet pre-filled with its name.
async fn pick_and_open_pdf(
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    parent: FolderId,
) {
    use std::io::Read;
    use std::sync::Arc as StdArc;

    use istmo_file_picker::{FileFilter, FilePickerClient, PickConfig};

    let picker = match FilePickerClient::acquire() {
        Ok(p) => p,
        Err(err) => {
            log::error!("file picker unavailable: {err:?}");
            return;
        }
    };
    let config = PickConfig {
        dialog_title: Some("Elegí un PDF".into()),
        filter: FileFilter {
            mime_types: vec!["application/pdf".into()],
            extensions: vec!["pdf".into()],
            uti_types: vec!["com.adobe.pdf".into()],
        },
        start_directory_hint: None,
        allow_multiple: false,
    };
    let picked = match picker.pick_file_owned(config).await {
        Ok(Some(f)) => f,
        Ok(None) => return,
        Err(err) => {
            log::error!("picker call error: {err:?}");
            return;
        }
    };
    let display = picked.display_name.clone();
    let owned = StdArc::new(picked);
    let mut reader = match owned.open_reader(&picker).await {
        Ok(r) => r,
        Err(err) => {
            log::error!("open reader: {err:?}");
            return;
        }
    };
    let mut bytes = Vec::new();
    if let Err(err) = reader.read_to_end(&mut bytes) {
        log::error!("read pdf: {err}");
        return;
    }
    drop(reader);
    drop(owned);

    let default_name = strip_pdf_ext(&display);
    let bytes = StdArc::new(bytes);
    let available = current_tag_names(snap);
    CanvasCreateSheet::new(move |req: CreateCanvasRequest| {
        log::info!(
            "pdf confirm: name={:?} kind={:?} tags={:?}",
            req.name,
            req.kind,
            req.tag_names
        );
        let bytes = StdArc::clone(&bytes);
        let handle = handle.clone();
        let mut snap = snap;
        spawn_forever(async move {
            let mut lib = handle.lock().await;
            let tag_ids = resolve_tag_names(&mut lib, &req.tag_names).await;
            log::info!("pdf create → parent={parent:?} tag_ids={tag_ids:?}");
            match lib
                .create_item(
                    parent,
                    ItemKind::PdfCanvas,
                    &req.name,
                    Some(color_to_rgba(req.color)),
                    tag_ids,
                    req.background,
                )
                .await
            {
                Ok(id) => {
                    log::info!("pdf item created id={id:?}");
                    if let Err(err) = lib.attach_pdf(id, &bytes).await {
                        log::error!("attach_pdf: {err}");
                    }
                    snap.set(Some(lib.index().clone()));
                    let _ = RouterContext::get().push(Route::CanvasPdfView);
                }
                Err(err) => log::error!("create_item: {err}"),
            }
        });
    })
    .kind(ItemKind::PdfCanvas)
    .default_name(default_name)
    .available_tags(available)
    .open();
}

fn strip_pdf_ext(name: &str) -> String {
    name.rsplit_once('.')
        .filter(|(_, ext)| ext.eq_ignore_ascii_case("pdf"))
        .map_or_else(|| name.to_owned(), |(stem, _)| stem.to_owned())
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn rgba_to_color(rgba: Rgba) -> Color {
    Color::from_argb(rgba[3], rgba[0], rgba[1], rgba[2])
}

fn color_to_rgba(color: Color) -> Rgba {
    [color.r(), color.g(), color.b(), color.a()]
}

/// Snapshot the current library tag names for the picker's suggestion
/// list. Empty when the library hasn't loaded yet.
fn current_tag_names(snap: State<Option<LibraryIndex>>) -> Vec<String> {
    snap.read()
        .as_ref()
        .map(|idx| idx.tags.iter().map(|t| t.name.clone()).collect())
        .unwrap_or_default()
}

/// Resolve raw tag names emitted by the picker into `TagId`s, creating
/// any tag whose name isn't present yet. Names are compared
/// case-insensitively; empty entries are dropped silently. New tags
/// receive a color auto-derived from their name.
async fn resolve_tag_names(lib: &mut Library, names: &[String]) -> Vec<TagId> {
    let mut ids = Vec::with_capacity(names.len());
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(existing) = lib
            .tags()
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(trimmed))
        {
            ids.push(existing.id);
            continue;
        }
        let color = color_to_rgba(auto_color(trimmed));
        match lib.create_tag(trimmed, color).await {
            Ok(id) => ids.push(id),
            Err(err) => log::error!("create_tag({trimmed:?}): {err}"),
        }
    }
    ids
}


/// Format a millisecond epoch as `dd/mm/yy` (Spanish convention) using
/// the civil-from-days algorithm — no chrono dep just for one label.
fn format_date(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{:02}/{:02}/{:02}", d, m, (y.rem_euclid(100)) as u32)
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    // Howard Hinnant, "chrono-Compatible Low-Level Date Algorithms".
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

//! Bible install modal — catalogue list + flat search + per-row
//! install button.
//!
//! Opened by:
//! - the Bible plugin's empty-state render (no translations installed
//!   yet),
//! - the translation picker's "Agregar traducción…" row.
//!
//! Fetches the biblionlabs catalogue on mount via
//! [`notein_bible::BibleCanvas::load_catalogue`] (blocking HTTP in a
//! background thread) and polls `install_progress()` while mounted so
//! active downloads surface per-row.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use freya::icons::lucide::{check, download, search};
use freya::prelude::*;

use notein_bible::import::biblionlabs::{BibleEntryView, CatalogueView, ImportError};
use notein_bible::install_background::InstallStage;
use notein_plugin::{CanvasKind, ConfigValue};

use crate::components::{Modal, ModalController, theme};

const MODAL_WIDTH: f32 = 560.0;
const BODY_HEIGHT: f32 = 520.0;
const POLL_MS: u64 = 400;
const ROW_SIZE: f32 = 56.0;

/// Root-scoped epoch bumped every time an install promotion runs
/// through this modal. Consumers (notably `BibleFace`) subscribe via
/// `*install_epoch().read()` so their render invalidates when a
/// background install finishes — the empty-state branch flips to the
/// reader without the user having to re-enter the item.
#[must_use]
pub fn install_epoch() -> State<u64> {
    if let Some(existing) = try_consume_root_context::<State<u64>>() {
        return existing;
    }
    let slot = State::create_in_scope(0u64, ScopeId::ROOT);
    provide_context_for_scope_id(slot, ScopeId::ROOT);
    slot
}

pub fn open_install_modal() {
    ModalController::get().open(
        Modal::new(BibleInstallModal {})
            .center()
            .width(MODAL_WIDTH)
            .backdrop_alpha(170),
    );
}

#[derive(Clone, Debug, Default)]
enum CatalogueFetch {
    #[default]
    Loading,
    Ready(CatalogueView),
    Failed(String),
}

#[derive(Clone, PartialEq)]
struct BibleRowData {
    entry: BibleEntryView,
    installed: bool,
    active: Option<notein_bible::install_background::InstallProgress>,
}

#[derive(Clone, Copy, PartialEq)]
struct BibleInstallModal;

impl std::fmt::Debug for BibleInstallModal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BibleInstallModal").finish()
    }
}

impl Component for BibleInstallModal {
    fn render(&self) -> impl IntoElement {
        let canvas = crate::builtin::bible_canvas();

        let catalogue: State<Arc<Mutex<CatalogueFetch>>> =
            use_state(|| Arc::new(Mutex::new(CatalogueFetch::Loading)));
        // Bumped by a background poll so install-progress snapshots +
        // the `list_bibles()` re-read below pick up changes while the
        // modal stays open.
        let mut tick: State<u32> = use_state(|| 0);
        let query: State<String> = use_state(String::new);
        // Ids the user asked to install from this view. Once the
        // installer reports `Finished` for one of them AND the DB has
        // the row we promote it to the stored default and drop it
        // from the set.
        let pending: State<Arc<Mutex<HashSet<String>>>> =
            use_state(|| Arc::new(Mutex::new(HashSet::new())));

        use_hook({
            let canvas = canvas.clone();
            let catalogue = catalogue;
            move || {
                let fetch_slot = catalogue.read().clone();
                let canvas_for_fetch = canvas.clone();
                std::thread::Builder::new()
                    .name("bible-catalogue-fetch".into())
                    .spawn(move || {
                        let result = match canvas_for_fetch.as_ref() {
                            Some(c) => c
                                .load_catalogue()
                                .map(CatalogueFetch::Ready)
                                .unwrap_or_else(fetch_err),
                            None => CatalogueFetch::Failed(
                                "El plugin de Biblia no está registrado.".into(),
                            ),
                        };
                        if let Ok(mut slot) = fetch_slot.lock() {
                            *slot = result;
                        }
                    })
                    .ok();
                spawn(async move {
                    loop {
                        async_io::Timer::after(Duration::from_millis(POLL_MS)).await;
                        let next = tick.peek().wrapping_add(1);
                        tick.set(next);
                    }
                });
            }
        });
        let _ = *tick.read();

        let fetch_snapshot = catalogue
            .read()
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        // Live DB read on every render — SQLite `list_bibles` is a
        // bounded query (<100 rows in practice) so the poll tick's
        // ~400ms cadence is a non-issue, and we don't need a separate
        // state slot kept in sync with install completions.
        let installed_ids: Vec<String> = canvas
            .as_ref()
            .and_then(|c| c.db().list_bibles().ok())
            .map(|rows| rows.into_iter().map(|b| b.id).collect())
            .unwrap_or_default();
        let progress_snapshot = canvas
            .as_ref()
            .map(|c| c.install_progress().read().unwrap().clone());

        // Promote finished installs to the stored default so re-opens
        // land on whatever the user just picked.
        if let (Some(c), Some(p)) = (canvas.as_ref(), progress_snapshot.as_ref())
            && matches!(p.stage, InstallStage::Finished)
            && !p.bible_id.is_empty()
            && installed_ids.iter().any(|id| id == &p.bible_id)
            && let Ok(mut set) = pending.read().lock()
            && set.remove(&p.bible_id)
        {
            if let Err(e) = CanvasKind::set_config(
                c.as_ref(),
                notein_bible::plugin_config::KEY_DEFAULT_BIBLE,
                ConfigValue::String(p.bible_id.clone()),
            ) {
                log::warn!("bible install modal: set default failed: {e:?}");
            }
            let mut epoch = install_epoch();
            let next = epoch.peek().wrapping_add(1);
            epoch.set(next);
        }

        let close = Callback::<(), ()>::new(|_| {
            ModalController::get().close();
        });

        let on_install = {
            let canvas = canvas.clone();
            let pending = pending.read().clone();
            Callback::<String, ()>::new(move |id: String| {
                if let Some(c) = canvas.as_ref() {
                    if let Ok(mut set) = pending.lock() {
                        set.insert(id.clone());
                    }
                    c.request_install(id);
                }
            })
        };
        let on_pick_installed = {
            let canvas = canvas.clone();
            Callback::<String, ()>::new(move |id: String| {
                if let Some(c) = canvas.as_ref() {
                    if let Err(e) = CanvasKind::set_config(
                        c.as_ref(),
                        notein_bible::plugin_config::KEY_DEFAULT_BIBLE,
                        ConfigValue::String(id),
                    ) {
                        log::warn!("bible install modal: pick default failed: {e:?}");
                    }
                }
                let mut epoch = install_epoch();
                let next = epoch.peek().wrapping_add(1);
                epoch.set(next);
                ModalController::get().close();
            })
        };

        rect()
            .width(Size::px(MODAL_WIDTH))
            .background(theme::PANEL_BG)
            .with_corner_radius(16.0)
            .padding((16., 20.))
            .spacing(12.0)
            .child(header(close))
            .child(search_bar(query))
            .child(body(
                fetch_snapshot,
                installed_ids,
                progress_snapshot,
                &query.read(),
                on_install,
                on_pick_installed,
            ))
    }
}

fn fetch_err(err: ImportError) -> CatalogueFetch {
    CatalogueFetch::Failed(format!("No se pudo cargar el catálogo: {err}"))
}

fn header(close: Callback<(), ()>) -> impl IntoElement {
    rect()
        .width(Size::fill())
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .main_align(Alignment::SpaceBetween)
        .child(
            label()
                .font_size(18.0)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::TEXT_PRIMARY)
                .text("Instalar Biblia"),
        )
        .child(
            rect()
                .padding((6., 10.))
                .with_corner_radius(8.0)
                .on_press(move |_: Event<PressEventData>| close.call(()))
                .child(
                    label()
                        .color(theme::TEXT_SECONDARY)
                        .font_size(14.0)
                        .text("Cerrar"),
                ),
        )
}

fn search_bar(query: State<String>) -> impl IntoElement {
    notein_ui_kit::FormInput::new(query)
        .flat()
        .left_icon(search())
        .placeholder("Buscar por idioma, abreviación o id")
        .width(Size::fill())
}

fn body(
    fetch: CatalogueFetch,
    installed_ids: Vec<String>,
    progress: Option<notein_bible::install_background::InstallProgress>,
    query: &str,
    on_install: Callback<String, ()>,
    on_pick_installed: Callback<String, ()>,
) -> Element {
    let container = rect()
        .width(Size::fill())
        .height(Size::px(BODY_HEIGHT))
        .background(theme::CARD_BG)
        .with_corner_radius(12.0)
        .padding((8., 6.));
    match fetch {
        CatalogueFetch::Loading => container
            .center()
            .child(
                label()
                    .color(theme::TEXT_SECONDARY)
                    .font_size(14.0)
                    .text("Cargando catálogo…"),
            )
            .into_element(),
        CatalogueFetch::Failed(message) => container
            .center()
            .child(
                label()
                    .color(theme::ERROR)
                    .font_size(14.0)
                    .text(message),
            )
            .into_element(),
        CatalogueFetch::Ready(view) => {
            let filtered = filter_bibles(&view.bibles, query);
            if filtered.is_empty() {
                return container
                    .center()
                    .child(
                        label()
                            .color(theme::TEXT_SECONDARY)
                            .font_size(14.0)
                            .text("Nada coincide con la búsqueda."),
                    )
                    .into_element();
            }
            let rows: Vec<BibleRowData> = filtered
                .into_iter()
                .map(|entry| {
                    let installed = installed_ids.iter().any(|id| *id == entry.id);
                    let active = progress
                        .as_ref()
                        .filter(|p| p.bible_id == entry.id)
                        .cloned();
                    BibleRowData {
                        entry: entry.clone(),
                        installed,
                        active,
                    }
                })
                .collect();
            let length = rows.len();
            let scroll = VirtualScrollView::new_with_data(
                rows,
                move |item, data: &Vec<BibleRowData>| {
                    let row = data[item.index].clone();
                    bible_row(
                        row.entry,
                        row.installed,
                        row.active,
                        on_install.clone(),
                        on_pick_installed.clone(),
                    )
                },
            )
            .length(length)
            .width(Size::fill())
            .item_size(ROW_SIZE);
            container.child(scroll).into_element()
        }
    }
}

fn filter_bibles<'a>(bibles: &'a [BibleEntryView], query: &str) -> Vec<&'a BibleEntryView> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return bibles.iter().collect();
    }
    bibles
        .iter()
        .filter(|b| {
            matches(&b.id, &needle)
                || matches_opt(&b.language, &needle)
                || matches_opt(&b.name_local, &needle)
                || matches_opt(&b.name_english, &needle)
        })
        .collect()
}

fn matches(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(needle)
}

fn matches_opt(hay: &Option<String>, needle: &str) -> bool {
    hay.as_deref().map(|s| matches(s, needle)).unwrap_or(false)
}

fn bible_row(
    entry: BibleEntryView,
    installed: bool,
    active: Option<notein_bible::install_background::InstallProgress>,
    on_install: Callback<String, ()>,
    on_pick_installed: Callback<String, ()>,
) -> Element {
    let name = entry
        .name_local
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| entry.name_english.clone())
        .unwrap_or_else(|| entry.id.clone());
    let subtitle = {
        let mut parts = Vec::new();
        if let Some(lang) = entry.language.as_deref().filter(|s| !s.is_empty()) {
            parts.push(lang.to_string());
        }
        parts.push(entry.id.clone());
        parts.join(" · ")
    };
    let id = entry.id.clone();
    let install_press = {
        let id = id.clone();
        move |_: Event<PressEventData>| on_install.call(id.clone())
    };

    let text_col = rect()
        .width(Size::flex(1.0))
        .spacing(2.0)
        .child(
            label()
                .font_size(14.0)
                .color(theme::TEXT_PRIMARY)
                .text(name),
        )
        .child(
            label()
                .font_size(11.0)
                .color(theme::TEXT_SECONDARY)
                .text(subtitle),
        );

    let trailing = row_trailing(installed, active, install_press);

    let mut row = rect()
        .width(Size::fill())
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .padding((6., 10.))
        .spacing(10.0)
        .with_corner_radius(10.0);
    if installed {
        // Picking an installed row commits it as the stored default +
        // closes the modal, so re-opens of the reader land on the
        // user's new choice without a reader-side listener.
        let pick_id = id.clone();
        row = row.on_press(move |_: Event<PressEventData>| on_pick_installed.call(pick_id.clone()));
    }
    row.child(text_col).child(trailing).into_element()
}

fn row_trailing(
    installed: bool,
    active: Option<notein_bible::install_background::InstallProgress>,
    on_install: impl FnMut(Event<PressEventData>) + 'static,
) -> Element {
    if installed {
        return rect()
            .direction(Direction::Horizontal)
            .cross_align(Alignment::Center)
            .spacing(6.0)
            .child(
                SvgViewer::new(check())
                    .color(theme::SUCCESS)
                    .width(Size::px(14.0))
                    .height(Size::px(14.0)),
            )
            .child(
                label()
                    .color(theme::SUCCESS)
                    .font_size(12.0)
                    .text("Instalada"),
            )
            .into_element();
    }
    if let Some(p) = active {
        let text = match p.stage {
            InstallStage::Starting => "Preparando…".to_string(),
            InstallStage::Downloading => {
                if p.total_books > 0 {
                    format!("Descargando {} / {}", p.done_books, p.total_books)
                } else {
                    "Descargando…".to_string()
                }
            }
            InstallStage::Finished => "Finalizando…".to_string(),
            InstallStage::Failed => p
                .error
                .clone()
                .unwrap_or_else(|| "Falló".to_string()),
            InstallStage::Idle => "Pendiente".to_string(),
        };
        let color = if matches!(p.stage, InstallStage::Failed) {
            theme::ERROR
        } else {
            theme::TEXT_SECONDARY
        };
        return label()
            .color(color)
            .font_size(12.0)
            .text(text)
            .into_element();
    }
    rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(6.0)
        .padding((6., 10.))
        .with_corner_radius(8.0)
        .background(theme::PRIMARY)
        .on_press(on_install)
        .child(
            SvgViewer::new(download())
                .color(theme::TEXT_INVERSE)
                .width(Size::px(14.0))
                .height(Size::px(14.0)),
        )
        .child(
            label()
                .color(theme::TEXT_INVERSE)
                .font_size(12.0)
                .text("Instalar"),
        )
        .into_element()
}

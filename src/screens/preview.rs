//! Headless renders of the screens over a demo library, written as PNGs
//! to `$NOTEIN_PREVIEW_DIR` (the test temp dir otherwise).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use freya::prelude::*;
use freya_testing::prelude::*;

use crate::library::{
    DocKind, Folder, FolderId, Item, ItemId, LibraryData, LibraryIndex, Link, LinkId, LinkIndex,
    LinkKind, PaneLayout, PaneTab, ROOT_FOLDER, SplitOrientation, TabTarget, Workspace,
    WorkspaceId, WorkspaceIndex,
};
use crate::plugins::builtin::{canvas, pdf};

const DAY: u64 = 24 * 60 * 60 * 1000;

fn item(name: &str, kind: &str, folder: FolderId, age_days: u64) -> Item {
    let now = crate::format::now();
    Item {
        id: ItemId::new_v4(),
        folder,
        kind: DocKind::plugin(kind),
        name: name.to_owned(),
        color: None,
        tags: Vec::new(),
        background: crate::library::BackgroundStyle::default(),
        created_at: now - (age_days + 3) * DAY,
        updated_at: now - age_days * DAY,
        thumbnail: None,
        trashed: None,
    }
}

fn folder(name: &str, color: [u8; 4]) -> Folder {
    let now = crate::format::now();
    Folder {
        id: FolderId::new_v4(),
        parent: ROOT_FOLDER,
        name: name.to_owned(),
        color: Some(color),
        icon: None,
        tags: Vec::new(),
        created_at: now - 20 * DAY,
        updated_at: now - 2 * DAY,
        trashed: None,
    }
}

fn tab(id: ItemId) -> PaneTab {
    PaneTab {
        target: TabTarget::Item(id),
        plugin_id: None,
    }
}

/// A library like the one in the design: a study mesa, pinned
/// documents, two folders and a mix of kinds.
pub(crate) fn demo() -> LibraryData {
    let now = crate::format::now();
    let estudio = folder("Estudio", [0x7B, 0x8C, 0xFF, 0xFF]);
    let clases = folder("Clases", [0x5F, 0xB3, 0x8A, 0xFF]);
    let romanos = item("Comentario de Romanos", pdf::KIND, ROOT_FOLDER, 0);
    let cuaderno = item("Cuaderno de Romanos", canvas::KIND, ROOT_FOLDER, 0);
    let biblia = item("Biblia", notein_bible::kind::KIND, ROOT_FOLDER, 1);
    let plan = item("Plan de lectura", notein_notes::kind::KIND, ROOT_FOLDER, 2);
    let sermon = item(
        "Sermón del domingo",
        notein_notes::kind::KIND,
        ROOT_FOLDER,
        4,
    );
    let mapa = item("Mapa de Pablo", canvas::KIND, ROOT_FOLDER, 9);
    let manual = item(
        "Manual de griego koiné con un nombre muy largo",
        pdf::KIND,
        ROOT_FOLDER,
        30,
    );
    let inside = item("Efesios 6", canvas::KIND, estudio.id, 3);
    let mut layout = PaneLayout::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.55,
        first: Box::new(PaneLayout::leaf(crate::library::PaneId::new_v4())),
        second: Box::new(PaneLayout::leaf(crate::library::PaneId::new_v4())),
    };
    if let PaneLayout::Split { first, second, .. } = &mut layout {
        if let PaneLayout::Leaf(pane) = first.as_mut() {
            pane.tabs = vec![tab(romanos.id), tab(biblia.id)];
        }
        if let PaneLayout::Leaf(pane) = second.as_mut() {
            pane.tabs = vec![tab(cuaderno.id)];
        }
    }
    let study = Workspace {
        layout,
        ..Workspace::fresh(
            WorkspaceId::new_v4(),
            String::new(),
            None,
            now - 2 * 60 * 60 * 1000,
        )
    };
    let mut clase = Workspace::fresh(
        WorkspaceId::new_v4(),
        "Clase de los martes".to_owned(),
        Some([0xE0, 0xA8, 0x45, 0xFF]),
        now - DAY,
    );
    clase.named = true;
    clase.pinned = true;
    if let PaneLayout::Leaf(pane) = &mut clase.layout {
        pane.tabs = vec![tab(sermon.id), tab(plan.id)];
    }
    let link = Link {
        id: LinkId::new_v4(),
        from: romanos.id,
        to: cuaderno.id,
        kind: LinkKind::Companion,
        created_at: now,
        updated_at: now,
        deleted: false,
    };
    let pins = vec![biblia.id, plan.id];
    let index = LibraryIndex {
        version: crate::library::INDEX_VERSION,
        folders: vec![estudio, clases],
        items: vec![
            romanos, cuaderno, biblia, plan, sermon, mapa, manual, inside,
        ],
        tags: Vec::new(),
        pins,
    };
    let workspaces = WorkspaceIndex {
        version: crate::library::WORKSPACE_INDEX_VERSION,
        active: Some(study.id),
        workspaces: vec![study, clase],
    };
    let links = LinkIndex {
        version: 1,
        links: vec![link],
    };
    LibraryData::from_parts(index, workspaces, links)
}

fn out(name: &str) -> PathBuf {
    std::env::var_os("NOTEIN_PREVIEW_DIR")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join(format!("{name}.png"))
}

fn shell(body: fn() -> Element, size: (f32, f32)) -> impl Fn() -> Element {
    move || {
        let area = use_state(move || Area::new((0.0, 0.0).into(), size.into()));
        let _settings = crate::settings::use_settings_root();
        use_hook(|| {
            if let Some(mut theme) = notein_ui_kit::theme::try_theme() {
                theme.update(|a| a.reduce_motion = true);
            }
        });
        crate::state::use_fixed_library(Arc::new(demo()));
        crate::screens::home::use_browse_root();
        rect()
            .expanded()
            .background(notein_ui_kit::palette().bg)
            .child(body())
            .child(crate::components::ModalPortal::new(area))
            .into_element()
    }
}

fn render(name: &str, size: (f32, f32), body: fn() -> Element) {
    let app = shell(body, size);
    let (mut runner, ()) = TestingRunner::new(app, size.into(), |_| (), 1.0);
    runner.poll_n(Duration::from_millis(40), 12);
    runner.sync_and_update();
    runner.poll_n(Duration::from_millis(40), 4);
    runner.sync_and_update();
    runner.render_to_file(out(name));
}

fn home() -> Element {
    crate::screens::home::Home.into_element()
}

fn trash() -> Element {
    crate::screens::trash::Trash.into_element()
}

#[test]
fn home_desktop() {
    render("home-desktop", (1440.0, 900.0), home);
}

#[test]
fn home_tablet() {
    render("home-tablet", (1024.0, 768.0), home);
}

#[test]
fn home_phone() {
    render("home-phone", (390.0, 844.0), home);
}

#[test]
fn trash_tablet() {
    render("trash-tablet", (1024.0, 768.0), trash);
}

#[test]
fn home_desktop_full_page() {
    render("home-desktop-full", (1440.0, 1500.0), home);
}

#[test]
fn home_phone_full_page() {
    render("home-phone-full", (390.0, 1700.0), home);
}

fn settings_at(section: crate::screens::settings::Section) -> Element {
    use_hook(move || {
        spawn(async move { crate::screens::settings::open_at(section) });
    });
    home()
}

fn settings_appearance() -> Element {
    settings_at(crate::screens::settings::Section::Appearance)
}

fn settings_pen() -> Element {
    settings_at(crate::screens::settings::Section::Pen)
}

fn settings_library() -> Element {
    settings_at(crate::screens::settings::Section::Library)
}

fn settings_devices() -> Element {
    settings_at(crate::screens::settings::Section::Devices)
}

fn settings_plugins() -> Element {
    settings_at(crate::screens::settings::Section::Plugins)
}

fn search_empty() -> Element {
    use_hook(|| {
        spawn(async { crate::screens::search::open(String::new()) });
    });
    home()
}

#[test]
fn settings_renders() {
    render("settings-appearance", (1280.0, 860.0), settings_appearance);
    render("settings-pen", (1280.0, 860.0), settings_pen);
    render("settings-library", (1280.0, 860.0), settings_library);
    render("settings-devices", (1280.0, 860.0), settings_devices);
    render("settings-plugins", (1280.0, 860.0), settings_plugins);
    render("settings-phone", (390.0, 844.0), settings_appearance);
}

#[test]
fn search_renders() {
    render("search-desktop", (1280.0, 860.0), search_empty);
}

fn mesa_view(id: Option<crate::library::WorkspaceId>) -> Element {
    id.map_or_else(
        || rect().into_element(),
        |id| crate::screens::mesa::Mesa { id }.into_element(),
    )
}

fn mesa() -> Element {
    let data = crate::state::LibraryView::get().data();
    mesa_view(data.and_then(|d| d.active_workspace()))
}

#[test]
fn mesa_renders() {
    render("mesa-desktop", (1440.0, 900.0), mesa);
    render("mesa-phone", (390.0, 844.0), mesa);
}

fn class_mesa() -> Element {
    let data = crate::state::LibraryView::get().data();
    mesa_view(data.and_then(|d| d.workspaces().iter().find(|w| w.named).map(|w| w.id)))
}

#[test]
fn note_mesa_renders() {
    render("mesa-note-tablet", (1024.0, 768.0), class_mesa);
}

fn map() -> Element {
    use_hook(crate::screens::map::open);
    rect().into_element()
}

#[test]
fn map_renders() {
    render("map-desktop", (1280.0, 800.0), map);
}

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

const fn tab(id: ItemId) -> PaneTab {
    PaneTab {
        target: TabTarget::Item(id),
        plugin_id: None,
    }
}

/// Named mesas with nothing open, enough to fold the list on Home.
fn named_mesas(now: u64) -> impl Iterator<Item = Workspace> {
    [
        ("Lectura de Hechos", [0x5F, 0xB3, 0x8A, 0xFF], 2),
        ("Sermón del domingo", [0x7B, 0x8C, 0xFF, 0xFF], 4),
        ("Griego", [0xD0, 0x6B, 0x6B, 0xFF], 9),
        ("Investigación", [0xB0, 0x8C, 0xE0, 0xFF], 21),
    ]
    .into_iter()
    .map(move |(name, color, days)| {
        let mut w = Workspace::fresh(
            WorkspaceId::new_v4(),
            name.to_owned(),
            Some(color),
            now - days * DAY,
        );
        w.named = true;
        w
    })
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
    // Made on the tablet.
    clase.device = Some(notein_sync::DeviceId([9; 32]));
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
        workspaces: [study, clase].into_iter().chain(named_mesas(now)).collect(),
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

fn shell(
    body: fn() -> Element,
    size: (f32, f32),
    gutters: notein_ui_kit::Gutters,
) -> impl Fn() -> Element {
    move || {
        let area = use_state(move || Area::new((0.0, 0.0).into(), size.into()));
        let covered = use_memo(move || gutters);
        use_provide_context(move || notein_ui_kit::ModalChrome {
            gutters: Some(covered),
            ..Default::default()
        });
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

/// Point the app's data folders at a scratch directory, so a preview
/// that changes a setting never writes into the real data.
fn sandbox() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let root = std::env::temp_dir().join(format!("notein-preview-{}", std::process::id()));
        let dir = |name: &str| root.join(name);
        istmo::path::install(istmo::path::PlatformPaths {
            data_dir: dir("data"),
            data_local_dir: dir("data-local"),
            config_dir: dir("config"),
            config_local_dir: dir("config-local"),
            cache_dir: dir("cache"),
            state_dir: dir("state"),
            preference_dir: dir("preferences"),
            documents_dir: dir("documents"),
            runtime_dir: dir("runtime"),
            temp_dir: dir("tmp"),
        });
    });
}

fn render(name: &str, size: (f32, f32), body: fn() -> Element) {
    render_covered(name, size, notein_ui_kit::Gutters::default(), body);
}

/// [`render`] with the window edges `gutters` covered, as by the system
/// bars and the soft keyboard.
fn render_covered(
    name: &str,
    size: (f32, f32),
    gutters: notein_ui_kit::Gutters,
    body: fn() -> Element,
) {
    sandbox();
    let app = shell(body, size, gutters);
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

fn settings_notes() -> Element {
    settings_at(crate::screens::settings::Section::Notes)
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
    render("settings-notes", (1280.0, 860.0), settings_notes);
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
    render("mesa-note-portrait", (820.0, 1180.0), class_mesa);
}

fn map() -> Element {
    use_hook(crate::screens::map::open);
    rect().into_element()
}

#[test]
fn map_renders() {
    render("map-desktop", (1280.0, 800.0), map);
}

fn library_failed() -> Element {
    crate::screens::home::Failed {
        why: "library codec error: UnexpectedVariant".to_owned(),
    }
    .into_element()
}

#[test]
fn library_failed_desktop() {
    render("library-failed-desktop", (1440.0, 900.0), library_failed);
}

#[test]
fn library_failed_phone() {
    render("library-failed-phone", (390.0, 844.0), library_failed);
}

fn long_sheet() -> Element {
    use_hook(|| {
        notein_ui_kit::Sheet::titled("Muchas opciones")
            .children(
                (0..30).map(|i| notein_ui_kit::ListRow::new(format!("Opción {i}")).into_element()),
            )
            .footer(notein_ui_kit::Button::new("Listo").primary())
            .open();
    });
    rect().into_element()
}

fn edge_popover() -> Element {
    use_hook(|| {
        let entries = (0..8)
            .map(|i| {
                notein_ui_kit::ActionEntry::Item(notein_ui_kit::Action::new(
                    format!("Acción {i}"),
                    |()| {},
                ))
            })
            .collect();
        crate::actions::show(entries, CursorPoint::new(790.0, 560.0));
    });
    rect().into_element()
}

const KEYBOARD: notein_ui_kit::Gutters = notein_ui_kit::Gutters {
    top: 24.0,
    right: 0.0,
    bottom: 380.0,
    left: 0.0,
};

#[test]
fn overlays_stay_in_the_visible_window() {
    render_covered("sheet-keyboard", (820.0, 1000.0), KEYBOARD, long_sheet);
    render_covered("sheet-keyboard-phone", (390.0, 844.0), KEYBOARD, long_sheet);
    render("popover-edge", (820.0, 600.0), edge_popover);
}

fn selection_menu() -> Element {
    use_hook(|| {
        crate::selection::SelectionMenu {
            at: (420.0, 320.0),
            active: Some(1),
            on_color: std::rc::Rc::new(|_| {}),
            on_note: std::rc::Rc::new(|| {}),
            reference: Some((
                notein_plugin::bookmark::SourceRef::Plugin {
                    plugin_id: "bible".to_owned(),
                    item: [1; 16],
                    anchor: Vec::new(),
                    selection: None,
                },
                "Juan 3:16".to_owned(),
            )),
            document: Some(crate::library::ItemId([1; 16])),
            extra: Vec::new(),
            on_dismiss: std::rc::Rc::new(|| {}),
        }
        .show();
    });
    rect().into_element()
}

#[test]
fn selection_menu_hugs_its_actions() {
    render("selection-desktop", (1440.0, 900.0), selection_menu);
    render("selection-tablet", (1024.0, 768.0), selection_menu);
}

fn annotation_popover() -> Element {
    use_hook(|| {
        crate::annotation::AnnotationPopover::open(
            notein_plugin::services::AnnotationRequest {
                doc: notein_plugin::DocId::default(),
                kind: "bible".to_owned(),
                title: "Verso 16".to_owned(),
                quote: Some(
                    "Porque de tal manera amó Dios al mundo, que ha dado a su Hijo unigénito, \
                     para que todo aquel que en él cree, no se pierda, mas tenga vida eterna."
                        .to_owned(),
                ),
                color: Some([255, 214, 102]),
                body: String::new(),
                target: Some((420.0, 420.0, 1.0, 1.0)),
                existing: false,
            },
            None,
            std::sync::Arc::new(|_| {}),
        );
    });
    rect().into_element()
}

#[test]
fn annotation_popover_keeps_its_width() {
    render("annotation-desktop", (1440.0, 900.0), annotation_popover);
    render("annotation-tablet", (1024.0, 768.0), annotation_popover);
}

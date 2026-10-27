//! Headless renders of the screens over a demo library, written as PNGs
//! to `$NOTEIN_PREVIEW_DIR` (the test temp dir otherwise).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use freya::prelude::*;
use freya_testing::prelude::*;
use notein_ui_kit::TapExt;

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
    let efesios = item("Notas de Efesios", notein_notes::kind::KIND, estudio.id, 5);
    let griego = item("Léxico de Efesios", pdf::KIND, estudio.id, 6);
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
            romanos, cuaderno, biblia, plan, sermon, mapa, manual, inside, efesios, griego,
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
            // The fonts the app bundles at launch.
            for face in notein_bible::fonts::FACES {
                Platform::get().load_font(notein_bible::fonts::FAMILY, Bytes::from_static(face));
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

/// The annotation popover over a mesa-like row with room for the sidebar.
fn annotation_beside() -> Element {
    annotation_popover();
    rect()
        .expanded()
        .horizontal()
        .content(Content::Flex)
        .child(rect().width(Size::flex(1.0)).height(Size::fill()))
        .child(crate::annotation::AnnotationSidebar)
        .into_element()
}

#[test]
fn annotation_expands_and_picks_colors() {
    render_after_press(
        "annotation-sidebar",
        (1440.0, 900.0),
        (559.0, 53.0),
        annotation_beside,
    );
    render_after_press(
        "annotation-picker",
        (1440.0, 900.0),
        (430.0, 190.0),
        annotation_beside,
    );
}

const JOHN_3: &str = r#"<osis><osisText>
<div type="book" osisID="John"><chapter osisID="John.3">
<title type="section">Jesús y Nicodemo</title>
<verse osisID="John.3.14">Y como Moisés levantó la serpiente en el desierto, así es necesario que el Hijo del Hombre sea levantado,</verse>
<verse osisID="John.3.15">para que todo aquel que en él cree, no se pierda, mas tenga vida eterna.</verse>
<verse osisID="John.3.16">Porque de tal manera amó Dios al mundo, que ha dado a su Hijo unigénito, para que todo aquel que en él cree, no se pierda, mas tenga vida eterna.</verse>
<verse osisID="John.3.17">Porque no envió Dios a su Hijo al mundo para condenar al mundo, sino para que el mundo sea salvo por él.</verse>
</chapter></div>
<div type="book" osisID="Rom"><chapter osisID="Rom.5">
<verse osisID="Rom.5.8">Mas Dios muestra su amor para con nosotros, en que siendo aún pecadores, Cristo murió por nosotros.</verse>
</chapter></div>
<div type="book" osisID="1John"><chapter osisID="1John.4">
<verse osisID="1John.4.9">En esto se mostró el amor de Dios para con nosotros, en que Dios envió a su Hijo unigénito al mundo, para que vivamos por él.</verse>
</chapter></div>
</osisText></osis>"#;

/// A Bible item open at John 3, with cross-references on verse 16.
fn bible_john_3() -> Element {
    use_hook(|| {
        let Some(kind) = crate::plugins::builtin::bible_kind() else {
            return;
        };
        let db = kind.db();
        if db.list_bibles().unwrap_or_default().is_empty() {
            notein_bible::import::osis::import_osis_xml(db, "spa_rv1960", JOHN_3.as_bytes())
                .expect("seed John 3");
            let books = db.list_books("spa_rv1960").expect("books");
            let id = |osis: &str| {
                books
                    .iter()
                    .find(|b| b.book_id.eq_ignore_ascii_case(osis) || b.name == osis)
                    .map_or_else(|| osis.to_owned(), |b| b.book_id.clone())
            };
            let conn = rusqlite::Connection::open(notein_bible::db::Db::default_path())
                .expect("open bible db");
            for (book, chapter, verse) in [("Rom", 5, 8), ("1John", 4, 9), ("John", 3, 14)] {
                conn.execute(
                    "INSERT INTO cross_refs
                     (bible_id, src_book, src_chap, src_verse, tgt_book, tgt_chap, tgt_verse)
                     VALUES ('spa_rv1960', ?1, 3, 16, ?2, ?3, ?4)",
                    rusqlite::params![id("John"), id(book), chapter, verse],
                )
                .expect("seed cross-reference");
            }
        }
        let state = notein_bible::state::BibleViewerState::new("spa_rv1960", "John", 3);
        let bytes = notein_bible::state::BibleSnapshot::from_state(&state).encode();
        crate::library::bodies::write_plugin_snapshot(BIBLE_ITEM, crate::bible_face::KIND, &bytes)
            .expect("snapshot");
    });
    crate::bible_face::BibleFace {
        item: BIBLE_ITEM,
        slot: None,
    }
    .into_element()
}

const BIBLE_ITEM: crate::library::ItemId = crate::library::ItemId([7; 16]);

/// [`render`] after pressing `at`, once the screen settled.
fn render_after_press(name: &str, size: (f32, f32), at: (f64, f64), body: fn() -> Element) {
    sandbox();
    let app = shell(body, size, notein_ui_kit::Gutters::default());
    let (mut runner, ()) = TestingRunner::new(app, size.into(), |_| (), 1.0);
    runner.poll_n(Duration::from_millis(40), 12);
    runner.sync_and_update();
    runner.click_cursor(at);
    runner.poll_n(Duration::from_millis(40), 12);
    runner.sync_and_update();
    runner.render_to_file(out(name));
}

#[test]
fn bible_cross_references() {
    render("bible-refs-closed", (1440.0, 900.0), bible_john_3);
    render_after_press(
        "bible-refs-panel",
        (1440.0, 900.0),
        (466.0, 312.0),
        bible_john_3,
    );
    render("bible-refs-phone-closed", (390.0, 844.0), bible_john_3);
    render_after_press(
        "bible-refs-sheet",
        (390.0, 844.0),
        (136.0, 565.0),
        bible_john_3,
    );
}

thread_local! {
    static SHEET_PRESSES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// A sheet with a long grid, like the Bible's book picker.
fn book_grid_sheet() -> Element {
    use_hook(|| {
        let grid = rect()
            .width(Size::fill())
            .horizontal()
            .spacing(8.0)
            .content(Content::wrap_spacing(8.0))
            .children((1..=66).map(|i| {
                rect()
                    .width(Size::px(72.0))
                    .height(Size::px(72.0))
                    .center()
                    .background(notein_ui_kit::palette().s3)
                    .on_tap(|_| SHEET_PRESSES.set(SHEET_PRESSES.get() + 1))
                    .child(label().text(i.to_string()))
                    .into_element()
            }));
        notein_ui_kit::Sheet::titled("Libros").child(grid).open();
    });
    rect().into_element()
}

#[test]
fn sheet_scrolls_by_touch_without_pressing() {
    sandbox();
    let size = (390.0, 844.0);
    let app = shell(book_grid_sheet, size, notein_ui_kit::Gutters::default());
    let (mut runner, ()) = TestingRunner::new(app, size.into(), |_| (), 1.0);
    runner.poll_n(Duration::from_millis(40), 12);
    runner.sync_and_update();
    runner.render_to_file(out("sheet-touch-before"));
    runner.press_touch((150.0, 600.0));
    runner.sync_and_update();
    for step in 1..=10 {
        runner.move_touch((150.0, f64::from(step).mul_add(-30.0, 600.0)));
        runner.poll_n(Duration::from_millis(16), 1);
        runner.sync_and_update();
    }
    runner.render_to_file(out("sheet-touch-during"));
    runner.release_touch((150.0, 300.0));
    runner.poll_n(Duration::from_millis(40), 20);
    runner.sync_and_update();
    runner.render_to_file(out("sheet-touch-after"));
    assert_eq!(
        SHEET_PRESSES.get(),
        0,
        "lifting after a scroll pressed a cell"
    );
    runner.press_touch((60.0, 400.0));
    runner.sync_and_update();
    runner.release_touch((62.0, 401.0));
    runner.sync_and_update();
    assert_eq!(SHEET_PRESSES.get(), 1, "a tap did not press its cell");
}

/// The book grid sheet on a phone, settled.
fn phone_sheet() -> TestingRunner {
    sandbox();
    let size = (390.0, 844.0);
    let app = shell(book_grid_sheet, size, notein_ui_kit::Gutters::default());
    let (mut runner, ()) = TestingRunner::new(app, size.into(), |_| (), 1.0);
    runner.poll_n(Duration::from_millis(40), 12);
    runner.sync_and_update();
    runner
}

/// Drag a finger from `from` straight down by `by`, in `steps` moves a
/// frame apart, and lift it after `hold`.
fn finger_drag(runner: &mut TestingRunner, from: (f64, f64), by: f64, steps: u32, hold: Duration) {
    runner.press_touch(from);
    runner.sync_and_update();
    let mut y = from.1;
    for step in 1..=steps {
        y = (by / f64::from(steps)).mul_add(f64::from(step), from.1);
        runner.move_touch((from.0, y));
        runner.poll_n(Duration::from_millis(16), 1);
    }
    std::thread::sleep(hold);
    runner.release_touch((from.0, y));
    runner.poll_n(Duration::from_millis(30), 20);
    runner.sync_and_update();
}

fn open_modals(runner: &TestingRunner) -> usize {
    runner.run_in(|| notein_ui_kit::ModalController::get().depth())
}

#[test]
fn sheet_closes_when_dragged_down_by_its_handle() {
    let mut runner = phone_sheet();
    finger_drag(&mut runner, (195.0, 40.0), 520.0, 10, Duration::ZERO);
    assert_eq!(open_modals(&runner), 0);
}

#[test]
fn sheet_glides_back_after_a_short_slow_drag() {
    let mut runner = phone_sheet();
    runner.press_touch((195.0, 40.0));
    runner.sync_and_update();
    runner.move_touch((195.0, 120.0));
    runner.poll_n(Duration::from_millis(16), 2);
    runner.render_to_file(out("sheet-drag-held"));
    std::thread::sleep(Duration::from_millis(150));
    runner.release_touch((195.0, 120.0));
    runner.poll_n(Duration::from_millis(30), 20);
    runner.sync_and_update();
    runner.render_to_file(out("sheet-drag-back"));
    assert_eq!(open_modals(&runner), 1);
}

#[test]
fn sheet_follows_a_finger_on_its_content_only_from_the_top() {
    // Scrolled down, a finger going down scrolls the content back.
    let mut runner = phone_sheet();
    finger_drag(&mut runner, (150.0, 700.0), -300.0, 10, Duration::ZERO);
    finger_drag(
        &mut runner,
        (150.0, 300.0),
        120.0,
        6,
        Duration::from_millis(150),
    );
    assert_eq!(
        open_modals(&runner),
        1,
        "scrolling back up closed the sheet"
    );
    // At the top, the same finger takes the sheet down.
    let mut runner = phone_sheet();
    finger_drag(&mut runner, (150.0, 300.0), 480.0, 10, Duration::ZERO);
    assert_eq!(open_modals(&runner), 0);
}

/// Home on a phone tall enough to show the shelf grid, settled. The
/// first grid card, the "Estudio" folder, sits at [`GRID_FOLDER`].
fn tall_home() -> TestingRunner {
    sandbox();
    let size = (390.0, 1700.0);
    let app = shell(home, size, notein_ui_kit::Gutters::default());
    let (mut runner, ()) = TestingRunner::new(app, size.into(), |_| (), 1.0);
    runner.poll_n(Duration::from_millis(40), 12);
    runner.sync_and_update();
    runner
}

const GRID_FOLDER: (f64, f64) = (90.0, 950.0);

fn browsing_root(runner: &TestingRunner) -> bool {
    runner.run_in(|| crate::screens::home::Browse::get().folder() == crate::library::ROOT_FOLDER)
}

#[test]
fn grid_card_opens_on_click() {
    let mut runner = tall_home();
    assert!(browsing_root(&runner));
    runner.click_cursor(GRID_FOLDER);
    runner.poll_n(Duration::from_millis(40), 4);
    runner.sync_and_update();
    assert!(
        !browsing_root(&runner),
        "a click on a grid card did nothing"
    );
}

#[test]
fn grid_card_opens_on_tap() {
    let mut runner = tall_home();
    runner.press_touch(GRID_FOLDER);
    runner.sync_and_update();
    runner.release_touch(GRID_FOLDER);
    runner.poll_n(Duration::from_millis(40), 4);
    runner.sync_and_update();
    assert!(!browsing_root(&runner), "a tap on a grid card did nothing");
}

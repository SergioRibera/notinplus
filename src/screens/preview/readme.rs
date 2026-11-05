//! The screenshots in the README, over a library with real content: a
//! study note, a sketch, the Reina-Valera 1960 with its original-language
//! words and cross-references, and two other people working along.
//!
//! Ignored by default: they need an installed Bible with the original
//! packs. Run each test in a process of its own, so nothing else changes
//! the language or the sandbox, and the canvas shot stays the first
//! render of its process: ghost ink from other people only reaches the
//! image of the first runner a process starts.
//!
//! ```sh
//! export NOTEIN_PREVIEW_DIR=screenshots \
//!   NOTEIN_README_BIBLE=~/.local/share/rs.sergioribera.notinplus/notinplus/bible
//! cargo test --lib readme_desktop -- --ignored
//! cargo test --lib readme_phone -- --ignored
//! ```
//!
//! `NOTEIN_README_BIBLE` is an app Bible folder with the Reina-Valera
//! 1960, the original packs and cross-references installed; it is
//! copied, never written.

use super::*;

use crate::doc::Doc;
use crate::library::bodies;

const NOTE: ItemId = ItemId([0x11; 16]);
const SKETCH: ItemId = ItemId([0x12; 16]);
const BIBLE: ItemId = ItemId([0x13; 16]);
const SERMON: ItemId = ItemId([0x14; 16]);
const GREEK: ItemId = ItemId([0x15; 16]);
const JOURNEYS: ItemId = ItemId([0x16; 16]);
const PRAYER: ItemId = ItemId([0x17; 16]);
const COMMENTARY: ItemId = ItemId([0x18; 16]);
const OUTLINE: ItemId = ItemId([0x19; 16]);

/// Romans study: the note on the left, the sketch and the Bible right.
const STUDY: WorkspaceId = WorkspaceId([0x21; 16]);
/// One-document desks, for the phone shots.
const NOTE_DESK: WorkspaceId = WorkspaceId([0x22; 16]);
const SKETCH_DESK: WorkspaceId = WorkspaceId([0x23; 16]);

const BIBLE_ID: &str = "spa_rv1960";

/// Two people in the study with you.
const ANA: (notein_sync::DeviceId, &str) = (notein_sync::DeviceId([0x2A; 32]), "Ana");
const MARCO: (notein_sync::DeviceId, &str) = (notein_sync::DeviceId([0xC4; 32]), "Marco");

const NOTE_BODY: &str = "# Romans 8 — Life in the Spirit

Paul moves from *no condemnation* (v. 1) to *no separation* (v. 39). \
The whole chapter hangs on what the Spirit does for those who are in Christ.

## Outline

1. **Freed** from the law of sin and death — vv. 1–11
2. **Adopted** as children who cry *Abba, Father* — vv. 12–17
3. **Kept** through present suffering and future glory — vv. 18–30
4. **Secured** in a love nothing can break — vv. 31–39

> And we know that all things work together for good to them that love God.
> — Romans 8:28

## Key words

| Greek | Transliteration | Meaning | Times in ch. 8 |
|---|---|---|---|
| πνεῦμα | pneuma | spirit, breath | 21 |
| υἱοθεσία | huiothesia | adoption as sons | 2 |
| δόξα | doxa | glory | 2 |

## For Sunday

- [x] Read the chapter in two translations
- [x] Trace the cross-references of v. 28
- [ ] Compare *huiothesia* with Galatians 4:5
- [ ] Draft the closing illustration
";

const SERMON_BODY: &str = "# Sunday sermon — More than conquerors

Text: Romans 8:31–39

## Big idea

If God is for us, nothing that comes against us has the last word.
";

const GREEK_BODY: &str = "# Greek vocabulary

- **ἀγάπη** (agapē) — love
- **ἐλπίς** (elpis) — hope
- **χάρις** (charis) — grace
";

const PRAYER_BODY: &str = "# Prayer list

- [ ] The youth group retreat
- [ ] Healing for Ruth
- [x] The new study group
";

const OUTLINE_BODY: &str = "# Acts — reading plan

| Week | Chapters | Theme |
|---|---|---|
| 1 | 1–4 | The church is born |
| 2 | 5–8 | Opposition and growth |
| 3 | 9–12 | Saul and Peter |
";

fn real_item(id: ItemId, name: &str, kind: &str, folder: FolderId, age_days: u64) -> Item {
    let thumbnail = match id {
        SKETCH => crate::thumbnail::canvas(&sketch(), id, PAPER),
        JOURNEYS => crate::thumbnail::canvas(&journeys(), id, PAPER),
        _ => None,
    };
    Item {
        id,
        thumbnail: thumbnail.map(Arc::from),
        ..item(name, kind, folder, age_days)
    }
}

const PAPER: [u8; 4] = [0xFA, 0xF9, 0xF6, 0xFF];

fn desk(
    id: WorkspaceId,
    name: &str,
    color: [u8; 4],
    layout: PaneLayout,
    age_hours: u64,
) -> Workspace {
    let now = crate::format::now();
    let mut desk = Workspace {
        layout,
        ..Workspace::fresh(
            id,
            name.to_owned(),
            Some(color),
            now - age_hours * 60 * 60 * 1000,
        )
    };
    desk.named = true;
    desk
}

fn leaf(tabs: Vec<PaneTab>) -> PaneLayout {
    let mut pane = PaneLayout::leaf(crate::library::PaneId::new_v4());
    if let PaneLayout::Leaf(p) = &mut pane {
        p.tabs = tabs;
    }
    pane
}

/// Every document of [`library`], newest first.
fn items(study: &Folder, sermons: &Folder) -> Vec<Item> {
    vec![
        real_item(
            NOTE,
            "Romans 8 — study notes",
            notein_notes::kind::KIND,
            study.id,
            0,
        ),
        real_item(SKETCH, "Romans 8 — sketch", canvas::KIND, study.id, 0),
        real_item(
            BIBLE,
            "Reina-Valera 1960",
            notein_bible::kind::KIND,
            ROOT_FOLDER,
            0,
        ),
        real_item(
            SERMON,
            "More than conquerors",
            notein_notes::kind::KIND,
            sermons.id,
            1,
        ),
        real_item(
            GREEK,
            "Greek vocabulary",
            notein_notes::kind::KIND,
            ROOT_FOLDER,
            2,
        ),
        real_item(JOURNEYS, "Paul's journeys", canvas::KIND, ROOT_FOLDER, 4),
        real_item(
            PRAYER,
            "Prayer list",
            notein_notes::kind::KIND,
            ROOT_FOLDER,
            5,
        ),
        real_item(COMMENTARY, "Commentary on Romans", pdf::KIND, study.id, 6),
        real_item(
            OUTLINE,
            "Acts — reading plan",
            notein_notes::kind::KIND,
            ROOT_FOLDER,
            9,
        ),
    ]
}

/// The library of a Bible teacher halfway through a study on Romans.
fn library() -> LibraryData {
    let now = crate::format::now();
    let study = folder("Romans study", [0x7B, 0x8C, 0xFF, 0xFF]);
    let sermons = folder("Sermons", [0x5F, 0xB3, 0x8A, 0xFF]);
    let items = items(&study, &sermons);
    let split = PaneLayout::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(leaf(vec![tab(NOTE)])),
        second: Box::new(leaf(vec![tab(SKETCH), tab(BIBLE)])),
    };
    let mut romans = desk(STUDY, "Romans study", [0x7B, 0x8C, 0xFF, 0xFF], split, 1);
    romans.named = false;
    let index = LibraryIndex {
        version: crate::library::INDEX_VERSION,
        folders: vec![study, sermons],
        items,
        tags: Vec::new(),
        pins: Vec::new(),
    };
    let workspaces = WorkspaceIndex {
        version: crate::library::WORKSPACE_INDEX_VERSION,
        active: Some(STUDY),
        workspaces: vec![
            romans,
            desk(
                NOTE_DESK,
                "Sunday sermon",
                [0xE0, 0xA8, 0x45, 0xFF],
                leaf(vec![tab(NOTE), tab(SERMON)]),
                26,
            ),
            desk(
                SKETCH_DESK,
                "Acts reading",
                [0x5F, 0xB3, 0x8A, 0xFF],
                leaf(vec![tab(SKETCH)]),
                72,
            ),
        ],
    };
    let link = Link {
        id: LinkId::new_v4(),
        from: NOTE,
        to: SKETCH,
        kind: LinkKind::Companion,
        created_at: now,
        updated_at: now,
        deleted: false,
    };
    LibraryData::from_parts(
        index,
        workspaces,
        LinkIndex {
            version: 1,
            links: vec![link],
        },
    )
}

fn write_note(id: ItemId, body: &str) {
    let mut doc = Doc::default();
    doc.set_body(body);
    bodies::write_doc(id, &doc).expect("write note");
}

/// Handwriting from the app's own ink fonts, as recognized writing
/// redrawn on the canvas.
fn handwrite(doc: &mut Doc, text: &str, (x, y): (f32, f32), height: f32, color: [u8; 4]) {
    let font = crate::ink_glyph::GlyphFont::Caveat;
    let Some(ink) = font.ink() else {
        return;
    };
    let scale = height / f32::from(ink.units_per_em);
    let space = f32::from(ink.units_per_em) / 4.0;
    let mut pen = x;
    for glyph in text.chars() {
        let Some(outline) = ink.glyph(glyph) else {
            pen = space.mul_add(scale, pen);
            continue;
        };
        doc.emit(crate::doc_op::DocOp::AddGlyph {
            layer_id: 0,
            glyph: crate::ink_glyph::InkGlyph {
                id: crate::ink_glyph::InkGlyphId::new_v4(),
                glyph,
                font,
                x: pen,
                y,
                scale,
                color,
            },
        });
        pen = f32::from(outline.advance).mul_add(scale, pen);
    }
}

/// A pen line through `points`, with a little pressure swell.
fn stroke(doc: &mut Doc, brush: crate::brush::BrushId, color: [u8; 4], points: &[(f32, f32)]) {
    let last = points.len().saturating_sub(1).max(1);
    #[allow(clippy::cast_precision_loss, reason = "a handful of points")]
    let ink = points
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            let t = i as f32 / last as f32;
            let pressure = 0.35f32.mul_add((t * std::f32::consts::PI).sin(), 0.45);
            crate::brush::InkPoint::from_normalized(x, y, pressure, 0.0, 8_000)
        })
        .collect();
    doc.push_stroke(brush, color, ink);
}

/// Points along an ellipse, a little open like a hand-drawn ring.
fn ring((cx, cy): (f32, f32), (rx, ry): (f32, f32)) -> Vec<(f32, f32)> {
    #[allow(clippy::cast_precision_loss, reason = "a handful of points")]
    (0..=40)
        .map(|i| {
            let a = (i as f32 / 40.0).mul_add(std::f32::consts::TAU * 1.04, -0.4);
            (rx.mul_add(a.cos(), cx), ry.mul_add(a.sin(), cy))
        })
        .collect()
}

const INK: [u8; 4] = [0x1F, 0x2A, 0x44, 0xFF];
const RED: [u8; 4] = [0xC0, 0x48, 0x2F, 0xFF];
const BLUE: [u8; 4] = [0x3B, 0x5B, 0xDB, 0xFF];

/// The study sketch: the chapter's four movements as a handwritten map.
fn sketch() -> Doc {
    let mut doc = Doc::default();
    let pen = doc
        .register_brush(crate::brush::BrushPreset::pen())
        .expect("pen brush");
    let marker = doc
        .register_brush(crate::brush::BrushPreset::highlighter())
        .expect("highlighter brush");
    handwrite(&mut doc, "Romans 8", (60.0, 110.0), 64.0, INK);
    stroke(
        &mut doc,
        marker,
        [0xF5, 0xC5, 0x42, 0x90],
        &[(56.0, 100.0), (330.0, 96.0)],
    );
    let steps = [
        ("Freed", "v. 1-11", 200.0),
        ("Adopted", "v. 12-17", 330.0),
        ("Kept", "v. 18-30", 460.0),
        ("Secured", "v. 31-39", 590.0),
    ];
    for (word, verses, y) in steps {
        handwrite(&mut doc, word, (110.0, y), 46.0, INK);
        handwrite(&mut doc, verses, (330.0, y), 30.0, BLUE);
        stroke(
            &mut doc,
            pen,
            INK,
            &[(70.0, y - 16.0), (82.0, y - 4.0), (96.0, y - 28.0)],
        );
    }
    for pair in steps.windows(2) {
        let (from, to) = (pair[0].2 + 22.0, pair[1].2 - 60.0);
        stroke(
            &mut doc,
            pen,
            RED,
            &[(84.0, from), (80.0, f32::midpoint(from, to)), (84.0, to)],
        );
    }
    stroke(&mut doc, pen, RED, &ring((178.0, 444.0), (120.0, 42.0)));
    handwrite(
        &mut doc,
        "all things work together",
        (470.0, 430.0),
        30.0,
        RED,
    );
    handwrite(&mut doc, "for good - v. 28", (470.0, 470.0), 30.0, RED);
    stroke(
        &mut doc,
        pen,
        RED,
        &[
            (300.0, 450.0),
            (360.0, 446.0),
            (420.0, 445.0),
            (462.0, 446.0),
        ],
    );
    doc.emit(crate::doc_op::DocOp::AddText {
        layer_id: 0,
        text: crate::text_box::TextBox {
            id: crate::text_box::TextBoxId::new_v4(),
            frame: crate::text_box::TextFrame::at(470.0, 560.0),
            markdown: "**No separation** — height, depth, nor any other creature (v. 39)"
                .to_owned(),
            color: INK,
        },
    });
    doc
}

/// Paul's journeys: the route of the third one, by hand.
fn journeys() -> Doc {
    let mut doc = Doc::default();
    let pen = doc
        .register_brush(crate::brush::BrushPreset::pen())
        .expect("pen brush");
    handwrite(&mut doc, "Third journey", (60.0, 100.0), 56.0, INK);
    let stops = [
        ("Antioch", (520.0, 520.0)),
        ("Ephesus", (300.0, 330.0)),
        ("Corinth", (150.0, 420.0)),
        ("Jerusalem", (560.0, 640.0)),
    ];
    for (name, (x, y)) in stops {
        stroke(&mut doc, pen, RED, &ring((x, y), (10.0, 10.0)));
        handwrite(&mut doc, name, (x + 20.0, y + 12.0), 34.0, INK);
    }
    for pair in stops.windows(2) {
        let ((_, (x0, y0)), (_, (x1, y1))) = (pair[0], pair[1]);
        let mid = (f32::midpoint(x0, x1) + 30.0, f32::midpoint(y0, y1) - 30.0);
        stroke(&mut doc, pen, BLUE, &[(x0, y0), mid, (x1, y1)]);
    }
    doc
}

/// Copy the Bible folder in and pair its words with the originals.
/// Once per process.
fn bible_ready() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let source = std::env::var_os("NOTEIN_README_BIBLE")
            .map(PathBuf::from)
            .expect("NOTEIN_README_BIBLE: an app Bible folder with originals installed");
        let target = notein_bible::Db::default_path()
            .parent()
            .expect("bible folder")
            .to_path_buf();
        copy_dir(&source, &target);
        let db = notein_bible::Db::open_default().expect("open bible db");
        db.save_setting("originals", &notein_plugin::config::ConfigValue::Bool(true))
            .expect("originals on");
        assert!(
            db.list_bibles()
                .unwrap_or_default()
                .iter()
                .any(|b| b.id == BIBLE_ID),
            "{BIBLE_ID} is not installed in NOTEIN_README_BIBLE"
        );
        let store = notein_bible::originals::Store::open(&notein_bible::originals::dir())
            .expect("original packs");
        notein_bible::originals::align::run(&db, &store, BIBLE_ID).expect("align");
    });
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("create bible folder");
    for entry in std::fs::read_dir(from).expect("read bible folder") {
        let entry = entry.expect("bible entry");
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &dest);
        } else {
            std::fs::copy(&path, &dest).expect("copy bible file");
        }
    }
}

/// Book id of Romans.
fn romans() -> String {
    let db = notein_bible::Db::open_default().expect("open bible db");
    db.list_books(BIBLE_ID)
        .expect("books")
        .into_iter()
        .find(|b| b.name.starts_with("Rom"))
        .map(|b| b.book_id)
        .expect("Romans")
}

/// Write every document of [`library`] and open the Bible at Romans 8.
fn content_ready() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        sandbox();
        bible_ready();
        write_note(NOTE, NOTE_BODY);
        write_note(SERMON, SERMON_BODY);
        write_note(GREEK, GREEK_BODY);
        write_note(PRAYER, PRAYER_BODY);
        write_note(OUTLINE, OUTLINE_BODY);
        bodies::write_doc(SKETCH, &sketch()).expect("write sketch");
        bodies::write_doc(JOURNEYS, &journeys()).expect("write journeys");
        let state = notein_bible::state::BibleViewerState::new(BIBLE_ID, romans(), 8);
        let bytes = notein_bible::state::BibleSnapshot::from_state(&state).encode();
        bodies::write_plugin_snapshot(BIBLE, notein_bible::kind::KIND, &bytes).expect("snapshot");
    });
}

/// Caret state the notes face shares: field, anchor and head.
fn caret(target: u32, anchor: u32, head: u32) -> Vec<u8> {
    [target, anchor, head]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}

/// Ana selecting a line of the outline, Marco typing in the key words
/// table, both in the other documents of the desk.
fn people_here() {
    crate::sync::presence::pretend(NOTE, ANA.0, ANA.1, caret(4, 0, 46));
    crate::sync::presence::pretend(NOTE, MARCO.0, MARCO.1, caret(14, 34, 34));
    crate::sync::presence::pretend(SKETCH, MARCO.0, MARCO.1, Vec::new());
    crate::sync::presence::pretend(BIBLE, ANA.0, ANA.1, Vec::new());
}

/// Marco halfway through a ring on the sketch, his pointer beside it.
fn marco_drawing() {
    for (_, board) in crate::active_boards::for_item(SKETCH) {
        let mut board = crate::canvas::lock(&board);
        let points = ring((215.0, 690.0), (125.0, 26.0))
            .into_iter()
            .take(30)
            .map(|(x, y)| crate::brush::InkPoint::from_normalized(x, y, 0.6, 0.0, 8_000))
            .collect();
        board.apply_live(
            MARCO.0,
            MARCO.1,
            crate::sync::live::LiveMessage::Ink {
                stroke: crate::ids::StrokeId::new_v4(),
                preset: crate::brush::BrushPreset::pen(),
                color: BLUE,
                opacity: 1.0,
                points,
                done: false,
            },
        );
        board.apply_live(
            MARCO.0,
            MARCO.1,
            crate::sync::live::LiveMessage::Pointer {
                x: 345.0,
                y: 684.0,
                shown: true,
            },
        );
        // Ghost ink does not move the layout, and a headless frame only
        // repaints what moved: nudge the zoom so the overlays paint again.
        board.set_viewport(crate::canvas::Viewport {
            tx: 0.0,
            ty: 0.0,
            scale: 0.821,
        });
    }
}

/// Load the sketch into its board: canvases read their document through
/// the app runtime, which previews do not start.
fn sketch_loaded() {
    for (_, board) in crate::active_boards::for_item(SKETCH) {
        let mut board = crate::canvas::lock(&board);
        board.replace_doc(sketch());
        board.set_viewport(crate::canvas::Viewport {
            tx: 0.0,
            ty: 0.0,
            scale: 0.82,
        });
    }
}

/// Render `body` over [`library`], pressing `presses` in order once the
/// screen settled.
fn shoot(name: &str, size: (f32, f32), body: fn() -> Element, presses: &[(f64, f64)]) {
    content_ready();
    let app = shell_with(library, body, size, notein_ui_kit::Gutters::default());
    let (mut runner, ()) = TestingRunner::new(app, size.into(), |_| (), 1.0);
    runner.poll_n(Duration::from_millis(40), 12);
    runner.sync_and_update();
    sketch_loaded();
    people_here();
    runner.poll_n(Duration::from_millis(40), 8);
    runner.sync_and_update();
    for &at in presses {
        runner.click_cursor(at);
        runner.poll_n(Duration::from_millis(40), 12);
        runner.sync_and_update();
    }
    runner.poll_n(Duration::from_millis(40), 6);
    runner.sync_and_update();
    // The sketch's own load lands at no fixed frame and drops other
    // people's ghost ink: draw Marco again on every one of the last frames.
    people_here();
    for _ in 0..4 {
        marco_drawing();
        runner.poll_n(Duration::from_millis(40), 3);
        runner.sync_and_update();
    }
    runner.render_to_file(out(name));
}

fn english() {
    notein_i18n::Lang::En.make_current();
    crate::plugins::catalog::set_lang(notein_i18n::Lang::En);
}

fn readme_home() -> Element {
    use_hook(english);
    home()
}

fn study_desk() -> Element {
    use_hook(english);
    mesa_view(Some(STUDY))
}

fn note_desk() -> Element {
    use_hook(english);
    mesa_view(Some(NOTE_DESK))
}

fn sketch_desk() -> Element {
    use_hook(english);
    mesa_view(Some(SKETCH_DESK))
}

fn bible() -> Element {
    use_hook(english);
    crate::plugins::faces::render(
        BIBLE,
        &crate::library::DocKind::plugin(notein_bible::kind::KIND),
        None,
    )
}

const DESKTOP: (f32, f32) = (1440.0, 900.0);
const PHONE: (f32, f32) = (390.0, 844.0);

#[test]
#[ignore = "needs an installed Bible with the originals; see the module docs"]
fn readme_desktop() {
    // First: Marco's ghost ink only shows in a process's first render.
    shoot("readme-desk", DESKTOP, study_desk, &[]);
    shoot("readme-home", DESKTOP, readme_home, &[]);
    // The alpha after verse 1, then the Original tab of its panel.
    shoot("readme-bible", DESKTOP, bible, &[(503.0, 238.0)]);
    shoot(
        "readme-bible-original",
        DESKTOP,
        bible,
        &[(503.0, 238.0), (1191.0, 143.0)],
    );
}

#[test]
#[ignore = "needs an installed Bible with the originals; see the module docs"]
fn readme_phone() {
    // First, like the desk of `readme_desktop`: see the module docs.
    shoot("readme-phone-sketch", PHONE, sketch_desk, &[]);
    shoot("readme-phone-home", PHONE, readme_home, &[]);
    shoot("readme-phone-note", PHONE, note_desk, &[]);
    shoot("readme-phone-bible", PHONE, bible, &[(148.0, 345.0)]);
    shoot(
        "readme-phone-bible-original",
        PHONE,
        bible,
        &[(148.0, 345.0), (84.0, 238.0)],
    );
}

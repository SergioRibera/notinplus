//! A selection bar fits the room it is given: labels, then icons alone,
//! then without its caption; never cut text.

use std::path::PathBuf;

use freya::icons::lucide;
use freya::prelude::*;
use freya_testing::prelude::*;
use notin_ui_kit::{BarItem, SelectionBar};

fn items() -> Vec<BarItem> {
    vec![
        BarItem::Caption("3 seleccionados".into()),
        BarItem::Divider,
        BarItem::action("Fijar", lucide::pin(), |()| {}),
        BarItem::action("Exportar", lucide::download(), |()| {}),
        BarItem::action("Nueva carpeta con 3", lucide::folder_plus(), |()| {}),
        BarItem::action("Mover", lucide::folder_input(), |()| {}),
        BarItem::action("Papelera", lucide::trash_2(), |()| {}),
    ]
}

fn bar(room: f32) -> impl Fn() -> Element + Clone {
    move || {
        rect()
            .expanded()
            .background((20, 20, 24))
            .center()
            .child(SelectionBar::new(items()).fit(room))
            .into_element()
    }
}

/// Width of the bar: the node as tall as a bar.
fn bar_width(runner: &TestingRunner) -> f32 {
    runner
        .find(|node, _| {
            let area = node.layout().area;
            (48.0..60.0).contains(&area.height()).then(|| area.width())
        })
        .expect("bar laid out")
}

fn run(name: &str, room: f32) -> f32 {
    let (mut runner, ()) = TestingRunner::new(bar(room), (room + 40.0, 120.).into(), |_| {}, 1.0);
    for _ in 0..6 {
        runner.poll_n(std::time::Duration::from_millis(16), 2);
        runner.sync_and_update();
    }
    runner.render_to_file(PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png")));
    bar_width(&runner)
}

#[test]
fn labels_when_they_fit_icons_when_not() {
    let wide = run("selection_bar_wide", 900.0);
    let narrow = run("selection_bar_narrow", 330.0);
    assert!(wide > 330.0, "labels shown when there is room: {wide}");
    assert!(narrow <= 330.0, "never wider than its room: {narrow}");
}

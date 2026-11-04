use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use freya::prelude::*;
use freya_markdown_editor::document::BlockId;
use freya_markdown_editor::prelude::*;
use freya_testing::prelude::*;

const NOTE: &str = "# Title\n\nA paragraph.\n\n- one\n- two\n\n```\ncode\n```\n\nLast line.\n";

/// What every block's shell was last handed.
#[derive(Clone, Default)]
struct Seen(Rc<RefCell<BTreeMap<BlockId, (f32, bool)>>>);

fn app() -> impl IntoElement {
    let seen = use_consume::<Seen>();
    let markdown = use_state(|| NOTE.to_owned());
    MarkdownEditor::new(markdown)
        .block_shell(move |block| {
            seen.0
                .borrow_mut()
                .insert(block.block, (block.height_hint, block.pinned));
            block.content
        })
        .width(Size::fill())
}

fn run() -> (TestingRunner, Seen) {
    let (mut runner, seen) = TestingRunner::new(
        app,
        (600., 400.).into(),
        |runner| {
            let seen = Seen::default();
            let shared = seen.clone();
            runner.provide_root_context(move || shared);
            seen
        },
        1.0,
    );
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
    (runner, seen)
}

#[test]
fn every_block_passes_through_the_shell_with_a_height() {
    let (_runner, seen) = run();
    let blocks = Document::parse(NOTE).blocks().len();
    let seen = seen.0.borrow();
    assert_eq!(seen.len(), blocks, "{seen:?}");
    assert!(seen.values().all(|(hint, _)| *hint > 0.0), "{seen:?}");
    assert!(seen.values().all(|(_, pinned)| !pinned), "{seen:?}");
}

#[test]
fn the_focused_block_is_pinned() {
    let (mut runner, seen) = run();
    runner.click_cursor((40., 20.));
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
    let seen = seen.0.borrow();
    let pinned = seen.values().filter(|(_, pinned)| *pinned).count();
    assert_eq!(pinned, 1, "{seen:?}");
}

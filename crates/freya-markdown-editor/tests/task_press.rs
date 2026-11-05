//! Pressing a task's check box ticks it and nothing else: the press must
//! not reach the editor root, which takes presses on empty space as a
//! request to focus the end of the note.

use std::time::Duration;

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use freya_testing::prelude::*;

const NOTE: &str = "- [ ] one\n  - [ ] two\n\nafter\n";

fn app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    let controller = use_consume::<EditorController>();
    ScrollView::new().expanded().child(
        rect().expanded().background((30, 32, 38)).child(
            MarkdownEditor::new(value)
                .controller(controller)
                .reorderable(true)
                .width(Size::fill()),
        ),
    )
}

fn run() -> (TestingRunner, State<String>, EditorController) {
    let (mut runner, (value, controller)) = TestingRunner::new(
        app,
        (400., 300.).into(),
        |runner| {
            let value = runner.provide_root_context(|| State::create(NOTE.to_owned()));
            let controller =
                runner.provide_root_context(|| EditorController::create(Document::default()));
            (value, controller)
        },
        1.0,
    );
    settle(&mut runner);
    (runner, value, controller)
}

fn settle(runner: &mut TestingRunner) {
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
}

/// Window position of the check box of block `index`: the drag handle
/// column, the indent, then the middle of the gutter.
fn marker_at(runner: &mut TestingRunner, controller: EditorController, index: usize) -> (f64, f64) {
    let top = runner.run_in(|| {
        let block = controller.document().blocks()[index].id;
        f64::from(controller.block_offset(block).expect("laid out"))
    });
    (20. + 24. * index as f64 + 12., top + 14.)
}

fn expected(index: usize) -> &'static str {
    if index == 0 {
        "- [x] one\n  - [ ] two\n\nafter\n"
    } else {
        "- [ ] one\n  - [x] two\n\nafter\n"
    }
}

fn check(index: usize, touch: bool) {
    let (mut runner, value, controller) = run();
    let at = marker_at(&mut runner, controller, index);
    if touch {
        runner.press_touch(at);
        runner.release_touch(at);
    } else {
        runner.click_cursor(at);
    }
    settle(&mut runner);
    assert_eq!(&*value.peek(), expected(index), "at {at:?}");
    // Nothing was focused before, and the press focused nothing: the
    // root did not take it as a tap past the last block.
    let focused = runner.run_in(|| controller.text_position());
    assert!(focused.is_none(), "the press moved the caret: {focused:?}");
}

#[test]
fn a_click_ticks_an_item_without_moving_the_caret() {
    check(0, false);
}

#[test]
fn a_click_ticks_a_nested_item_without_moving_the_caret() {
    check(1, false);
}

#[test]
fn a_tap_ticks_an_item_without_moving_the_caret() {
    check(0, true);
}

#[test]
fn a_tap_ticks_a_nested_item_without_moving_the_caret() {
    check(1, true);
}

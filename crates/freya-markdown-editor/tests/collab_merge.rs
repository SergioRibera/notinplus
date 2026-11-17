//! Text merged in from elsewhere keeps the editor as it was around it.

use std::rc::Rc;
use std::time::Duration;

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use freya_testing::prelude::*;

#[test]
fn blocks_left_as_they_were_keep_their_identity() {
    let before = Document::parse("one\n\ntwo\n\nthree");
    let mut after = Document::parse("one\n\ntwo and more\n\nnew\n\nthree");
    after.keep_identity(&before);
    let (old, new) = (before.blocks(), after.blocks());
    assert!(Rc::ptr_eq(&old[0], &new[0]));
    assert_eq!(new[1].id, old[1].id, "the edited paragraph keeps its id");
    assert!(Rc::ptr_eq(&old[2], &new[3]));
    assert!(new[2].id != old[0].id && new[2].id != old[1].id && new[2].id != old[2].id);
}

fn app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    let controller = use_consume::<EditorController>();
    MarkdownEditor::new(value)
        .controller(controller)
        .width(Size::fill())
}

#[test]
fn a_merge_moves_the_caret_with_the_text_typed_before_it() {
    let (mut runner, (value, controller)) = TestingRunner::new(
        app,
        (400., 300.).into(),
        |runner| {
            let value = runner.provide_root_context(|| State::create("one\n\ntwo".to_owned()));
            let controller =
                runner.provide_root_context(|| EditorController::create(Document::default()));
            (value, controller)
        },
        1.0,
    );
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
    let ids = |runner: &mut TestingRunner| {
        runner.run_in(|| {
            controller
                .document()
                .blocks()
                .iter()
                .map(|block| block.id)
                .collect::<Vec<_>>()
        })
    };
    let before = ids(&mut runner);
    runner.run_in(|| controller.focus(before[1], 3));
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();

    let mut value = value;
    runner.run_in(|| value.set("one\n\nsay two".to_owned()));
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();

    assert_eq!(ids(&mut runner), before);
    let selection = runner
        .run_in(|| controller.selection())
        .expect("caret kept");
    assert_eq!(selection.target.block, before[1]);
    assert_eq!((selection.anchor, selection.head), (7, 7));
}

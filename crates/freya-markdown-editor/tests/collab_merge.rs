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

type Reports = std::rc::Rc<std::cell::RefCell<Vec<InputContext>>>;

fn reporting_app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    let controller = use_consume::<EditorController>();
    let reports = use_consume::<Reports>();
    MarkdownEditor::new(value)
        .controller(controller)
        .on_input_context(move |context| reports.borrow_mut().push(context.clone()))
        .width(Size::fill())
}

#[test]
fn text_merged_while_composing_shows_and_survives_the_commit() {
    let (mut runner, (value, controller, reports)) = TestingRunner::new(
        reporting_app,
        (400., 300.).into(),
        |runner| {
            let value = runner.provide_root_context(|| State::create("hola mundo".to_owned()));
            let controller =
                runner.provide_root_context(|| EditorController::create(Document::default()));
            let reports = runner.provide_root_context(Reports::default);
            (value, controller, reports)
        },
        1.0,
    );
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
    let block = runner.run_in(|| controller.document().blocks()[0].id);
    runner.run_in(|| controller.focus(block, 10));
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();

    let preedit = |runner: &mut TestingRunner, text: &str| {
        runner.send_event(PlatformEvent::ImePreedit {
            name: ImeEventName::Preedit,
            text: text.to_owned(),
            cursor: None,
        });
        runner.poll_n(Duration::from_millis(1), 3);
        runner.sync_and_update();
    };
    preedit(&mut runner, " dí");

    let mut value = value;
    runner.run_in(|| value.set("Ana: hola mundo".to_owned()));
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
    let shown = reports.borrow().last().map(|context| context.text.clone());
    assert_eq!(shown.as_deref(), Some("Ana: hola mundo dí"));

    preedit(&mut runner, "");
    runner.write_text(" dí");
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
    assert_eq!(&*value.peek(), "Ana: hola mundo dí\n");
}

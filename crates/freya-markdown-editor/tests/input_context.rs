use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use freya::prelude::*;
use freya_markdown_editor::document::TextTarget;
use freya_markdown_editor::prelude::*;
use freya_testing::prelude::*;

type Reports = Rc<RefCell<Vec<InputContext>>>;

fn app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    let controller = use_consume::<EditorController>();
    let reports = use_consume::<Reports>();
    MarkdownEditor::new(value)
        .controller(controller)
        .on_input_context(move |context| reports.borrow_mut().push(context.clone()))
        .width(Size::fill())
}

fn key(runner: &mut TestingRunner, key: &Key) {
    for name in [KeyboardEventName::KeyDown, KeyboardEventName::KeyUp] {
        runner.send_event(PlatformEvent::Keyboard {
            name,
            key: key.clone(),
            code: Code::Unidentified,
            modifiers: Modifiers::empty(),
        });
        runner.sync_and_update();
    }
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
}

#[test]
fn input_context_follows_focus_and_jumps_but_not_typing() {
    let (mut runner, (controller, reports)) = TestingRunner::new(
        app,
        (400., 300.).into(),
        |runner| {
            runner.provide_root_context(|| State::create("héllo\n\nsecond".to_owned()));
            let controller =
                runner.provide_root_context(|| EditorController::create(Document::default()));
            let reports = runner.provide_root_context(Reports::default);
            (controller, reports)
        },
        1.0,
    );
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
    let block =
        |index: usize| runner.run_in(|| TextTarget::from(controller.document().blocks()[index].id));
    let (first, second) = (block(0), block(1));
    let focus = |runner: &mut TestingRunner, target: TextTarget, offset: usize| {
        runner.run_in(|| controller.focus(target, offset));
        runner.poll_n(Duration::from_millis(1), 3);
        runner.sync_and_update();
    };

    focus(&mut runner, first, 2);
    let expected = InputContext {
        text: "héllo".to_owned(),
        cursor: 3,
        anchor: 3,
    };
    assert_eq!(reports.borrow().last(), Some(&expected));
    let after_focus = reports.borrow().len();

    key(&mut runner, &Key::Character("x".to_owned()));
    key(&mut runner, &Key::Named(NamedKey::Backspace));
    assert_eq!(
        reports.borrow().len(),
        after_focus,
        "typing is not reported"
    );

    focus(&mut runner, second, 6);
    assert_eq!(
        reports.borrow().last(),
        Some(&InputContext {
            text: "second".to_owned(),
            cursor: 6,
            anchor: 6,
        })
    );
}

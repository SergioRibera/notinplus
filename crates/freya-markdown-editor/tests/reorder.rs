use std::time::Duration;

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use freya_testing::prelude::*;

fn app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    let controller = use_consume::<EditorController>();
    ScrollView::new().expanded().child(
        MarkdownEditor::new(value)
            .controller(controller)
            .reorderable(true)
            .width(Size::fill()),
    )
}

#[test]
fn touch_drag_on_the_handle_reorders_inside_a_scroll_view() {
    let (mut runner, (value, controller)) = TestingRunner::new(
        app,
        (400., 300.).into(),
        |runner| {
            let value =
                runner.provide_root_context(|| State::create("one\n\ntwo\n\nthree".to_owned()));
            let controller =
                runner.provide_root_context(|| EditorController::create(Document::default()));
            (value, controller)
        },
        1.0,
    );
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();

    let top = |index: usize| {
        runner.run_in(|| {
            let block = controller.document().blocks()[index].id;
            f64::from(controller.block_offset(block).expect("laid out"))
        })
    };
    let (from, to) = (top(0) + 12., top(2) + 12.);

    runner.press_touch((10., from));
    for step in 1..=8 {
        let y = from + (to - from) * f64::from(step) / 8.;
        runner.move_touch((10., y));
    }
    runner.release_touch((10., to));
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();

    assert_eq!(&*value.peek(), "two\n\nthree\n\none\n");
}

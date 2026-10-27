use std::path::PathBuf;

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use freya_markdown_math::TypstMath;
use freya_testing::prelude::*;

const SAMPLE: &str = "La energía es $E = mc^2$ y la suma vale:\n\n$$\n\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}\n$$\n\nFin.\n";

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"))
}

fn app() -> impl IntoElement {
    let value = use_state(|| SAMPLE.to_owned());
    let controller = use_consume::<EditorController>();
    rect()
        .expanded()
        .background(EditorTheme::dark().colors.surface)
        .padding(12.)
        .child(
            MarkdownEditor::new(value)
                .controller(controller)
                .decorator(TypstMath)
                .width(Size::fill()),
        )
}

#[test]
fn formulas_render_and_edit() {
    let (mut runner, controller) = TestingRunner::new(
        app,
        (560., 420.).into(),
        |runner| runner.provide_root_context(|| EditorController::create(Document::default())),
        1.0,
    );
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("math_view"));
    let block = runner.run_in(|| controller.document().blocks()[1].id);
    runner.run_in(|| controller.focus(block, 0));
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("math_edit"));
    runner.click_cursor((135., 26.));
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("math_inline_edit"));
}

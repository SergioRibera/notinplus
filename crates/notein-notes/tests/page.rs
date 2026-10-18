use std::path::PathBuf;
use std::time::Duration;

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use freya_testing::prelude::*;
use notein_notes::ui::{self, TopbarActions};

const NOTE: &str = "# Plan de estudio\n\nRepasar **Efesios 6** y anotar ideas.\n\n## Lecturas\n\n- [x] Capítulo 1\n- [ ] Capítulo 2\n\n## Preguntas\n\n> ¿Qué significa *la armadura*?\n";

fn page() -> impl IntoElement {
    let markdown = use_state(|| NOTE.to_owned());
    let controller = use_editor_controller();
    let action = |_: ()| Box::new(|| {}) as ui::Action;
    rect()
        .expanded()
        .background(ui::BACKGROUND)
        .content(Content::flex())
        .child(ui::topbar(
            "Estudio bíblico",
            false,
            TopbarActions {
                on_home: Some(action(())),
                on_sidebar: Some(action(())),
                on_toggle_mode: Some(action(())),
                on_split_horizontal: Some(action(())),
                on_split_vertical: Some(action(())),
                on_close_split: None,
            },
        ))
        .child(
            rect()
                .width(Size::fill())
                .padding((0., 16., 8., 16.))
                .child(MarkdownToolbar::new(controller).mode_toggle(false)),
        )
        .child(
            ScrollView::new()
                .width(Size::fill())
                .height(Size::flex(1.))
                .child(
                    rect()
                        .width(Size::fill())
                        .cross_align(Alignment::Center)
                        .child(
                            MarkdownEditor::new(markdown)
                                .controller(controller)
                                .reorderable(true)
                                .width(Size::fill())
                                .max_width(Size::px(760.))
                                .padding((24., 20., 160., 20.)),
                        ),
                ),
        )
}

#[test]
fn page_renders() {
    let (mut runner, ()) = TestingRunner::new(page, (900., 640.).into(), |_| (), 1.0);
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("notes_page.png");
    runner.render_to_file(path);
}

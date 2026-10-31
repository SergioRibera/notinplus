use std::path::PathBuf;

use freya::prelude::*;
use freya_markdown_diagrams::MermaidDiagrams;
use freya_markdown_editor::prelude::*;
use freya_testing::prelude::*;

const SAMPLE: &str = "Flujo:\n\n```mermaid\nflowchart TD\n    inicio[Abrir nota] --> pregunta{¿Tiene diagrama?}\n    pregunta -->|Sí| dibujar[Dibujarlo]\n    pregunta -->|No| texto[Mostrar texto]\n    dibujar --> fin((Listo))\n    texto --> fin\n```\n\n```mermaid\nsequenceDiagram\n    Ana->>Beto: Hola\n    Beto-->>Ana: ¿Qué tal?\n```\n\n```mermaid\npie title Tareas\n    \"Hechas\" : 7\n    \"Pendientes\" : 3\n```\n";

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
            ScrollView::new().expanded().child(
                MarkdownEditor::new(value)
                    .controller(controller)
                    .decorator(MermaidDiagrams)
                    .width(Size::fill()),
            ),
        )
}

#[test]
fn diagrams_render_and_edit() {
    let (mut runner, controller) = TestingRunner::new(
        app,
        (600., 1300.).into(),
        |runner| runner.provide_root_context(|| EditorController::create(Document::default())),
        1.0,
    );
    runner.poll_n(std::time::Duration::from_millis(16), 30);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("mermaid_view"));
    let block = runner.run_in(|| controller.document().blocks()[1].id);
    runner.run_in(|| controller.focus(block, 0));
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("mermaid_edit"));
}

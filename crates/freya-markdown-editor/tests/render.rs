use std::path::PathBuf;

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use freya_testing::prelude::*;

const SAMPLE: &str = r#"# Release notes {#notes}

Some **bold**, *italic*, <u>underlined</u>, ~~gone~~, ==marked==, `code`, H~2~O and x^2^.
<span style="color: #ef4444; font-size: 20px">Big red</span> and a [link](https://freyaui.dev).

> [!TIP] Shortcut
> Type `/` to open the block menu.

- [x] Parser
- [ ] Toolbar
  - nested bullet
1. first
2. second

> Quoted text

```rust
fn main() {
    println!("hi");
}
```

| Feature | Status |
|:--------|-------:|
| Tables  | done   |

---

Footnote ref[^1].

[^1]: The note.
"#;

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"))
}

fn editor_app() -> impl IntoElement {
    let value = use_state(|| SAMPLE.to_owned());
    let controller = use_editor_controller();
    rect()
        .expanded()
        .background(EditorTheme::dark().colors.surface)
        .padding(12.)
        .spacing(10.)
        .child(MarkdownToolbar::new(controller))
        .child(
            ScrollView::new().expanded().child(
                MarkdownEditor::new(value)
                    .controller(controller)
                    .slash_menu()
                    .decorator(PrefixTokens::new(
                        '#',
                        SpanStyle::default().color((168, 85, 247)),
                    ))
                    .width(Size::fill()),
            ),
        )
}

#[test]
fn renders_rich_document() {
    let (mut runner, ()) = TestingRunner::new(editor_app, (900., 1100.).into(), |_| {}, 1.0);
    runner.sync_and_update();
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    let path = snapshot_path("editor");
    runner.render_to_file(&path);
    assert!(path.exists());
}

fn typing_app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    rect()
        .expanded()
        .background((30, 32, 38))
        .padding(12.)
        .child(
            MarkdownEditor::new(value)
                .placeholder("Start typing")
                .width(Size::fill()),
        )
}

#[test]
fn typing_writes_markdown_back() {
    let (mut runner, value) = TestingRunner::new(
        typing_app,
        (500., 300.).into(),
        |runner| runner.provide_root_context(|| State::create(String::new())),
        1.0,
    );
    runner.sync_and_update();
    runner.click_cursor((30., 22.));
    for ch in "# Title".chars() {
        runner.write_text(ch);
    }
    runner.press_key(Key::Named(NamedKey::Enter));
    for ch in "hello **world**".chars() {
        runner.write_text(ch);
    }
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("typing"));
    assert_eq!(value.peek().as_str(), "# Title\n\nhello **world**\n");
}

fn toolbar_app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    let controller = use_consume::<EditorController>();
    rect()
        .expanded()
        .background(EditorTheme::dark().colors.surface)
        .padding(12.)
        .spacing(10.)
        .child(MarkdownToolbar::new(controller))
        .child(
            MarkdownEditor::new(value)
                .controller(controller)
                .width(Size::fill()),
        )
}

#[test]
fn toolbar_menus_render() {
    let (mut runner, controller) = TestingRunner::new(
        toolbar_app,
        (900., 520.).into(),
        |runner| {
            runner.provide_root_context(|| State::create("Pick **some** text".to_owned()));
            runner.provide_root_context(|| EditorController::create(Document::default()))
        },
        1.0,
    );
    runner.poll_n(std::time::Duration::from_millis(1), 3);
    runner.run_in(|| controller.focus_end());
    runner.poll_n(std::time::Duration::from_millis(1), 3);
    runner.click_cursor((155., 27.));
    runner.poll_n(std::time::Duration::from_millis(1), 3);
    runner.render_to_file(snapshot_path("toolbar_block_menu"));
    runner.click_cursor((155., 27.));
    runner.click_cursor((725., 27.));
    runner.poll_n(std::time::Duration::from_millis(1), 3);
    runner.render_to_file(snapshot_path("toolbar_palette"));
}

fn sections_app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    let controller = use_consume::<EditorController>();
    rect()
        .expanded()
        .background(EditorTheme::dark().colors.surface)
        .padding(12.)
        .child(
            MarkdownEditor::new(value)
                .controller(controller)
                .reorderable(true)
                .suggestions(SuggestionGroup::new("People", |query| {
                    ["Ada Lovelace", "Alan Turing", "Grace Hopper"]
                        .into_iter()
                        .filter(|name| name.to_lowercase().contains(query))
                        .map(|name| {
                            Suggestion::new(name, SuggestionAction::InsertText(name.into()))
                                .icon("@")
                        })
                        .collect()
                }))
                .slash_menu()
                .width(Size::fill()),
        )
}

#[test]
fn slash_menu_sections_and_handles_render() {
    let (mut runner, controller) = TestingRunner::new(
        sections_app,
        (700., 520.).into(),
        |runner| {
            runner.provide_root_context(|| {
                State::create("# Plan\n\nFirst line\n\n- item".to_owned())
            });
            runner.provide_root_context(|| EditorController::create(Document::default()))
        },
        1.0,
    );
    runner.poll_n(std::time::Duration::from_millis(1), 3);
    runner.run_in(|| controller.focus_end());
    runner.poll_n(std::time::Duration::from_millis(1), 3);
    runner.write_text("/");
    runner.write_text("a");
    runner.poll_n(std::time::Duration::from_millis(1), 3);
    runner.render_to_file(snapshot_path("slash_sections_handles"));
}

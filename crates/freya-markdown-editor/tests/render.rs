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

#[test]
fn renders_header_menus() {
    let (mut runner, ()) = TestingRunner::new(editor_app, (900., 1100.).into(), |_| {}, 1.0);
    runner.sync_and_update();
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.click_cursor((60., 205.));
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    let callout = snapshot_path("callout_menu");
    runner.render_to_file(&callout);
    runner.click_cursor((600., 900.));
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.click_cursor((55., 447.));
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    let language = snapshot_path("language_menu");
    runner.render_to_file(&language);
    assert!(callout.exists() && language.exists());
}

#[test]
fn code_fence_opens_the_language_picker() {
    let (mut runner, value) = TestingRunner::new(
        typing_app,
        (500., 420.).into(),
        |runner| runner.provide_root_context(|| State::create(String::new())),
        1.0,
    );
    runner.sync_and_update();
    runner.click_cursor((30., 22.));
    for ch in "```".chars() {
        runner.write_text(ch);
    }
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    for ch in "py".chars() {
        runner.write_text(ch);
    }
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("code_fence_picker"));
    runner.press_key(Key::Named(NamedKey::Enter));
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    for ch in "x = 1".chars() {
        runner.write_text(ch);
    }
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    assert_eq!(value.peek().as_str(), "```python\nx = 1\n```\n");
}

const LOOSE_HTML: &str = "## 12. HTML suelto\n\n<details>\n<summary>Detalles plegables</summary>\n\nContenido dentro de `<details>`.\n\n</details>\n\nLínea con `Ctrl` + `Z`, un \\\nsalto HTML y un <abbr title=\"ejemplo\">abbr</abbr>.\n\n<!-- Un comentario HTML que no debería verse -->\n\n> [!WARNING] Ojo\n> Texto del aviso.\n\n<div class=\"card\">\n  <span>hola</span>\n</div>\n";

fn loose_html_app() -> impl IntoElement {
    let value = use_state(|| LOOSE_HTML.to_owned());
    let read_only = use_consume::<bool>();
    rect()
        .expanded()
        .background(EditorTheme::dark().colors.surface)
        .padding(12.)
        .child(
            MarkdownEditor::new(value)
                .read_only(read_only)
                .width(Size::fill()),
        )
}

#[test]
fn renders_loose_html() {
    for read_only in [false, true] {
        let (mut runner, ()) = TestingRunner::new(
            loose_html_app,
            (600., 520.).into(),
            |runner| {
                runner.provide_root_context(|| read_only);
            },
            1.0,
        );
        runner.sync_and_update();
        runner.poll_n(std::time::Duration::from_millis(16), 4);
        runner.sync_and_update();
        let name = if read_only {
            "loose_html_read"
        } else {
            "loose_html_edit"
        };
        runner.render_to_file(snapshot_path(name));
        runner.click_cursor((120., 86.));
        runner.poll_n(std::time::Duration::from_millis(16), 4);
        runner.sync_and_update();
        runner.render_to_file(snapshot_path(&format!("{name}_open")));
        runner.click_cursor((150., 172.));
        runner.poll_n(std::time::Duration::from_millis(16), 4);
        runner.sync_and_update();
        runner.render_to_file(snapshot_path(&format!("{name}_abbr")));
    }
}

struct Swatch;

impl Decorator for Swatch {
    fn render_image(&self, image: &ImageContext<'_>) -> Option<Element> {
        Some(
            rect()
                .width(Size::px(image.max_width.min(240.)))
                .height(Size::px(120.))
                .background((80, 120, 200))
                .corner_radius(8.)
                .into_element(),
        )
    }
}

fn image_app() -> impl IntoElement {
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
                .decorator(Swatch)
                .image_picker(ImagePicker::new(|controller, request| match request {
                    ImageRequest::Insert => controller.insert_image("assets/new.png", ""),
                    ImageRequest::Replace(slot) => {
                        controller.set_image_url(slot, "assets/other.png");
                    }
                }))
                .width(Size::fill()),
        )
}

#[test]
fn images_render_and_offer_actions() {
    let (mut runner, (value, controller)) = TestingRunner::new(
        image_app,
        (700., 520.).into(),
        |runner| {
            let value = runner.provide_root_context(|| {
                State::create("Intro\n\n![a cat](assets/cat.png)\n".to_owned())
            });
            let controller =
                runner.provide_root_context(|| EditorController::create(Document::default()));
            (value, controller)
        },
        1.0,
    );
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("image_block"));
    runner.click_cursor((80., 150.));
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("image_menu"));
    runner.run_in(|| {
        controller.request_image(ImageRequest::Replace(AtomSlot {
            target: controller.document().blocks()[1].id.into(),
            atom: 0,
        }));
    });
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    assert!(
        value.peek().contains("![a cat](assets/other.png)"),
        "{}",
        value.peek()
    );
}

fn front_matter_app() -> impl IntoElement {
    let value = use_state(|| {
        "---\ntitle: Stress\ndate: 2026-10-27\ntags: [md, test]\n---\n\n# Body\n".to_owned()
    });
    rect()
        .expanded()
        .background(EditorTheme::dark().colors.surface)
        .padding(12.)
        .child(MarkdownEditor::new(value).width(Size::fill()))
}

#[test]
fn renders_front_matter_as_properties() {
    let (mut runner, ()) = TestingRunner::new(front_matter_app, (520., 420.).into(), |_| {}, 1.0);
    runner.sync_and_update();
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("properties_closed"));
    runner.click_cursor((60., 34.));
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("properties_open"));
    runner.click_cursor((120., 70.));
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("properties_form"));
}

#[test]
fn header_menus_fit_a_phone() {
    let (mut runner, ()) = TestingRunner::new(editor_app, (390., 760.).into(), |_| {}, 1.0);
    runner.sync_and_update();
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("phone_editor"));
}

fn remote_app() -> impl IntoElement {
    let value = use_state(|| {
        "# Plan\n\nPrimera línea del plan.\n\nSegunda idea con más texto.\n".to_owned()
    });
    rect()
        .expanded()
        .background(EditorTheme::dark().colors.surface)
        .padding(20.)
        .child(
            MarkdownEditor::new(value)
                .remote_carets(vec![
                    RemoteCaret {
                        position: TextPosition {
                            target: 1,
                            anchor: 0,
                            head: 7,
                        },
                        name: "Ana".to_owned(),
                        color: Color::from_rgb(96, 165, 250),
                    },
                    RemoteCaret {
                        position: TextPosition {
                            target: 2,
                            anchor: 14,
                            head: 14,
                        },
                        name: "Beto".to_owned(),
                        color: Color::from_rgb(244, 114, 182),
                    },
                ])
                .width(Size::fill()),
        )
}

#[test]
fn renders_remote_carets() {
    let (mut runner, ()) = TestingRunner::new(remote_app, (520., 260.).into(), |_| {}, 1.0);
    runner.sync_and_update();
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("remote_carets"));
}

fn table_caret_app() -> impl IntoElement {
    let value =
        use_state(|| "Intro\n\n| Nombre | Estado |\n|---|---|\n| Ana | lista |\n".to_owned());
    rect()
        .expanded()
        .background(EditorTheme::dark().colors.surface)
        .padding(20.)
        .child(
            MarkdownEditor::new(value)
                .remote_carets(vec![RemoteCaret {
                    position: TextPosition {
                        target: 4,
                        anchor: 0,
                        head: 3,
                    },
                    name: "Beto".to_owned(),
                    color: Color::from_rgb(244, 114, 182),
                }])
                .width(Size::fill()),
        )
}

#[test]
fn renders_remote_caret_in_a_table_cell() {
    let (mut runner, ()) = TestingRunner::new(table_caret_app, (520., 220.).into(), |_| {}, 1.0);
    runner.sync_and_update();
    runner.poll_n(std::time::Duration::from_millis(16), 4);
    runner.sync_and_update();
    runner.render_to_file(snapshot_path("remote_caret_table"));
}

use std::path::PathBuf;
use std::time::Duration;

use freya::prelude::*;
use freya_markdown_editor::document::{Atom, BlockKind, CellPos, TextTarget};
use freya_markdown_editor::prelude::*;
use freya_testing::prelude::*;

struct Harness {
    runner: TestingRunner,
    value: State<String>,
    controller: EditorController,
}

fn app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    let controller = use_consume::<EditorController>();
    rect()
        .expanded()
        .background((30, 32, 38))
        .padding(12.)
        .child(
            MarkdownEditor::new(value)
                .controller(controller)
                .slash_commands()
                .trigger(
                    Trigger::new('[', |query| {
                        vec![
                            Suggestion::new(
                                format!("Doc {query}"),
                                SuggestionAction::InsertEmbed {
                                    label: "Doc".into(),
                                    url: "app://doc".into(),
                                },
                            )
                            .detail("embed"),
                        ]
                    })
                    .anywhere(),
                )
                .decorator(SchemeEmbed::new("app", |embed| {
                    rect()
                        .height(Size::px(20.))
                        .padding((2., 8.))
                        .corner_radius(10.)
                        .background((60, 70, 110))
                        .child(label().font_size(12.).text(format!("◆ {}", embed.label)))
                        .into_element()
                }))
                .width(Size::fill()),
        )
}

impl Harness {
    fn new(markdown: &'static str) -> Self {
        let (runner, (value, controller)) = TestingRunner::new(
            app,
            (600., 400.).into(),
            move |runner| {
                let value = runner.provide_root_context(|| State::create(markdown.to_owned()));
                let controller =
                    runner.provide_root_context(|| EditorController::create(Document::default()));
                (value, controller)
            },
            1.0,
        );
        let mut harness = Self {
            runner,
            value,
            controller,
        };
        harness.settle();
        harness
    }

    fn settle(&mut self) {
        self.runner.poll_n(Duration::from_millis(1), 3);
        self.runner.sync_and_update();
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        self.runner.send_event(PlatformEvent::Keyboard {
            name: KeyboardEventName::KeyDown,
            key: key.clone(),
            code: Code::Unidentified,
            modifiers,
        });
        self.runner.sync_and_update();
        self.runner.send_event(PlatformEvent::Keyboard {
            name: KeyboardEventName::KeyUp,
            key,
            code: Code::Unidentified,
            modifiers,
        });
        self.settle();
    }

    fn named(&mut self, key: NamedKey) {
        self.key(Key::Named(key), Modifiers::empty());
    }

    fn command(&mut self, ch: &str) {
        self.key(Key::Character(ch.to_owned()), Modifiers::CONTROL);
    }

    fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            self.key(Key::Character(ch.to_string()), Modifiers::empty());
        }
    }

    fn focus(&mut self, target: TextTarget, offset: usize) {
        let controller = self.controller;
        self.runner.run_in(|| controller.focus(target, offset));
        self.settle();
    }

    fn block(&self, index: usize) -> TextTarget {
        let controller = self.controller;
        self.runner
            .run_in(|| TextTarget::from(controller.document().blocks()[index].id))
    }

    fn document(&self) -> Document {
        let controller = self.controller;
        self.runner.run_in(|| controller.document())
    }

    fn value(&self) -> String {
        self.value.peek().clone()
    }

    fn snapshot(&mut self, name: &str) {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
        self.runner.render_to_file(path);
    }
}

#[test]
fn shift_home_then_ctrl_b_bolds_the_line() {
    let mut harness = Harness::new("hello world");
    let first = harness.block(0);
    harness.focus(first, 11);
    harness.key(Key::Named(NamedKey::Home), Modifiers::SHIFT);
    harness.command("b");
    assert_eq!(harness.value(), "**hello world**\n");
}

#[test]
fn pending_mark_applies_to_next_typed_text() {
    let mut harness = Harness::new("plain");
    let first = harness.block(0);
    harness.focus(first, 5);
    harness.command("i");
    harness.type_text(" tilted");
    harness.command("i");
    harness.type_text(" back");
    assert_eq!(harness.value(), "plain *tilted* back\n");
}

#[test]
fn slash_menu_turns_block_into_quote() {
    let mut harness = Harness::new("");
    let first = harness.block(0);
    harness.focus(first, 0);
    harness.type_text("/quo");
    harness.snapshot("slash_menu");
    harness.named(NamedKey::Enter);
    harness.type_text("wise words");
    assert_eq!(harness.value(), "> wise words\n");
}

#[test]
fn backspace_at_start_merges_with_previous_block() {
    let mut harness = Harness::new("ab\n\ncd");
    let second = harness.block(1);
    harness.focus(second, 0);
    harness.named(NamedKey::Backspace);
    assert_eq!(harness.value(), "abcd\n");
    harness.type_text("X");
    assert_eq!(harness.value(), "abXcd\n");
}

#[test]
fn enter_in_list_continues_and_exits() {
    let mut harness = Harness::new("- one");
    let first = harness.block(0);
    harness.focus(first, 3);
    harness.named(NamedKey::Enter);
    harness.type_text("two");
    harness.named(NamedKey::Enter);
    harness.named(NamedKey::Enter);
    harness.type_text("after");
    assert_eq!(harness.value(), "- one\n- two\n\nafter\n");
}

#[test]
fn tab_walks_table_cells_and_grows_rows() {
    let mut harness = Harness::new("| a | b |\n|---|---|\n| c | d |");
    let table = harness.block(0).block;
    harness.focus(
        TextTarget {
            block: table,
            cell: Some(CellPos { row: 0, column: 0 }),
        },
        1,
    );
    harness.named(NamedKey::Tab);
    harness.type_text("x");
    harness.named(NamedKey::Tab);
    harness.named(NamedKey::Tab);
    harness.named(NamedKey::Tab);
    harness.type_text("new");
    let document = harness.document();
    let BlockKind::Table(table) = &document.blocks()[0].kind else {
        panic!("table expected");
    };
    assert_eq!(table.rows()[0][1].text(), "xb");
    assert_eq!(table.rows().len(), 3);
    assert_eq!(table.rows()[2][0].text(), "new");
}

#[test]
fn read_only_ignores_typing() {
    let mut harness = Harness::new("locked");
    let controller = harness.controller;
    harness.runner.run_in(|| controller.set_read_only(true));
    harness.settle();
    let first = harness.block(0);
    harness.focus(first, 6);
    harness.type_text("!!");
    assert_eq!(harness.value(), "locked");
    harness.snapshot("read_only");
}

#[test]
fn undo_reverts_a_typing_burst() {
    let mut harness = Harness::new("start");
    let first = harness.block(0);
    harness.focus(first, 5);
    harness.type_text(" more");
    assert_eq!(harness.value(), "start more\n");
    harness.command("z");
    assert_eq!(harness.value(), "start\n");
    harness.command("y");
    assert_eq!(harness.value(), "start more\n");
}

#[test]
fn trigger_inserts_embed_that_round_trips() {
    let mut harness = Harness::new("see");
    let first = harness.block(0);
    harness.focus(first, 3);
    harness.type_text(" [do");
    harness.named(NamedKey::Enter);
    assert_eq!(harness.value(), "see [Doc](app://doc)\n");
    let document = harness.document();
    let BlockKind::Paragraph(content) = &document.blocks()[0].kind else {
        panic!("paragraph expected");
    };
    assert!(matches!(content.atoms(), [Atom::Embed { .. }]));
    harness.snapshot("embed");
}

#[test]
fn heading_shortcut_and_toolbar_api() {
    let mut harness = Harness::new("");
    let first = harness.block(0);
    harness.focus(first, 0);
    harness.type_text("## Title");
    let controller = harness.controller;
    harness
        .runner
        .run_in(|| controller.set_align(Align::Center));
    harness.settle();
    assert_eq!(
        harness.value(),
        "<div align=\"center\">\n\n## Title\n\n</div>\n"
    );
}

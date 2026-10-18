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

#[derive(Clone, Copy)]
struct RevealSyntax(bool);

fn app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    let controller = use_consume::<EditorController>();
    let reveal = use_consume::<RevealSyntax>().0;
    rect()
        .expanded()
        .background((30, 32, 38))
        .padding(12.)
        .child(
            MarkdownEditor::new(value)
                .controller(controller)
                .syntax_reveal(reveal)
                .slash_menu()
                .suggestions(
                    SuggestionGroup::new("Docs", |query| {
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
                    .keyword("doc"),
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
        Self::with_reveal(markdown, false)
    }

    fn with_reveal(markdown: &'static str, reveal: bool) -> Self {
        let (runner, (value, controller)) = TestingRunner::new(
            app,
            (600., 400.).into(),
            move |runner| {
                runner.provide_root_context(|| RevealSyntax(reveal));
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
fn suggestion_source_inserts_embed_that_round_trips() {
    let mut harness = Harness::new("see");
    let first = harness.block(0);
    harness.focus(first, 3);
    harness.type_text(" /doc");
    harness.snapshot("slash_sections");
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

#[test]
fn double_enter_at_the_end_leaves_a_code_block() {
    let mut harness = Harness::new("```\nlet x = 1;\n```");
    let code = harness.block(0);
    harness.focus(code, 10);
    harness.named(NamedKey::Enter);
    harness.type_text("x");
    harness.named(NamedKey::Enter);
    harness.named(NamedKey::Enter);
    harness.type_text("after");
    assert_eq!(harness.value(), "```\nlet x = 1;\nx\n```\n\nafter\n");
}

#[test]
fn code_block_at_the_end_always_has_a_way_out() {
    let mut harness = Harness::new("```\ncode\n```");
    assert_eq!(harness.document().blocks().len(), 2);
    let paragraph = harness.block(1);
    harness.focus(paragraph, 0);
    harness.type_text("next");
    assert_eq!(harness.value(), "```\ncode\n```\n\nnext\n");
}

#[test]
fn ctrl_shift_arrows_move_blocks() {
    let mut harness = Harness::new("one\n\ntwo\n\nthree");
    let first = harness.block(0);
    harness.focus(first, 0);
    harness.key(
        Key::Named(NamedKey::ArrowDown),
        Modifiers::CONTROL | Modifiers::SHIFT,
    );
    assert_eq!(harness.value(), "two\n\none\n\nthree\n");
    let controller = harness.controller;
    let ids: Vec<_> = harness.runner.run_in(|| {
        controller
            .document()
            .blocks()
            .iter()
            .map(|b| b.id)
            .collect()
    });
    harness
        .runner
        .run_in(|| controller.move_block_onto(ids[2], ids[0]));
    harness.settle();
    assert_eq!(harness.value(), "three\n\ntwo\n\none\n");
}

#[test]
fn deleting_back_to_a_mark_end_does_not_resume_it() {
    let mut harness = Harness::new("");
    let first = harness.block(0);
    harness.focus(first, 0);
    harness.type_text("**texto** ab");
    assert_eq!(harness.value(), "**texto** ab\n");
    for _ in 0..3 {
        harness.named(NamedKey::Backspace);
    }
    harness.type_text("x");
    assert_eq!(harness.value(), "**texto**x\n");
}

#[test]
fn arrow_right_at_the_end_leaves_the_mark() {
    let mut harness = Harness::new("**bold**");
    let first = harness.block(0);
    harness.focus(first, 4);
    harness.type_text("er");
    assert_eq!(harness.value(), "**bolder**\n");
    harness.named(NamedKey::ArrowRight);
    harness.type_text(" plain");
    assert_eq!(harness.value(), "**bolder** plain\n");
}

#[test]
fn revealed_source_is_edited_as_markdown() {
    let mut harness = Harness::with_reveal("say **hi**", true);
    let first = harness.block(0);
    harness.focus(first, 6);
    harness.snapshot("syntax_reveal");
    harness.type_text("!");
    assert_eq!(harness.value(), "say **hi**!\n");
    harness.named(NamedKey::Backspace);
    harness.named(NamedKey::Backspace);
    assert_eq!(harness.value(), "say \\**hi*\n");
    harness.type_text("*");
    assert_eq!(harness.value(), "say **hi**\n");
}

#[test]
fn revealed_source_closes_when_the_caret_leaves() {
    let mut harness = Harness::with_reveal("say **hi** there", true);
    let first = harness.block(0);
    harness.focus(first, 6);
    harness.named(NamedKey::End);
    harness.type_text("!");
    assert_eq!(harness.value(), "say **hi** there!\n");
    harness.focus(first, 0);
    harness.type_text("x");
    assert_eq!(harness.value(), "xsay **hi** there!\n");
}

#[test]
fn shortcut_types_delimiters_when_syntax_is_shown() {
    let mut harness = Harness::with_reveal("say", true);
    let first = harness.block(0);
    harness.focus(first, 3);
    harness.type_text(" ");
    harness.command("b");
    assert_eq!(harness.value(), "say \n");
    harness.snapshot("syntax_reveal_pair");
    harness.type_text("hi");
    assert_eq!(harness.value(), "say **hi**\n");
}

#[test]
fn shortcut_twice_removes_the_empty_pair() {
    let mut harness = Harness::with_reveal("say", true);
    let first = harness.block(0);
    harness.focus(first, 3);
    harness.type_text(" ");
    harness.command("i");
    harness.command("i");
    harness.type_text("x");
    assert_eq!(harness.value(), "say x\n");
}

#[test]
fn deleting_the_content_back_to_an_empty_pair_leaves_no_text() {
    let mut harness = Harness::with_reveal("say", true);
    let first = harness.block(0);
    harness.focus(first, 3);
    harness.type_text(" ");
    harness.command("b");
    harness.type_text("a");
    assert_eq!(harness.value(), "say **a**\n");
    harness.named(NamedKey::Backspace);
    assert_eq!(harness.value(), "say \n");
    harness.type_text("b");
    assert_eq!(harness.value(), "say **b**\n");
}

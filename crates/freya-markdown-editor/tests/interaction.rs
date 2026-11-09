use std::path::PathBuf;
use std::time::Duration;

use freya::prelude::*;
use freya_core::elements::paragraph::ParagraphElement;
use freya_markdown_editor::document::{Atom, BlockKind, CellPos, TextTarget};
use freya_markdown_editor::prelude::*;
use freya_testing::TestingNode;
use freya_testing::prelude::*;

struct Harness {
    runner: TestingRunner,
    value: State<String>,
    controller: EditorController,
}

#[derive(Clone, Copy)]
struct RevealSyntax(bool);

/// Touches that reached the page holding the editor, like a canvas does.
#[derive(Clone, Copy)]
struct TouchesBelow(State<u32>);

fn app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    let controller = use_consume::<EditorController>();
    let reveal = use_consume::<RevealSyntax>().0;
    let mut below = use_consume::<TouchesBelow>().0;
    rect()
        .expanded()
        .on_touch_start(move |_| *below.write() += 1)
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
                .suggestions(
                    SuggestionGroup::new("Actions", |_| {
                        vec![Suggestion::new(
                            "Stamp",
                            SuggestionAction::Run(SuggestionRun::new(|controller| {
                                controller.insert_text("stamped");
                            })),
                        )]
                    })
                    .keyword("run"),
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
                runner.provide_root_context(|| TouchesBelow(State::create(0)));
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
fn a_transclusion_pasted_mid_paragraph_splits_it_and_keeps_typing_after() {
    let mut harness = Harness::new("antesdespués");
    let first = harness.block(0);
    harness.focus(first, 5);
    let controller = harness.controller;
    harness.runner.run_in(|| {
        controller.insert_markdown("> [!app:live]\n>\n> texto\n>\n> — [Fuente](app://x)");
    });
    harness.settle();
    harness.type_text("!");
    assert_eq!(
        harness.value(),
        "antes\n\n> [!app:live]\n>\n> texto\n>\n> — [Fuente](app://x)\n\n!después\n"
    );
}

#[test]
fn a_transclusion_pasted_at_a_line_start_goes_before_it() {
    let mut harness = Harness::new("arriba\n\nabajo");
    let empty = harness.block(1);
    harness.focus(empty, 0);
    let controller = harness.controller;
    harness.runner.run_in(|| {
        controller.insert_markdown("> [!app:live]\n>\n> texto\n>\n> — [Fuente](app://x)");
    });
    harness.settle();
    assert_eq!(
        harness.value(),
        "arriba\n\n> [!app:live]\n>\n> texto\n>\n> — [Fuente](app://x)\n\nabajo\n"
    );
}

#[test]
fn backspace_after_a_transclusion_removes_it_whole() {
    let mut harness =
        Harness::new("antes\n\n> [!app:live]\n>\n> texto\n>\n> — [Fuente](app://x)\n\ndespués");
    assert!(matches!(
        harness.document().blocks()[1].kind,
        BlockKind::Transclusion(_)
    ));
    harness.snapshot("transclusion");
    let after = harness.block(2);
    harness.focus(after, 0);
    harness.named(NamedKey::Backspace);
    assert_eq!(harness.value(), "antes\n\ndespués\n");
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
fn read_only_press_neither_focuses_nor_selects() {
    let mut harness = Harness::new("locked text");
    let controller = harness.controller;
    harness.runner.run_in(|| controller.set_read_only(true));
    harness.settle();
    harness.runner.click_cursor((40., 22.));
    harness.settle();
    assert!(!harness.runner.run_in(|| controller.has_focus()));
    assert_eq!(harness.runner.run_in(|| controller.selection()), None);
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

#[test]
fn block_offsets_follow_the_layout() {
    let harness = Harness::new("# One\n\ntext\n\n# Two");
    let controller = harness.controller;
    let (first, last) = (harness.block(0).block, harness.block(2).block);
    let offsets = harness.runner.run_in(|| {
        (
            controller.block_offset(first),
            controller.block_offset(last),
        )
    });
    let (Some(first), Some(last)) = offsets else {
        panic!("blocks were laid out: {offsets:?}");
    };
    assert!(first < last, "{first} < {last}");
}

#[test]
fn run_suggestion_drives_the_controller() {
    let mut harness = Harness::new("");
    let first = harness.block(0);
    harness.focus(first, 0);
    harness.type_text("/run st");
    harness.named(NamedKey::Enter);
    assert_eq!(harness.value(), "stamped\n");
}

#[test]
fn soft_keyboard_line_break_splits_the_block() {
    let mut harness = Harness::new("one");
    let first = harness.block(0);
    harness.focus(first, 3);
    harness.type_text("\n");
    harness.type_text("two");
    assert_eq!(harness.value(), "one\n\ntwo\n");
}

#[test]
fn focus_and_block_bounds_are_tracked() {
    let mut harness = Harness::new("one\n\ntwo");
    let controller = harness.controller;
    assert!(!harness.runner.run_in(|| controller.has_focus()));
    let second = harness.block(1);
    harness.focus(second, 0);
    assert!(harness.runner.run_in(|| controller.has_focus()));
    let bounds = harness
        .runner
        .run_in(|| controller.block_bounds(second.block));
    let Some((top, bottom)) = bounds else {
        panic!("block was laid out");
    };
    assert!(top < bottom, "{top} < {bottom}");
    let caret = harness.runner.run_in(|| controller.caret_bounds());
    let Some((caret_top, caret_bottom)) = caret else {
        panic!("caret was measured");
    };
    assert!(
        top <= caret_top && caret_bottom <= bottom + 1.,
        "{caret_top}..{caret_bottom} in {top}..{bottom}"
    );
}

#[test]
fn soft_keyboard_paste_imports_markdown_blocks() {
    let mut harness = Harness::new("intro");
    let first = harness.block(0);
    harness.focus(first, 5);
    harness.named(NamedKey::Enter);
    harness.key(
        Key::Character("# Title\n\n- one\n- **two**".to_owned()),
        Modifiers::empty(),
    );
    harness.type_text("!");
    assert_eq!(harness.value(), "intro\n\n# Title\n\n- one\n- **two!**\n");
}

#[test]
fn pasted_markdown_splits_around_the_caret() {
    let mut harness = Harness::new("before after");
    let first = harness.block(0);
    harness.focus(first, 7);
    harness.key(
        Key::Character("## Mid\n\ntext".to_owned()),
        Modifiers::empty(),
    );
    assert_eq!(harness.value(), "before \n\n## Mid\n\ntext\n\nafter\n");
}

fn tap(harness: &mut Harness, at: (f64, f64)) {
    for (name, phase) in [
        (TouchEventName::TouchStart, TouchPhase::Started),
        (TouchEventName::TouchEnd, TouchPhase::Ended),
    ] {
        harness.runner.send_event(PlatformEvent::Touch {
            name,
            location: at.into(),
            finger_id: 0,
            phase,
            force: None,
        });
        harness.runner.sync_and_update();
    }
    harness.settle();
}

#[test]
fn a_tap_on_the_quote_converter_reaches_it() {
    let mut harness = Harness::new("> quoted\n\nafter\n");
    let quote = harness.block(0);
    harness.focus(quote, 2);
    tap(&mut harness, (60., 31.));
    tap(&mut harness, (80., 73.));
    assert!(matches!(
        harness.document().blocks()[0].kind,
        BlockKind::Callout { .. }
    ));
}

#[test]
fn folding_details_with_the_caret_inside_moves_it_out() {
    let mut harness =
        Harness::new("before\n\n<details>\n<summary>Hola</summary>\n\nhidden\n\n</details>\n");
    let details = harness.block(1);
    harness.focus(details, 2);
    tap(&mut harness, (40., 63.));
    let controller = harness.controller;
    let selection = harness.runner.run_in(|| controller.selection());
    assert_eq!(
        selection.map(|selection| selection.target),
        Some(harness.block(0))
    );
}

/// Draws every formula as a plain box.
struct DrawnMath;

impl Decorator for DrawnMath {
    fn render_math(&self, _math: &MathContext<'_>) -> Option<Element> {
        Some(
            rect()
                .width(Size::px(120.))
                .height(Size::px(40.))
                .background((200, 80, 80))
                .into_element(),
        )
    }
}

#[derive(Clone, Copy)]
struct KeyboardAsks(State<u32>);

fn drawn_math_app() -> impl IntoElement {
    let value = use_consume::<State<String>>();
    let controller = use_consume::<EditorController>();
    let asks = use_consume::<KeyboardAsks>().0;
    rect().expanded().padding(12.).child(
        MarkdownEditor::new(value)
            .controller(controller)
            .decorator(DrawnMath)
            .on_request_keyboard(move || {
                let mut asks = asks;
                *asks.write() += 1;
            })
            .width(Size::fill()),
    )
}

#[test]
fn edit_opens_the_source_of_a_drawing_and_done_folds_it_again() {
    let (runner, (value, controller, asks)) = TestingRunner::new(
        drawn_math_app,
        (600., 400.).into(),
        move |runner| {
            let value =
                runner.provide_root_context(|| State::create("$$\nx^2\n$$\n\nafter\n".to_owned()));
            let controller =
                runner.provide_root_context(|| EditorController::create(Document::default()));
            let asks = runner.provide_root_context(|| KeyboardAsks(State::create(0)));
            (value, controller, asks)
        },
        1.0,
    );
    let mut harness = Harness {
        runner,
        value,
        controller,
    };
    harness.settle();
    tap(&mut harness, (560., 30.));
    let formula = harness.block(0);
    let selection = harness.runner.run_in(|| controller.selection());
    assert_eq!(selection.map(|selection| selection.target), Some(formula));
    assert_eq!(*asks.0.peek(), 1);
    tap(&mut harness, (560., 30.));
    let selection = harness.runner.run_in(|| controller.selection());
    assert_eq!(
        selection.map(|selection| selection.target),
        Some(harness.block(1))
    );
}

#[test]
fn a_tap_on_a_suggestion_picks_it_and_stays_in_the_menu() {
    let mut harness = Harness::new("see");
    let first = harness.block(0);
    harness.focus(first, 3);
    harness.type_text(" /doc");
    let below = harness
        .runner
        .run_in(try_consume_root_context::<TouchesBelow>)
        .map(|touches| touches.0);
    tap(&mut harness, (100., 65.));
    assert_eq!(harness.value(), "see [Doc](app://doc)\n");
    assert_eq!(below.map(|touches| *touches.peek()), Some(0));
}

/// Centre of the paragraph showing `text`, in window coordinates.
fn text_center(harness: &Harness, text: &str) -> (f64, f64) {
    harness
        .runner
        .find(|node, element| {
            let paragraph = (element as &dyn std::any::Any).downcast_ref::<ParagraphElement>()?;
            if !paragraph.to_string().contains(text) {
                return None;
            }
            let area = node.layout().area;
            Some((
                f64::from(area.min_x() + area.width() / 2.),
                f64::from(area.min_y() + area.height() / 2.),
            ))
        })
        .unwrap_or_else(|| panic!("no paragraph shows {text:?}"))
}

#[test]
fn a_table_cell_takes_the_focus_back_after_a_click_elsewhere() {
    let mut harness = Harness::new("| alpha | beta |\n|---|---|\n| gamma | delta |\n\nafter\n");
    let table = harness.block(0).block;
    harness.focus(
        TextTarget {
            block: table,
            cell: Some(CellPos { row: 1, column: 0 }),
        },
        5,
    );
    harness.type_text("1");
    // Leave the table for the paragraph below it.
    let after = text_center(&harness, "after");
    harness.runner.click_cursor(after);
    harness.settle();
    harness.type_text("2");
    // Back into the same cell with the mouse.
    let cell = text_center(&harness, "gamma");
    harness.runner.click_cursor(cell);
    harness.settle();
    harness.type_text("3");
    let document = harness.document();
    let BlockKind::Table(table) = &document.blocks()[0].kind else {
        panic!("table expected");
    };
    assert_eq!(table.rows()[1][0].text(), "gamma13");
    assert_eq!(
        document.blocks()[1].kind.content().map(Inline::text),
        Some("after2")
    );
}

#[test]
fn a_table_cell_takes_the_focus_back_after_a_tap_elsewhere() {
    let mut harness = Harness::new("| alpha | beta |\n|---|---|\n| gamma | delta |\n\nafter\n");
    let table = harness.block(0).block;
    harness.focus(
        TextTarget {
            block: table,
            cell: Some(CellPos { row: 1, column: 0 }),
        },
        5,
    );
    harness.type_text("1");
    let after = text_center(&harness, "after");
    tap(&mut harness, after);
    harness.type_text("2");
    let cell = text_center(&harness, "gamma");
    tap(&mut harness, cell);
    harness.type_text("3");
    let document = harness.document();
    let BlockKind::Table(table) = &document.blocks()[0].kind else {
        panic!("table expected");
    };
    assert_eq!(table.rows()[1][0].text(), "gamma13");
    assert_eq!(
        document.blocks()[1].kind.content().map(Inline::text),
        Some("after2")
    );
}

#[test]
fn empty_table_cells_keep_a_line_of_height() {
    let mut harness = Harness::new("| a | b |\n|---|---|\n|  |  |\n\nafter\n");
    harness.settle();
    let heights: Vec<(String, f32)> = harness.runner.find_many(|node, element| {
        let paragraph = (element as &dyn std::any::Any).downcast_ref::<ParagraphElement>()?;
        Some((paragraph.to_string(), node.layout().area.height()))
    });
    eprintln!("{heights:?}");
    let empty: Vec<f32> = heights
        .iter()
        .filter(|(text, _)| text.is_empty())
        .map(|(_, height)| *height)
        .collect();
    assert!(
        empty.iter().all(|h| *h > 10.),
        "empty cells collapsed: {empty:?}"
    );
}

/// Centre of the `index`th paragraph showing nothing, in window coordinates.
fn empty_paragraph_center(harness: &Harness, index: usize) -> (f64, f64) {
    let centers = harness.runner.find_many(|node, element| {
        let paragraph = (element as &dyn std::any::Any).downcast_ref::<ParagraphElement>()?;
        if !paragraph.to_string().is_empty() {
            return None;
        }
        let area = node.layout().area;
        Some((
            f64::from(area.min_x() + area.width() / 2.),
            f64::from(area.min_y() + area.height() / 2.),
        ))
    });
    centers[index]
}

#[test]
fn a_tap_on_an_empty_table_cell_types_into_it() {
    let mut harness = Harness::new("| a | b |\n|---|---|\n|  |  |\n\nafter\n");
    let after = text_center(&harness, "after");
    tap(&mut harness, after);
    harness.type_text("2");
    let cell = empty_paragraph_center(&harness, 0);
    tap(&mut harness, cell);
    harness.type_text("3");
    let document = harness.document();
    let BlockKind::Table(table) = &document.blocks()[0].kind else {
        panic!("table expected");
    };
    assert_eq!(
        document.blocks()[1].kind.content().map(Inline::text),
        Some("after2")
    );
    assert_eq!(table.rows()[1][0].text(), "3");
}

#[test]
fn a_click_on_an_empty_table_cell_types_into_it() {
    let mut harness = Harness::new("| a | b |\n|---|---|\n|  |  |\n\nafter\n");
    let after = text_center(&harness, "after");
    harness.runner.click_cursor(after);
    harness.settle();
    harness.type_text("2");
    let cell = empty_paragraph_center(&harness, 0);
    harness.runner.click_cursor(cell);
    harness.settle();
    harness.type_text("3");
    let document = harness.document();
    let BlockKind::Table(table) = &document.blocks()[0].kind else {
        panic!("table expected");
    };
    assert_eq!(
        document.blocks()[1].kind.content().map(Inline::text),
        Some("after2")
    );
    assert_eq!(table.rows()[1][0].text(), "3");
}

/// Height of the table cell holding the paragraph showing `text`: the
/// rect three levels above it (cell, text block, focusable area).
fn cell_height(harness: &Harness, text: &str) -> f32 {
    fn holds(node: &TestingNode, text: &str, depth: usize) -> bool {
        let children = node.children();
        if depth == 0 {
            return children.is_empty()
                && (node.element().as_ref() as &dyn std::any::Any)
                    .downcast_ref::<ParagraphElement>()
                    .is_some_and(|paragraph| paragraph.to_string().contains(text));
        }
        children.iter().any(|child| holds(child, text, depth - 1))
    }
    harness
        .runner
        .find(|node, _| holds(&node, text, 3).then(|| node.layout().area.height()))
        .unwrap_or_else(|| panic!("no cell shows {text:?}"))
}

#[test]
fn table_rows_take_the_height_of_their_tallest_cell() {
    let mut harness = Harness::new(
        "| a | b |\n|---|---|\n| short | one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen |\n",
    );
    harness.settle();
    let short = cell_height(&harness, "short");
    let long = cell_height(&harness, "eleven");
    assert!(long > 30., "the long cell did not wrap: {long}");
    assert!((short - long).abs() < 0.5, "short {short} vs long {long}");
}

const SHOPPING: &str = "\
| ✓ | Artículo | Presupuesto | Costo |
|:-:|---|--:|--:|
| [x] | Pan | 10 | 12,50 |
| [ ] | Leche | 5 | |

<!--
notein:table
check: 1
column: Diferencia = [Costo] - [Presupuesto]
calc: Costo = sum, checked
-->
";

fn shopping_table(harness: &Harness) -> freya_markdown_editor::document::Table {
    let document = harness.document();
    let BlockKind::Table(table) = &document.blocks()[0].kind else {
        panic!("table expected");
    };
    table.clone()
}

#[test]
fn a_tap_on_a_check_mark_ticks_the_row_and_the_total_follows() {
    let mut harness = Harness::new(SHOPPING);
    assert!(!shopping_table(&harness).is_checked(2));
    let (_, y) = text_center(&harness, "Leche");
    // The check column is the first of five equal columns across the editor.
    let x = 12. + (600. - 24.) / 5. / 2.;
    tap(&mut harness, (x, y));
    let table = shopping_table(&harness);
    assert!(table.is_checked(2));
    let totals = table.evaluate().totals;
    assert_eq!(totals[3].as_ref().map(|t| t.text.as_str()), Some("12,50"));
    assert!(
        harness.value().contains("| [x] | Leche"),
        "{}",
        harness.value()
    );
    assert!(harness.value().contains("Sum 12,50"), "{}", harness.value());
}

#[test]
fn tab_skips_check_marks_and_computed_cells() {
    let mut harness = Harness::new(SHOPPING);
    let table = harness.block(0).block;
    harness.focus(
        TextTarget {
            block: table,
            cell: Some(CellPos { row: 0, column: 3 }),
        },
        0,
    );
    harness.named(NamedKey::Tab);
    harness.type_text("x");
    let table = shopping_table(&harness);
    assert_eq!(table.rows()[1][1].text(), "xPan");
    harness.named(NamedKey::Tab);
    harness.named(NamedKey::Tab);
    harness.named(NamedKey::Tab);
    harness.named(NamedKey::Tab);
    harness.named(NamedKey::Tab);
    harness.named(NamedKey::Tab);
    harness.type_text("new");
    let table = shopping_table(&harness);
    assert_eq!(table.rows().len(), 4);
    assert_eq!(table.rows()[3][0].text(), "[ ]");
    assert_eq!(table.rows()[3][1].text(), "new");
}

#[test]
fn renaming_a_header_keeps_the_formula_pointing_at_it() {
    let mut harness = Harness::new(SHOPPING);
    let table = harness.block(0).block;
    harness.focus(
        TextTarget {
            block: table,
            cell: Some(CellPos { row: 0, column: 3 }),
        },
        5,
    );
    harness.type_text("s");
    let table = shopping_table(&harness);
    assert_eq!(table.header(3), "Costos");
    assert_eq!(table.computed()[0].formula, "[Costos] - [Presupuesto]");
    assert!(
        harness.value().contains("calc: Costos = sum, checked"),
        "{}",
        harness.value()
    );
}

#[test]
fn arithmetic_at_the_end_of_a_line_offers_its_result() {
    let mut harness = Harness::new("Total:");
    let first = harness.block(0);
    harness.focus(first, 6);
    harness.type_text(" 123 + 87");
    // The first entry replaces the expression by its result.
    harness.named(NamedKey::ArrowDown);
    harness.named(NamedKey::ArrowUp);
    harness.named(NamedKey::Enter);
    assert_eq!(harness.value(), "Total: 210\n");
}

#[test]
fn enter_keeps_breaking_the_line_while_a_calculation_is_offered() {
    let mut harness = Harness::new("Total:");
    let first = harness.block(0);
    harness.focus(first, 6);
    harness.type_text(" 123 + 87");
    harness.named(NamedKey::Enter);
    harness.type_text("next");
    assert_eq!(harness.value(), "Total: 123 + 87\n\nnext\n");
}

#[test]
fn the_second_calculator_entry_appends_the_result() {
    let mut harness = Harness::new("");
    let first = harness.block(0);
    harness.focus(first, 0);
    harness.type_text("(2,5 + 1) * 2");
    harness.named(NamedKey::ArrowDown);
    harness.named(NamedKey::Tab);
    assert_eq!(harness.value(), "(2,5 + 1) \\* 2 = 7,0\n");
    harness.type_text(" ok");
    assert_eq!(harness.value(), "(2,5 + 1) \\* 2 = 7,0 ok\n");
}

#[test]
fn escape_dismisses_the_calculator_until_the_expression_changes() {
    let mut harness = Harness::new("");
    let first = harness.block(0);
    harness.focus(first, 0);
    harness.type_text("1 + 2");
    harness.named(NamedKey::Escape);
    harness.named(NamedKey::ArrowDown);
    harness.named(NamedKey::Enter);
    assert_eq!(harness.value(), "1 + 2\n");
    harness.named(NamedKey::Backspace);
    harness.type_text("3");
    harness.named(NamedKey::ArrowDown);
    harness.named(NamedKey::ArrowUp);
    harness.named(NamedKey::Enter);
    assert_eq!(harness.value(), "24\n");
}

#[test]
fn code_blocks_do_not_offer_calculations() {
    let mut harness = Harness::new("```\nx\n```\n");
    let code = harness.block(0);
    harness.focus(code, 1);
    harness.type_text(" + 1 + 2");
    harness.named(NamedKey::ArrowDown);
    harness.named(NamedKey::Enter);
    assert_eq!(harness.value(), "```\nx + 1 + 2\n```\n");
}

/// Another device writing the document while this one has text selected
/// must not lose the selection the next toolbar press acts on.
#[test]
fn a_remote_write_keeps_the_selection_a_mark_applies_to() {
    let mut harness = Harness::new("hello world\n\nsecond\n");
    let first = harness.block(0);
    harness.focus(first, 11);
    harness.key(Key::Named(NamedKey::Home), Modifiers::SHIFT);
    let mut value = harness.value;
    harness
        .runner
        .run_in(|| *value.write() = "hello world\n\nsecond line\n".to_owned());
    harness.settle();
    let controller = harness.controller;
    harness
        .runner
        .run_in(|| controller.toggle_mark(MarkKind::Highlight));
    harness.settle();
    assert_eq!(harness.value(), "==hello world==\n\nsecond line\n");
}

/// A block the other device adds above the selection does not move it to
/// another paragraph.
#[test]
fn a_remote_block_above_keeps_the_selection_on_its_text() {
    let mut harness = Harness::new("hello world\n\nsecond\n");
    let first = harness.block(0);
    harness.focus(first, 11);
    harness.key(Key::Named(NamedKey::Home), Modifiers::SHIFT);
    let mut value = harness.value;
    harness
        .runner
        .run_in(|| *value.write() = "intro\n\nhello world\n\nsecond\n".to_owned());
    harness.settle();
    let controller = harness.controller;
    harness
        .runner
        .run_in(|| controller.toggle_mark(MarkKind::Highlight));
    harness.settle();
    assert_eq!(harness.value(), "intro\n\n==hello world==\n\nsecond\n");
}

//! A read only editor inside something pressable (a quote card): taps on
//! its text reach the card, taps on its links stay on the link.

use std::time::Duration;

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use freya_testing::prelude::*;

#[derive(Clone, Copy)]
struct Presses {
    card: State<u32>,
    links: State<u32>,
}

fn app() -> impl IntoElement {
    let Presses { mut card, links } = use_consume::<Presses>();
    let value = use_state(|| "plain words here\n\n[a link](app://target)".to_owned());
    let controller = use_editor_controller();
    rect()
        .expanded()
        .padding(10.)
        .on_press(move |_| *card.write() += 1)
        .child(
            MarkdownEditor::new(value)
                .controller(controller)
                .read_only(true)
                .on_link(move |_| {
                    let mut links = links;
                    *links.write() += 1;
                })
                .width(Size::fill()),
        )
}

fn harness() -> (TestingRunner, Presses) {
    let (mut runner, presses) = TestingRunner::new(
        app,
        (400., 300.).into(),
        |runner| {
            runner.provide_root_context(|| Presses {
                card: State::create(0),
                links: State::create(0),
            })
        },
        1.0,
    );
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
    (runner, presses)
}

#[test]
fn a_tap_on_read_only_text_reaches_the_card() {
    let (mut runner, presses) = harness();
    runner.click_cursor((40., 22.));
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
    assert_eq!(*presses.card.peek(), 1);
}

#[test]
fn a_tap_on_a_link_opens_the_link() {
    let (mut runner, presses) = harness();
    let link = runner
        .find(|node, element| {
            Paragraph::try_downcast(element)
                .filter(|p| p.spans.iter().any(|s| s.text.contains("a link")))
                .map(|_| node.layout().area)
        })
        .expect("link paragraph");
    let at = (f64::from(link.min_x() + 10.), f64::from(link.center().y));
    runner.click_cursor(at);
    runner.poll_n(Duration::from_millis(1), 3);
    runner.sync_and_update();
    assert_eq!(*presses.links.peek(), 1);
    assert_eq!(*presses.card.peek(), 0, "the link keeps its tap");
}

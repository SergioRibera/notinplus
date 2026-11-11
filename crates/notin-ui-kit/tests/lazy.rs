use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use freya::prelude::*;
use freya_testing::prelude::*;
use notin_ui_kit::Lazy;

/// How many probes have been built so far, and how many dropped.
#[derive(Clone, Default)]
struct Built(Rc<Cell<usize>>, Rc<Cell<usize>>);

impl Built {
    fn alive(&self) -> usize {
        self.0.get() - self.1.get()
    }
}

/// Counts itself once when mounted and once when dropped.
#[derive(PartialEq)]
struct Probe;

impl Component for Probe {
    fn render(&self) -> impl IntoElement {
        let built = use_consume::<Built>();
        let dropped = built.clone();
        use_hook(move || built.0.set(built.0.get() + 1));
        use_drop(move || dropped.1.set(dropped.1.get() + 1));
        rect().width(Size::fill()).height(Size::px(100.))
    }
}

const ITEMS: usize = 30;

fn list(overscan: f32, pinned: Option<usize>, recycle: bool) -> Element {
    ScrollView::new()
        .width(Size::fill())
        .height(Size::px(300.))
        .children((0..ITEMS).map(|i| {
            Lazy::new(Probe)
                .key(i)
                .height_hint(100.)
                .overscan(overscan)
                .pinned(pinned == Some(i))
                .recycle(recycle)
                .into_element()
        }))
        .into_element()
}

fn viewport_only() -> Element {
    list(0., None, false)
}

fn with_overscan() -> Element {
    list(250., None, false)
}

fn with_pinned() -> Element {
    list(0., Some(ITEMS - 1), false)
}

fn unclipped() -> Element {
    rect()
        .width(Size::fill())
        .children((0..ITEMS).map(|i| Lazy::new(Probe).key(i).height_hint(100.).into_element()))
        .into_element()
}

fn run(app: fn() -> Element) -> (TestingRunner, Built) {
    let (mut runner, built) = TestingRunner::new(
        app,
        (400., 300.).into(),
        |runner| {
            let built = Built::default();
            let shared = built.clone();
            runner.provide_root_context(move || shared);
            built
        },
        1.0,
    );
    runner.poll_n(Duration::from_millis(1), 4);
    runner.sync_and_update();
    (runner, built)
}

#[test]
fn only_what_the_viewport_shows_is_built() {
    let (_runner, built) = run(viewport_only);
    let count = built.0.get();
    assert!((3..=4).contains(&count), "built {count} of {ITEMS}");
}

#[test]
fn overscan_builds_a_little_ahead() {
    let (_runner, built) = run(with_overscan);
    let count = built.0.get();
    assert!((5..=7).contains(&count), "built {count} of {ITEMS}");
}

#[test]
fn scrolling_builds_what_comes_into_view() {
    let (mut runner, built) = run(viewport_only);
    let before = built.0.get();
    for _ in 0..5 {
        runner.scroll((200., 150.), (0., -300.));
        runner.poll_n(Duration::from_millis(1), 4);
        runner.sync_and_update();
    }
    let count = built.0.get();
    assert!(
        count > before && count < ITEMS,
        "built {count} of {ITEMS}, {before} before scrolling"
    );
}

#[test]
fn a_pinned_child_is_built_out_of_view() {
    let (_runner, built) = run(with_pinned);
    let count = built.0.get();
    assert!((4..=5).contains(&count), "built {count} of {ITEMS}");
}

#[test]
fn nothing_clipping_means_everything_is_built() {
    let (_runner, built) = run(unclipped);
    assert_eq!(built.0.get(), ITEMS);
}

fn recycling() -> Element {
    list(0., None, true)
}

#[test]
fn recycling_drops_what_scrolled_away() {
    let (mut runner, built) = run(recycling);
    for _ in 0..5 {
        runner.scroll((200., 150.), (0., -300.));
        runner.poll_n(Duration::from_millis(1), 4);
        runner.sync_and_update();
    }
    let built_total = built.0.get();
    let alive = built.alive();
    assert!(built_total > 4, "built {built_total}");
    assert!(
        (3..=4).contains(&alive),
        "alive {alive} of {built_total} built"
    );
}

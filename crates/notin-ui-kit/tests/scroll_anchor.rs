//! Content changing height above the view never moves what is in view,
//! not even for one frame.

use std::time::Duration;

use freya::prelude::*;
use freya_testing::prelude::*;
use notin_ui_kit::{Lazy, use_scroll_anchor};

const ITEMS: usize = 20;
const MARKED: usize = 10;
/// Width only the marker has, to find it in the layout.
const MARKER_WIDTH: f32 = 123.0;

#[derive(PartialEq)]
struct Item(usize);

impl Component for Item {
    fn render(&self) -> impl IntoElement {
        let heights = use_consume::<State<Vec<f32>>>();
        let height = heights.read()[self.0];
        rect()
            .width(Size::fill())
            .height(Size::px(height))
            .maybe_child(
                (self.0 == MARKED)
                    .then(|| rect().width(Size::px(MARKER_WIDTH)).height(Size::px(10.))),
            )
    }
}

fn app() -> impl IntoElement {
    let scroll = use_scroll_controller(ScrollConfig::default);
    let anchor = use_scroll_anchor(scroll);
    ScrollView::new_controlled(scroll)
        .width(Size::fill())
        .height(Size::px(300.))
        .on_sized(move |event: Event<SizedEventData>| anchor.view(event.area))
        .children((0..ITEMS).map(|i| {
            Lazy::new(Item(i))
                .key(i)
                .height_hint(100.)
                .recycle(true)
                .into_element()
        }))
}

fn marker_top(runner: &TestingRunner) -> f32 {
    runner
        .find(|node, _| {
            let area = node.layout().area;
            ((area.width() - MARKER_WIDTH).abs() < 0.1).then(|| area.min_y())
        })
        .expect("marker laid out")
}

fn setup() -> (TestingRunner, State<Vec<f32>>) {
    let (mut runner, heights) = TestingRunner::new(
        app,
        (400., 300.).into(),
        |runner| runner.provide_root_context(|| State::create(vec![100.0_f32; ITEMS])),
        1.0,
    );
    runner.poll_n(Duration::from_millis(1), 4);
    for _ in 0..4 {
        runner.scroll((200., 150.), (0., -250.));
        runner.poll_n(Duration::from_millis(1), 4);
    }
    (runner, heights)
}

fn assert_still(runner: &mut TestingRunner, before: f32) {
    for frame in 0..6 {
        runner.sync_and_update();
        let now = marker_top(runner);
        assert!(
            (now - before).abs() < 1.0,
            "frame {frame}: marker moved from {before} to {now}"
        );
    }
}

#[test]
fn growing_above_the_view_keeps_the_view_still() {
    let (mut runner, mut heights) = setup();
    let before = marker_top(&runner);
    assert!((0.0..300.0).contains(&before), "marker in view at {before}");
    runner.run_in(|| heights.write()[MARKED - 2] += 57.0);
    assert_still(&mut runner, before);
}

#[test]
fn shrinking_above_the_view_keeps_the_view_still() {
    let (mut runner, mut heights) = setup();
    let before = marker_top(&runner);
    runner.run_in(|| heights.write()[MARKED - 2] -= 40.0);
    assert_still(&mut runner, before);
}

#[test]
fn growing_below_the_view_top_pushes_what_follows() {
    let (mut runner, mut heights) = setup();
    let before = marker_top(&runner);
    // The marked item's own block grows downwards: the marker, at its
    // top, stays; that is ordinary layout, not anchoring.
    runner.run_in(|| heights.write()[MARKED] += 80.0);
    assert_still(&mut runner, before);
}

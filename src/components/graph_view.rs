//! Full-screen map of the library: every document is a node, every
//! link or reference between two documents is an edge. Tapping a node
//! opens it beside the focused pane.
//!
//! The layout is a small force-directed pass run once over the loaded
//! graph; positions live in unit space and are scaled to whatever area
//! the modal ends up with, so rotation and resizes never push a node
//! outside the card.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use freya::icons::lucide;
use freya::prelude::*;
use freya_engine::prelude::{Color as SkColor, Paint, PaintStyle};

use crate::components::theme::{CARD_BG, PANEL_BG, TEXT_MUTED, TEXT_PRIMARY};
use crate::faces;
use crate::library::{DocKind, ItemId, LinkKind, Rgba, TabTarget, refs};
use crate::space::OpenHint;

use super::modal::{Modal, ModalController};

const MARGIN: f32 = 16.0;
const HEADER_HEIGHT: f32 = 48.0;
const NODE_RADIUS: f32 = 14.0;
const LABEL_WIDTH: f32 = 96.0;
const LABEL_HEIGHT: f32 = 16.0;
/// Space kept between the graph and the card edge so labels stay in.
const INNER_PAD: f32 = LABEL_WIDTH / 2.0 + 4.0;
const ITERATIONS: usize = 250;

/// How two documents are connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeKind {
    Companion,
    Related,
    Reference,
}

impl From<LinkKind> for EdgeKind {
    fn from(kind: LinkKind) -> Self {
        match kind {
            LinkKind::Companion => Self::Companion,
            LinkKind::Related => Self::Related,
        }
    }
}

impl EdgeKind {
    const fn stroke(self) -> (SkColor, f32) {
        match self {
            Self::Companion => (SkColor::from_argb(220, 120, 170, 255), 2.0),
            Self::Related => (SkColor::from_argb(180, 160, 160, 180), 1.5),
            Self::Reference => (SkColor::from_argb(120, 200, 180, 120), 1.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Node {
    id: ItemId,
    name: String,
    kind: DocKind,
    color: Option<Rgba>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Edge {
    a: usize,
    b: usize,
    kind: EdgeKind,
}

/// Nodes with their unit-space positions, ready to scale.
#[derive(Clone, Debug, PartialEq, Default)]
struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    positions: Vec<(f32, f32)>,
}

impl Graph {
    fn from_library(nodes: Vec<Node>, links: &[(ItemId, ItemId, LinkKind)]) -> Self {
        let index: HashMap<ItemId, usize> =
            nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        let mut edges: Vec<Edge> = Vec::new();
        let mut push = |from: ItemId, to: ItemId, kind: EdgeKind| {
            let (Some(&a), Some(&b)) = (index.get(&from), index.get(&to)) else {
                return;
            };
            if a == b {
                return;
            }
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            match edges.iter_mut().find(|e| e.a == lo && e.b == hi) {
                Some(existing) => {
                    if kind == EdgeKind::Companion {
                        existing.kind = kind;
                    }
                }
                None => edges.push(Edge { a: lo, b: hi, kind }),
            }
        };
        for &(from, to, kind) in links {
            push(from, to, kind.into());
        }
        for node in &nodes {
            for entry in refs::refs_from(node.id) {
                push(entry.from, entry.to, EdgeKind::Reference);
            }
        }
        let positions = layout(nodes.len(), &edges);
        Self {
            nodes,
            edges,
            positions,
        }
    }

    /// Neighbour count per node, used to size the dots.
    fn degree(&self, node: usize) -> usize {
        self.edges
            .iter()
            .filter(|e| e.a == node || e.b == node)
            .count()
    }
}

/// Fruchterman-Reingold in the unit square. Deterministic: nodes
/// start on a circle so the same library always draws the same map.
#[allow(clippy::cast_precision_loss)]
fn layout(count: usize, edges: &[Edge]) -> Vec<(f32, f32)> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![(0.5, 0.5)];
    }
    let mut pos: Vec<(f32, f32)> = (0..count)
        .map(|i| {
            let angle = (i as f32) / (count as f32) * std::f32::consts::TAU;
            (0.5 + 0.4 * angle.cos(), 0.5 + 0.4 * angle.sin())
        })
        .collect();
    let k = (1.0 / count as f32).sqrt();
    let mut temperature = 0.1;
    let cooling = temperature / ITERATIONS as f32;
    let mut disp = vec![(0.0f32, 0.0f32); count];
    for _ in 0..ITERATIONS {
        for d in &mut disp {
            *d = (0.0, 0.0);
        }
        for i in 0..count {
            for j in (i + 1)..count {
                let dx = pos[i].0 - pos[j].0;
                let dy = pos[i].1 - pos[j].1;
                let dist = (dx * dx + dy * dy).sqrt().max(0.001);
                let force = k * k / dist;
                let fx = dx / dist * force;
                let fy = dy / dist * force;
                disp[i].0 += fx;
                disp[i].1 += fy;
                disp[j].0 -= fx;
                disp[j].1 -= fy;
            }
        }
        for edge in edges {
            let dx = pos[edge.a].0 - pos[edge.b].0;
            let dy = pos[edge.a].1 - pos[edge.b].1;
            let dist = (dx * dx + dy * dy).sqrt().max(0.001);
            let force = dist * dist / k;
            let fx = dx / dist * force;
            let fy = dy / dist * force;
            disp[edge.a].0 -= fx;
            disp[edge.a].1 -= fy;
            disp[edge.b].0 += fx;
            disp[edge.b].1 += fy;
        }
        for (p, d) in pos.iter_mut().zip(&disp) {
            let len = (d.0 * d.0 + d.1 * d.1).sqrt().max(0.001);
            let step = len.min(temperature);
            p.0 = (p.0 + d.0 / len * step).clamp(0.0, 1.0);
            p.1 = (p.1 + d.1 / len * step).clamp(0.0, 1.0);
        }
        temperature = (temperature - cooling).max(0.005);
    }
    normalise(&mut pos);
    pos
}

/// Stretch the cloud so it fills the unit square on both axes.
fn normalise(pos: &mut [(f32, f32)]) {
    let (min_x, max_x, min_y, max_y) = pos.iter().fold(
        (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
        |(ax, bx, ay, by), &(x, y)| (ax.min(x), bx.max(x), ay.min(y), by.max(y)),
    );
    let span_x = (max_x - min_x).max(0.05);
    let span_y = (max_y - min_y).max(0.05);
    for p in pos {
        p.0 = (p.0 - min_x) / span_x;
        p.1 = (p.1 - min_y) / span_y;
    }
}

/// Modal entry point. [`GraphView::open`] pushes the map over
/// whatever is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GraphView;

impl GraphView {
    pub fn open() {
        ModalController::get().open(Modal::new(Self).manual().dismiss_on_backdrop(true));
    }
}

async fn load_graph() -> Graph {
    let Ok(handle) = crate::home::open_library().await else {
        return Graph::default();
    };
    let lib = handle.lock().await;
    let mut nodes: Vec<Node> = lib
        .index()
        .items
        .iter()
        .map(|it| Node {
            id: it.id,
            name: it.name.clone(),
            kind: it.kind.clone(),
            color: it.color,
        })
        .collect();
    nodes.sort_by_key(|n| n.name.to_lowercase());
    let links: Vec<(ItemId, ItemId, LinkKind)> = lib
        .links()
        .links
        .iter()
        .filter(|l| !l.deleted)
        .map(|l| (l.from, l.to, l.kind))
        .collect();
    drop(lib);
    Graph::from_library(nodes, &links)
}

impl Component for GraphView {
    fn render(&self) -> impl IntoElement {
        let graph = use_state(Graph::default);
        let loaded = use_state(|| false);
        let board = use_state(Area::default);
        use_hook({
            let mut graph = graph;
            let mut loaded = loaded;
            move || {
                spawn(async move {
                    graph.set(load_graph().await);
                    loaded.set(true);
                });
            }
        });

        let viewport = try_consume_context::<notein_ui_kit::ViewportArea>()
            .map_or_else(Area::default, |v| *v.0.read());
        let card_w = (viewport.width() - 2.0 * MARGIN).max(200.0);
        let card_h = (viewport.height() - 2.0 * MARGIN).max(200.0);

        let graph_now = graph.read().clone();
        let area = *board.read();
        let screen: Vec<(f32, f32)> = graph_now
            .positions
            .iter()
            .map(|&(ux, uy)| {
                let w = (area.width() - 2.0 * INNER_PAD).max(1.0);
                let h = (area.height() - 2.0 * INNER_PAD - LABEL_HEIGHT).max(1.0);
                (INNER_PAD + ux * w, INNER_PAD + uy * h)
            })
            .collect();

        let mut body = rect()
            .position(Position::new_absolute().top(MARGIN).left(MARGIN))
            .width(Size::px(card_w))
            .height(Size::px(card_h))
            .background(PANEL_BG)
            .with_corner_radius(16.0)
            .overflow(Overflow::Clip)
            .vertical()
            .child(header(graph_now.nodes.len(), graph_now.edges.len()));

        let empty = *loaded.read() && graph_now.nodes.is_empty();
        if empty {
            body = body.child(
                rect()
                    .width(Size::fill())
                    .height(Size::fill())
                    .center()
                    .child(
                        label()
                            .color(TEXT_MUTED)
                            .font_size(14.0)
                            .text("Todavía no hay documentos para mostrar."),
                    ),
            );
            return body;
        }

        let mut stage = rect()
            .width(Size::fill())
            .height(Size::fill())
            .on_sized({
                let mut board = board;
                move |e: Event<SizedEventData>| board.set(e.area)
            })
            .child(edges_layer(&graph_now, &screen));
        for (i, node) in graph_now.nodes.iter().enumerate() {
            let Some(&(x, y)) = screen.get(i) else {
                continue;
            };
            stage = stage.child(node_chip(node, graph_now.degree(i), x, y));
        }
        body.child(stage)
    }
}

fn header(nodes: usize, edges: usize) -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::px(HEADER_HEIGHT))
        .horizontal()
        .content(Content::Flex)
        .cross_align(Alignment::Center)
        .padding((0.0, 16.0))
        .background(CARD_BG)
        .child(
            SvgViewer::new(lucide::waypoints())
                .color(TEXT_PRIMARY)
                .width(Size::px(18.0))
                .height(Size::px(18.0)),
        )
        .child(spacer_px(10.0))
        .child(label().color(TEXT_PRIMARY).font_size(15.0).text("Mapa"))
        .child(spacer_px(12.0))
        .child(
            label()
                .color(TEXT_MUTED)
                .font_size(12.0)
                .text(format!("{nodes} documentos · {edges} conexiones")),
        )
        .child(flexible_spacer())
        .child(
            rect()
                .width(Size::px(32.0))
                .height(Size::px(32.0))
                .center()
                .with_corner_radius(16.0)
                .on_press(|_| ModalController::get().close())
                .child(
                    SvgViewer::new(lucide::x())
                        .color(TEXT_PRIMARY)
                        .width(Size::px(18.0))
                        .height(Size::px(18.0)),
                ),
        )
}

fn spacer_px(w: f32) -> Rect {
    rect().width(Size::px(w))
}

fn flexible_spacer() -> Rect {
    rect().width(Size::flex(1.0))
}

/// Skia pass drawing every edge. The segment list is shared through a
/// cell so the paint callback sees the latest layout even though Freya
/// treats the callback itself as unchanged between renders.
fn edges_layer(graph: &Graph, screen: &[(f32, f32)]) -> impl IntoElement {
    let segments = use_hook(|| {
        Rc::new(RefCell::new(
            Vec::<((f32, f32), (f32, f32), EdgeKind)>::new(),
        ))
    });
    *segments.borrow_mut() = graph
        .edges
        .iter()
        .filter_map(|e| Some((*screen.get(e.a)?, *screen.get(e.b)?, e.kind)))
        .collect();
    let segments_ref = segments.clone();
    canvas(RenderCallback::new(move |ctx| {
        for &(a, b, kind) in segments_ref.borrow().iter() {
            let (color, width) = kind.stroke();
            let mut paint = Paint::default();
            paint.set_color(color);
            paint.set_style(PaintStyle::Stroke);
            paint.set_stroke_width(width);
            paint.set_anti_alias(true);
            ctx.canvas.draw_line((a.0, a.1), (b.0, b.1), &paint);
        }
    }))
    .position(Position::new_absolute().top(0.0).left(0.0))
    .width(Size::fill())
    .height(Size::fill())
}

#[allow(clippy::cast_precision_loss)]
fn node_chip(node: &Node, degree: usize, x: f32, y: f32) -> Rect {
    let id = node.id;
    let radius = NODE_RADIUS + (degree.min(6) as f32) * 1.5;
    let fill = node.color.map_or(Color::from_rgb(90, 100, 130), |c| {
        Color::from_rgb(c[0], c[1], c[2])
    });
    rect()
        .position(
            Position::new_absolute()
                .top(y - radius)
                .left(x - LABEL_WIDTH / 2.0),
        )
        .width(Size::px(LABEL_WIDTH))
        .vertical()
        .cross_align(Alignment::Center)
        .spacing(4.0)
        .on_press(move |_| {
            ModalController::get().close();
            crate::space::open(TabTarget::Item(id), OpenHint::Beside);
        })
        .child(
            rect()
                .width(Size::px(radius * 2.0))
                .height(Size::px(radius * 2.0))
                .center()
                .background(fill)
                .border(Border::new().width(1.5).fill(Color::from_rgb(20, 20, 26)))
                .with_corner_radius(radius)
                .child(
                    SvgViewer::new(faces::icon(&node.kind))
                        .color(Color::from_rgb(245, 245, 250))
                        .width(Size::px(radius))
                        .height(Size::px(radius)),
                ),
        )
        .child(
            label()
                .color(TEXT_PRIMARY)
                .font_size(11.0)
                .max_lines(1)
                .text_overflow(TextOverflow::Ellipsis)
                .text_align(TextAlign::Center)
                .width(Size::px(LABEL_WIDTH))
                .text(node.name.clone()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_keeps_every_node_inside_the_unit_square() {
        let edges = vec![
            Edge {
                a: 0,
                b: 1,
                kind: EdgeKind::Related,
            },
            Edge {
                a: 1,
                b: 2,
                kind: EdgeKind::Companion,
            },
        ];
        for &(x, y) in &layout(4, &edges) {
            assert!((0.0..=1.0).contains(&x), "x = {x}");
            assert!((0.0..=1.0).contains(&y), "y = {y}");
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let edges = vec![Edge {
            a: 0,
            b: 2,
            kind: EdgeKind::Reference,
        }];
        assert_eq!(layout(3, &edges), layout(3, &edges));
    }

    #[test]
    fn linked_nodes_end_up_closer_than_unlinked_ones() {
        let edges = vec![Edge {
            a: 0,
            b: 1,
            kind: EdgeKind::Companion,
        }];
        let pos = layout(3, &edges);
        let dist = |a: usize, b: usize| {
            let dx = pos[a].0 - pos[b].0;
            let dy = pos[a].1 - pos[b].1;
            (dx * dx + dy * dy).sqrt()
        };
        assert!(dist(0, 1) < dist(0, 2));
        assert!(dist(0, 1) < dist(1, 2));
    }
}

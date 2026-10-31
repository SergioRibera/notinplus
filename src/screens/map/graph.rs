//! The library as a graph: documents are nodes, notebooks, links and
//! references are edges, laid out once with a force-directed pass.

use std::collections::HashMap;

use notein_i18n::tr;

use crate::library::{ItemId, LibraryData, LinkKind, refs};

/// Rounds of the layout pass.
const ITERATIONS: usize = 250;

/// How two documents are connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum EdgeKind {
    /// One is a notebook of the other.
    Notebook,
    /// Linked by the user.
    Link,
    /// One mentions the other.
    Reference,
}

impl EdgeKind {
    pub(super) const ALL: [Self; 3] = [Self::Notebook, Self::Link, Self::Reference];

    #[must_use]
    pub(super) fn label(self) -> String {
        match self {
            Self::Notebook => tr!("map-edge-notebook"),
            Self::Link => tr!("map-edge-link"),
            Self::Reference => tr!("map-edge-reference"),
        }
    }
}

impl From<LinkKind> for EdgeKind {
    fn from(kind: LinkKind) -> Self {
        match kind {
            LinkKind::Companion => Self::Notebook,
            LinkKind::Related => Self::Link,
        }
    }
}

/// A document on the map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Node {
    pub(super) id: ItemId,
    pub(super) name: String,
    pub(super) kind: String,
}

/// An edge between nodes `a` and `b` (indices, `a < b`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Edge {
    pub(super) a: usize,
    pub(super) b: usize,
    pub(super) kind: EdgeKind,
}

/// Nodes with their positions in the unit square.
#[derive(Clone, Debug, PartialEq, Default)]
pub(super) struct Graph {
    pub(super) nodes: Vec<Node>,
    pub(super) edges: Vec<Edge>,
    pub(super) positions: Vec<(f32, f32)>,
}

impl Graph {
    /// The graph of every live document of `data`.
    #[must_use]
    #[sismo::function("map::graph_of")]
    pub(super) fn of(data: &LibraryData) -> Self {
        let mut nodes: Vec<Node> = data
            .live_items()
            .map(|i| Node {
                id: i.id,
                name: i.name.clone(),
                kind: i.kind.plugin_id.clone(),
            })
            .collect();
        nodes.sort_by_cached_key(|n| n.name.to_lowercase());
        let mut links: Vec<(ItemId, ItemId, EdgeKind)> = data
            .links()
            .links
            .iter()
            .filter(|l| !l.deleted)
            .map(|l| (l.from, l.to, l.kind.into()))
            .collect();
        for node in &nodes {
            links.extend(
                refs::refs_from(node.id)
                    .into_iter()
                    .map(|r| (r.from, r.to, EdgeKind::Reference)),
            );
        }
        Self::build(nodes, &links)
    }

    /// Lay out `nodes` joined by `links` (by item id).
    #[must_use]
    #[sismo::function("map::graph_build")]
    pub(super) fn build(nodes: Vec<Node>, links: &[(ItemId, ItemId, EdgeKind)]) -> Self {
        let index: HashMap<ItemId, usize> =
            nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        let mut edges: Vec<Edge> = Vec::new();
        for &(from, to, kind) in links {
            let (Some(&a), Some(&b)) = (index.get(&from), index.get(&to)) else {
                continue;
            };
            if a == b {
                continue;
            }
            let (a, b) = (a.min(b), a.max(b));
            match edges.iter_mut().find(|e| e.a == a && e.b == b) {
                Some(existing) if kind == EdgeKind::Notebook => existing.kind = kind,
                Some(_) => {}
                None => edges.push(Edge { a, b, kind }),
            }
        }
        let positions = layout(nodes.len(), &edges);
        Self {
            nodes,
            edges,
            positions,
        }
    }

    /// Edges touching node `node`.
    #[must_use]
    pub(super) fn degree(&self, node: usize) -> usize {
        self.edges
            .iter()
            .filter(|e| e.a == node || e.b == node)
            .count()
    }
}

/// Fruchterman-Reingold in the unit square. Deterministic: nodes start
/// on a circle so the same library always draws the same map.
#[allow(clippy::cast_precision_loss)] // node counts
#[sismo::function("map::graph_layout")]
fn layout(count: usize, edges: &[Edge]) -> Vec<(f32, f32)> {
    match count {
        0 => return Vec::new(),
        1 => return vec![(0.5, 0.5)],
        _ => {}
    }
    let mut pos: Vec<(f32, f32)> = (0..count)
        .map(|i| {
            let angle = (i as f32) / (count as f32) * std::f32::consts::TAU;
            (
                0.4f32.mul_add(angle.cos(), 0.5),
                0.4f32.mul_add(angle.sin(), 0.5),
            )
        })
        .collect();
    let k = (1.0 / count as f32).sqrt();
    let mut temperature = 0.1;
    let cooling = temperature / ITERATIONS as f32;
    let mut disp = vec![(0.0f32, 0.0f32); count];
    for _ in 0..ITERATIONS {
        disp.fill((0.0, 0.0));
        for i in 0..count {
            for j in (i + 1)..count {
                let (dx, dy) = (pos[i].0 - pos[j].0, pos[i].1 - pos[j].1);
                let length = dx.hypot(dy).max(0.001);
                let force = k * k / length;
                let (fx, fy) = (dx / length * force, dy / length * force);
                disp[i].0 += fx;
                disp[i].1 += fy;
                disp[j].0 -= fx;
                disp[j].1 -= fy;
            }
        }
        for edge in edges {
            let (dx, dy) = (pos[edge.a].0 - pos[edge.b].0, pos[edge.a].1 - pos[edge.b].1);
            let length = dx.hypot(dy).max(0.001);
            let force = length * length / k;
            let (fx, fy) = (dx / length * force, dy / length * force);
            disp[edge.a].0 -= fx;
            disp[edge.a].1 -= fy;
            disp[edge.b].0 += fx;
            disp[edge.b].1 += fy;
        }
        for (p, d) in pos.iter_mut().zip(&disp) {
            let len = d.0.hypot(d.1).max(0.001);
            let step = len.min(temperature);
            p.0 = (d.0 / len).mul_add(step, p.0).clamp(0.0, 1.0);
            p.1 = (d.1 / len).mul_add(step, p.1).clamp(0.0, 1.0);
        }
        temperature = (temperature - cooling).max(0.005);
    }
    normalise(&mut pos);
    pos
}

/// Stretch the cloud so it fills the unit square on both axes.
#[sismo::function("map::graph_normalise")]
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

#[cfg(test)]
mod tests {
    use super::{Edge, EdgeKind, layout};

    #[test]
    fn layout_keeps_every_node_inside_the_unit_square() {
        let edges = [
            Edge {
                a: 0,
                b: 1,
                kind: EdgeKind::Link,
            },
            Edge {
                a: 1,
                b: 2,
                kind: EdgeKind::Notebook,
            },
        ];
        for &(x, y) in &layout(4, &edges) {
            assert!((0.0..=1.0).contains(&x), "x = {x}");
            assert!((0.0..=1.0).contains(&y), "y = {y}");
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let edges = [Edge {
            a: 0,
            b: 2,
            kind: EdgeKind::Reference,
        }];
        assert_eq!(layout(3, &edges), layout(3, &edges));
    }

    #[test]
    fn linked_nodes_end_up_closer_than_unlinked_ones() {
        let pos = layout(
            3,
            &[Edge {
                a: 0,
                b: 1,
                kind: EdgeKind::Notebook,
            }],
        );
        let dist = |a: usize, b: usize| (pos[a].0 - pos[b].0).hypot(pos[a].1 - pos[b].1);
        assert!(dist(0, 1) < dist(0, 2));
        assert!(dist(0, 1) < dist(1, 2));
    }
}

//! The document's outline in the panel: a tree that starts folded, so a
//! Bible shows its books and not its 1189 chapters, and that lays out
//! only the rows on screen.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use freya::prelude::*;
use notein_plugin::kind::{OutlineIcon, OutlineNode};
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Icon, Leading, ListRow, ScrollContent, Text, TextRole, tokens};

use crate::library::ItemId;

/// Where a node sits: its index under each ancestor, from the root.
type NodePath = Vec<u16>;

/// A document's outline as its kind built it.
type Outline = Vec<OutlineNode<Vec<u8>>>;

/// Which branches of a document's outline are open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Folds {
    item: Option<ItemId>,
    open: HashSet<NodePath>,
}

impl Folds {
    /// Every branch starts closed, and closes again in another document.
    fn is_open(&self, item: ItemId, path: &NodePath) -> bool {
        self.item == Some(item) && self.open.contains(path)
    }

    fn toggle(&mut self, item: ItemId, path: NodePath) {
        if self.item != Some(item) {
            *self = Self {
                item: Some(item),
                open: HashSet::new(),
            };
        }
        if !self.open.remove(&path) {
            self.open.insert(path);
        }
    }
}

/// The outline last built for a document, kept while its content stays
/// the same: building one can take a query per book.
#[derive(Clone, Default)]
pub(super) struct OutlineCache(Rc<RefCell<Option<Cached>>>);

struct Cached {
    item: ItemId,
    content: Vec<u8>,
    outline: Rc<Outline>,
}

impl OutlineCache {
    pub(super) fn get(
        &self,
        item: ItemId,
        content: &[u8],
        build: impl FnOnce() -> Outline,
    ) -> Rc<Outline> {
        let mut cached = self.0.borrow_mut();
        if let Some(hit) = cached
            .as_ref()
            .filter(|c| c.item == item && c.content == content)
        {
            return Rc::clone(&hit.outline);
        }
        let outline = Rc::new(build());
        *cached = Some(Cached {
            item,
            content: content.to_vec(),
            outline: Rc::clone(&outline),
        });
        outline
    }
}

/// One row on show.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    label: String,
    icon: OutlineIcon,
    meta: Option<String>,
    anchor: Option<Vec<u8>>,
    depth: u8,
    path: NodePath,
    branch: bool,
    open: bool,
}

fn matches(node: &OutlineNode<Vec<u8>>, query: &str) -> bool {
    query.is_empty()
        || node.label.to_lowercase().contains(query)
        || node.children.iter().any(|c| matches(c, query))
}

/// What the walk over the tree needs besides where it is.
struct Walk<'a> {
    query: &'a str,
    folds: &'a Folds,
    item: ItemId,
    lines: Vec<Line>,
}

impl Walk<'_> {
    /// The rows under open branches. While searching, every branch that
    /// leads to a match is open.
    fn visit(&mut self, nodes: &[OutlineNode<Vec<u8>>], depth: u8, path: &mut NodePath) {
        for (index, node) in nodes.iter().enumerate() {
            if !matches(node, self.query) {
                continue;
            }
            path.push(u16::try_from(index).unwrap_or(u16::MAX));
            let branch = !node.children.is_empty();
            let open = branch && (!self.query.is_empty() || self.folds.is_open(self.item, path));
            self.lines.push(Line {
                label: node.label.clone(),
                icon: node.icon,
                meta: node.meta.clone(),
                anchor: node.anchor.clone(),
                depth,
                path: path.clone(),
                branch,
                open,
            });
            if open {
                self.visit(&node.children, depth.saturating_add(1), path);
            }
            path.pop();
        }
    }
}

fn outline_icon(icon: OutlineIcon) -> Bytes {
    use freya::icons::lucide;
    match icon {
        OutlineIcon::Section => lucide::list(),
        OutlineIcon::Page => lucide::file(),
        OutlineIcon::Heading => lucide::heading(),
        OutlineIcon::Layer => lucide::layers(),
        OutlineIcon::Book => lucide::book_open(),
    }
}

/// The outline of `item`, filtered by `query`. It fills the panel, or in
/// a sheet hugs its rows up to `max_height`; it scrolls inside that.
#[derive(Clone, PartialEq)]
pub(super) struct OutlineList {
    pub(super) outline: Rc<Outline>,
    pub(super) query: String,
    pub(super) kind: String,
    pub(super) item: ItemId,
    pub(super) folds: State<Folds>,
    pub(super) max_height: Option<f32>,
}

/// What each row is built from.
#[derive(Clone, PartialEq)]
struct Rows {
    lines: Vec<Line>,
    kind: String,
    item: ItemId,
    folds: State<Folds>,
}

impl Rows {
    fn row(&self, line: &Line) -> Element {
        let p = palette();
        let (item, mut folds) = (self.item, self.folds);
        let path = line.path.clone();
        let mut toggle = move || folds.write().toggle(item, path.clone());
        let leading = if line.branch {
            let mut toggle = toggle.clone();
            Leading::Element(
                rect()
                    .a11y_role(AccessibilityRole::Button)
                    .a11y_alt(if line.open { "Contraer" } else { "Expandir" })
                    .on_tap(move |e: Event<PressEventData>| {
                        e.stop_propagation();
                        toggle();
                    })
                    .child(
                        Icon::new(if line.open {
                            freya::icons::lucide::chevron_down()
                        } else {
                            freya::icons::lucide::chevron_right()
                        })
                        .size(18.0)
                        .color(p.t2),
                    )
                    .into_element(),
            )
        } else {
            Leading::Icon(outline_icon(line.icon))
        };
        let row = ListRow::new(line.label.clone())
            .leading(leading)
            .dense(true)
            .indent(f32::from(line.depth) * 14.0);
        let row = match &line.meta {
            Some(meta) => row.meta(meta.clone()),
            None => row,
        };
        let (kind, anchor, branch) = (self.kind.clone(), line.anchor.clone(), line.branch);
        rect()
            .key(&line.path)
            .width(Size::fill())
            .height(Size::px(tokens::size::ROW_DENSE))
            .child(
                row.enabled(anchor.is_some() || branch)
                    .on_press(move |_| match &anchor {
                        Some(anchor) => super::jump(&kind, item, anchor.clone()),
                        None => toggle(),
                    }),
            )
            .into_element()
    }
}

impl Component for OutlineList {
    fn render(&self) -> impl IntoElement {
        let folds = self.folds.read();
        let mut walk = Walk {
            query: &self.query,
            folds: &folds,
            item: self.item,
            lines: Vec::new(),
        };
        walk.visit(&self.outline, 0, &mut NodePath::new());
        let lines = walk.lines;
        drop(folds);
        if lines.is_empty() {
            return Text::new(if self.query.is_empty() {
                "Este documento no tiene índice."
            } else {
                "Nada con ese nombre."
            })
            .role(TextRole::Hint)
            .into_element();
        }
        let length = lines.len();
        let rows = Rows {
            lines,
            kind: self.kind.clone(),
            item: self.item,
            folds: self.folds,
        };
        let list = VirtualScrollView::new_with_data(rows, |at, rows: &Rows| {
            rows.lines
                .get(at.index)
                .map_or_else(|| rect().into_element(), |line| rows.row(line))
        })
        .length(length)
        .item_size(tokens::size::ROW_DENSE)
        .width(Size::fill())
        .height(Size::fill());
        rect()
            .width(Size::fill())
            .height(self.max_height.map_or_else(Size::fill, |max| {
                // Four of padding above and below the rows.
                #[allow(clippy::cast_precision_loss, reason = "an outline has far fewer rows")]
                let rows = (length as f32).mul_add(tokens::size::ROW_DENSE, 8.0);
                Size::px(rows.min(max))
            }))
            .corner_radius(tokens::radius::XL)
            .background(palette().s1)
            .padding(4.0)
            .child(ScrollContent::new(list).height(Size::fill()))
            .into_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two books of three chapters, like a Bible's outline.
    fn bible() -> Outline {
        ["Génesis", "Éxodo"]
            .into_iter()
            .map(|book| {
                OutlineNode::new(book, None).children(
                    (1..=3)
                        .map(|c| OutlineNode::new(format!("{book} {c}"), Some(vec![c])))
                        .collect(),
                )
            })
            .collect()
    }

    fn labels(query: &str, folds: &Folds) -> Vec<String> {
        let mut walk = Walk {
            query,
            folds,
            item: ItemId([1; 16]),
            lines: Vec::new(),
        };
        walk.visit(&bible(), 0, &mut NodePath::new());
        walk.lines.into_iter().map(|l| l.label).collect()
    }

    #[test]
    fn outline_starts_folded() {
        assert_eq!(labels("", &Folds::default()), ["Génesis", "Éxodo"]);
    }

    #[test]
    fn opening_a_branch_shows_its_children_only() {
        let mut folds = Folds::default();
        folds.toggle(ItemId([1; 16]), vec![1]);
        assert_eq!(
            labels("", &folds),
            ["Génesis", "Éxodo", "Éxodo 1", "Éxodo 2", "Éxodo 3"]
        );
        folds.toggle(ItemId([1; 16]), vec![1]);
        assert_eq!(labels("", &folds), ["Génesis", "Éxodo"]);
    }

    #[test]
    fn folds_belong_to_their_document() {
        let mut folds = Folds::default();
        folds.toggle(ItemId([2; 16]), vec![0]);
        assert_eq!(labels("", &folds), ["Génesis", "Éxodo"]);
    }

    #[test]
    fn searching_opens_the_branches_that_match() {
        assert_eq!(labels("éxodo 2", &Folds::default()), ["Éxodo", "Éxodo 2"]);
    }
}

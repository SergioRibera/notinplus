//! The map of the library: every document and how it connects.

mod graph;

use std::cell::RefCell;
use std::rc::Rc;

use freya::prelude::*;
use freya_engine::prelude::{Color as SkColor, Paint, PaintStyle, PathEffect};
use notein_ui_kit::theme::{Palette, palette};
use notein_ui_kit::{
    Button, EmptyState, Icon, IconButton, Modal, ModalController, Tag, Text, TextRole, tokens,
    width_class,
};

use self::graph::{EdgeKind, Graph};
use crate::references::kind_color;
use crate::state::LibraryView;

const NODE: f32 = 26.0;
const LABEL_WIDTH: f32 = 104.0;
/// Space kept between the graph and the stage edge so labels stay in.
const INNER_PAD: f32 = LABEL_WIDTH / 2.0 + 8.0;

/// Open the map over whatever is on screen.
pub fn open() {
    ModalController::get().open(Modal::new(Map).manual().dismiss_on_backdrop(false));
}

fn sk(color: Color) -> SkColor {
    SkColor::from_argb(color.a(), color.r(), color.g(), color.b())
}

/// Stroke of an edge kind: notebooks solid, links dashed, references
/// thin.
fn stroke(kind: EdgeKind, p: &Palette) -> Paint {
    let mut paint = Paint::default();
    paint.set_style(PaintStyle::Stroke);
    paint.set_anti_alias(true);
    match kind {
        EdgeKind::Notebook => {
            paint.set_color(sk(p.accent));
            paint.set_stroke_width(2.2);
        }
        EdgeKind::Link => {
            paint.set_color(sk(p.t2));
            paint.set_stroke_width(1.6);
            paint.set_path_effect(PathEffect::dash(&[7.0, 5.0], 0.0));
        }
        EdgeKind::Reference => {
            paint.set_color(sk(p.t3));
            paint.set_stroke_width(0.9);
        }
    }
    paint
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Map;

/// A line between two node centres, in stage coordinates.
type Segment = ((f32, f32), (f32, f32), EdgeKind);

/// What the stage shows: which nodes, where, and around which one.
struct Layout {
    graph: Graph,
    visible: Vec<bool>,
    /// Node centres on the stage.
    screen: Vec<(f32, f32)>,
    /// The node the card is about.
    current: Option<usize>,
    /// Nodes connected to `current` by a shown edge.
    neighbours: Vec<usize>,
    segments: Vec<Segment>,
}

impl Layout {
    fn new(
        graph: Graph,
        area: Area,
        kinds_hidden: &[String],
        edges_hidden: &[EdgeKind],
        selected: Option<usize>,
    ) -> Self {
        let visible: Vec<bool> = graph
            .nodes
            .iter()
            .map(|n| !kinds_hidden.contains(&n.kind))
            .collect();
        let w = 2.0f32.mul_add(-INNER_PAD, area.width()).max(1.0);
        let h = 2.0f32.mul_add(-INNER_PAD, area.height()).max(1.0);
        let screen: Vec<(f32, f32)> = graph
            .positions
            .iter()
            .map(|&(ux, uy)| (ux.mul_add(w, INNER_PAD), uy.mul_add(h, INNER_PAD)))
            .collect();
        let shown = |a: usize, b: usize, kind: EdgeKind| {
            visible[a] && visible[b] && !edges_hidden.contains(&kind)
        };
        let segments = graph
            .edges
            .iter()
            .filter(|e| shown(e.a, e.b, e.kind))
            .filter_map(|e| Some((*screen.get(e.a)?, *screen.get(e.b)?, e.kind)))
            .collect();
        let current = selected.filter(|i| visible.get(*i).copied().unwrap_or(false));
        let neighbours = current.map_or_else(Vec::new, |i| {
            graph
                .edges
                .iter()
                .filter(|e| shown(e.a, e.b, e.kind))
                .filter_map(|e| match (e.a == i, e.b == i) {
                    (true, _) => Some(e.b),
                    (_, true) => Some(e.a),
                    _ => None,
                })
                .collect()
        });
        Self {
            graph,
            visible,
            screen,
            current,
            neighbours,
            segments,
        }
    }

    /// Node `i`: a disc sized by its connections, with its name.
    fn node(&self, i: usize, mut selected: State<Option<usize>>) -> Option<Element> {
        let p = palette();
        let node = self.graph.nodes.get(i)?;
        let &(x, y) = self.screen.get(i)?;
        let info = crate::plugins::kind_info(&node.kind);
        let tint = info.as_ref().map_or(p.t3, |k| kind_color(&p, k.color));
        let icon = info.map_or_else(freya::icons::lucide::file, |k| k.icon);
        #[allow(clippy::cast_precision_loss)] // small degree counts
        let radius = (self.graph.degree(i).min(6) as f32).mul_add(1.5, NODE / 2.0);
        let focused = self.current == Some(i);
        let dimmed = self.current.is_some() && !focused && !self.neighbours.contains(&i);
        Some(
            rect()
                .key(node.id.0)
                .position(
                    Position::new_absolute()
                        .top(y - radius)
                        .left(x - LABEL_WIDTH / 2.0),
                )
                .width(Size::px(LABEL_WIDTH))
                .cross_align(Alignment::Center)
                .spacing(4.0)
                .opacity(if dimmed { 0.35 } else { 1.0 })
                .a11y_role(AccessibilityRole::Button)
                .a11y_alt(node.name.clone())
                .on_press(move |_| selected.set(if focused { None } else { Some(i) }))
                .child(
                    rect()
                        .width(Size::px(radius * 2.0))
                        .height(Size::px(radius * 2.0))
                        .corner_radius(radius)
                        .background(tint)
                        .border(
                            Border::new()
                                .fill(if focused { p.t1 } else { p.bg })
                                .width(if focused { 3.0 } else { 2.0 })
                                .alignment(BorderAlignment::Outer),
                        )
                        .center()
                        .child(
                            Icon::new(icon)
                                .size(radius)
                                .color(notein_ui_kit::theme::on_color(tint)),
                        ),
                )
                .child(
                    label()
                        .text(node.name.clone())
                        .font_size(11.0)
                        .color(p.t1)
                        .max_lines(1)
                        .text_overflow(TextOverflow::Ellipsis)
                        .text_align(TextAlign::Center)
                        .width(Size::px(LABEL_WIDTH)),
                )
                .into_element(),
        )
    }

    /// The card of the selected node: what it is, what it connects to,
    /// and how to open it.
    fn card(&self, compact: bool, size: Size2D, bottom_inset: f32) -> Option<Rect> {
        let p = palette();
        let node = self.graph.nodes.get(self.current?)?;
        let id = node.id;
        let info = crate::plugins::kind_info(&node.kind);
        let connected: Vec<Element> = self
            .neighbours
            .iter()
            .filter_map(|n| self.graph.nodes.get(*n))
            .take(6)
            .map(|n| {
                let other = n.id;
                Tag::new(n.name.clone())
                    .on_press(move |_| {
                        ModalController::get().close();
                        crate::flows::open_beside(other);
                    })
                    .into_element()
            })
            .collect();
        let action =
            |label: &'static str, button: fn(Button) -> Button, run: fn(crate::library::ItemId)| {
                button(Button::new(label)).small().on_press(move |_| {
                    ModalController::get().close();
                    run(id);
                })
            };
        Some(
            rect()
                .position(if compact {
                    Position::new_absolute()
                        .left(12.0)
                        .bottom(12.0 + bottom_inset)
                } else {
                    Position::new_absolute().right(20.0).bottom(20.0)
                })
                .width(if compact {
                    Size::px(size.width - 24.0)
                } else {
                    Size::px(320.0)
                })
                .padding(16.0)
                .corner_radius(tokens::radius::XL)
                .background(p.s1)
                .border(
                    Border::new()
                        .fill(p.line)
                        .width(1.0)
                        .alignment(BorderAlignment::Inner),
                )
                .shadow(Shadow::new().y(12.0).blur(32.0).color(p.shadow))
                .spacing(10.0)
                .child(Text::new(node.name.clone()).role(TextRole::Title).lines(2))
                .child(
                    Text::new(format!(
                        "{} · {}",
                        info.map_or_else(|| "Documento".to_owned(), |k| k.label.into_owned()),
                        crate::format::count(self.neighbours.len(), "conexión", "conexiones")
                    ))
                    .role(TextRole::Meta),
                )
                .maybe_child((!connected.is_empty()).then(|| {
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .content(Content::wrap_spacing(6.0))
                        .spacing(6.0)
                        .children(connected)
                }))
                .child(
                    rect()
                        .horizontal()
                        .content(Content::wrap_spacing(8.0))
                        .spacing(8.0)
                        .child(action("Abrir", Button::primary, crate::flows::open))
                        .child(action("Al lado", Button::tonal, crate::flows::open_beside))
                        .child(action(
                            "Vincular con…",
                            Button::ghost,
                            crate::flows::link_with,
                        )),
                ),
        )
    }
}

/// Add `value` to `list`, or take it out when it is there.
fn toggle<T: PartialEq>(list: &mut Vec<T>, value: T) {
    match list.iter().position(|v| *v == value) {
        Some(at) => {
            list.remove(at);
        }
        None => list.push(value),
    }
}

/// Chips to hide kinds of documents and of connections.
fn filters(
    mut hidden_kinds: State<Vec<String>>,
    mut hidden_edges: State<Vec<EdgeKind>>,
) -> Element {
    let p = palette();
    let kinds_hidden = hidden_kinds.read().clone();
    let edges_hidden = hidden_edges.read().clone();
    let kinds = crate::plugins::catalog().kinds();
    let kind_chips = kinds.iter().map(|k| {
        let info = k.info();
        let id = info.id.to_string();
        Tag::new(info.plural.clone())
            .dot(kind_color(&p, info.color))
            .active(!kinds_hidden.contains(&id))
            .on_press(move |_| toggle(&mut hidden_kinds.write(), id.clone()))
            .into_element()
    });
    let edge_chips = EdgeKind::ALL.into_iter().map(|kind| {
        Tag::new(kind.label())
            .active(!edges_hidden.contains(&kind))
            .on_press(move |_| toggle(&mut hidden_edges.write(), kind))
            .into_element()
    });
    ScrollView::new()
        .direction(Direction::Horizontal)
        .width(Size::fill())
        .height(Size::px(34.0))
        .show_scrollbar(false)
        .child(
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(6.0)
                .children(kind_chips)
                .child(
                    rect()
                        .width(Size::px(1.0))
                        .height(Size::px(20.0))
                        .background(p.line),
                )
                .children(edge_chips),
        )
        .into_element()
}

/// Back, the title and how much there is.
fn title_row(graph: &Graph) -> Rect {
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::Flex)
        .spacing(8.0)
        .child(
            IconButton::new(freya::icons::lucide::arrow_left(), "Volver")
                .on_press(|_| ModalController::get().close()),
        )
        .child(
            rect()
                .width(Size::flex(1.0))
                .child(Text::new("Mapa").role(TextRole::Title))
                .child(
                    Text::new(format!(
                        "{} · {}",
                        crate::format::count(graph.nodes.len(), "documento", "documentos"),
                        crate::format::count(graph.edges.len(), "conexión", "conexiones")
                    ))
                    .role(TextRole::Meta),
                ),
        )
}

impl Component for Map {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let data = LibraryView::get().data();
        let graph = use_memo(move || data.as_deref().map(Graph::of).unwrap_or_default());
        let hidden_kinds = use_state(Vec::<String>::new);
        let hidden_edges = use_state(Vec::<EdgeKind>::new);
        let mut selected = use_state(|| None::<usize>);
        let mut stage = use_state(Area::default);
        let segments = use_hook(|| Rc::new(RefCell::new(Vec::<Segment>::new())));
        let size = *Platform::get().current_window().root_size.read();
        let insets = *crate::hooks::use_safe_area_insets().read();

        let layout = Layout::new(
            graph.read().clone(),
            *stage.read(),
            &hidden_kinds.read(),
            &hidden_edges.read(),
            *selected.read(),
        );
        segments.borrow_mut().clone_from(&layout.segments);
        let paint_segments = Rc::clone(&segments);
        // Paint runs outside the component, where `palette()` is not
        // available.
        let edge_palette = p;
        let edges_layer = rect()
            .background(RenderCallback::new(move |ctx| {
                let RenderTarget::Background { canvas, .. } = &ctx.target else {
                    return;
                };
                let p = edge_palette;
                for &(a, b, kind) in paint_segments.borrow().iter() {
                    canvas.draw_line((a.0, a.1), (b.0, b.1), &stroke(kind, &p));
                }
            }))
            .position(Position::new_absolute().top(0.0).left(0.0))
            .width(Size::fill())
            .height(Size::fill());

        let body = if layout.graph.nodes.is_empty() {
            EmptyState::new(
                freya::icons::lucide::waypoints(),
                "Todavía no hay nada para mostrar",
            )
            .body("Cuando crees documentos y los vincules, vas a ver acá cómo se conectan.")
            .into_element()
        } else {
            let nodes: Vec<Element> = (0..layout.graph.nodes.len())
                .filter(|i| layout.visible[*i])
                .filter_map(|i| layout.node(i, selected))
                .collect();
            rect()
                .width(Size::fill())
                .height(Size::flex(1.0))
                .on_sized(move |e: Event<SizedEventData>| stage.set(e.area))
                .on_press(move |_| selected.set(None))
                .child(edges_layer)
                .children(nodes)
                .maybe_child(layout.card(width_class().is_compact(), size, insets.bottom))
                .into_element()
        };

        rect()
            .position(Position::new_absolute().top(0.0).left(0.0))
            .width(Size::px(size.width))
            .height(Size::px(size.height))
            .background(p.bg)
            .padding((insets.top, insets.right, 0.0, insets.left))
            .content(Content::Flex)
            .on_global_key_down(|e: Event<KeyboardEventData>| {
                if matches!(e.key, Key::Named(NamedKey::Escape | NamedKey::BrowserBack)) {
                    e.stop_propagation();
                    ModalController::get().close();
                }
            })
            .child(
                rect()
                    .width(Size::fill())
                    .padding((8.0, 12.0))
                    .spacing(8.0)
                    .child(title_row(&layout.graph))
                    .child(filters(hidden_kinds, hidden_edges)),
            )
            .child(body)
    }
}

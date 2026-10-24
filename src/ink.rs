//! What floats over a canvas: the ink dock with every tool, the
//! popover that tunes the active one, the bar over a lasso selection,
//! the layers list and the zoom pill.

use std::sync::{Arc, Mutex};

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    BarItem, Icon, IconButton, Popover, PopoverAlign, PopoverSide, Segmented, SelectionBar, Text,
    TextRole, Toast, Toasts, tokens, width_class,
};

use crate::brush::{
    BrushConfig, BrushKind, BrushPreset, EraserMode, HighlighterTip, PressureCurve, ShapeMode,
};
use crate::canvas::{Board, InputMode, lock};
use crate::ui_mask::MaskSlot;

/// Ink colours one tap away, as the design sets them.
const INKS: [[u8; 4]; 6] = [
    [0x26, 0x25, 0x2A, 0xFF],
    [0x2C, 0x3A, 0xA8, 0xFF],
    [0xC8, 0x39, 0x2B, 0xFF],
    [0x2E, 0x8A, 0x57, 0xFF],
    [0xE0, 0xA8, 0x45, 0xFF],
    [0x7A, 0x55, 0xB5, 0xFF],
];

/// Highlighter inks (the theme's five markers).
const MARKERS: [[u8; 4]; 5] = [
    [0xF2, 0xD5, 0x6B, 0xFF],
    [0x9B, 0xD6, 0x7C, 0xFF],
    [0xF2, 0x9B, 0xB0, 0xFF],
    [0x7F, 0xC4, 0xF0, 0xFF],
    [0xB7, 0x9C, 0xF2, 0xFF],
];

const fn color(rgba: [u8; 4]) -> Color {
    Color::from_argb(rgba[3], rgba[0], rgba[1], rgba[2])
}

/// A tool of the dock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pen,
    Pencil,
    Marker,
    Highlighter,
    Eraser,
    Shape,
    Lasso,
    Text,
    /// Select PDF text with the pen or mouse.
    Select,
}

impl Tool {
    const BRUSHES: [Self; 4] = [Self::Pen, Self::Pencil, Self::Marker, Self::Highlighter];

    /// The tool `board` is using.
    #[must_use]
    pub const fn of(board: &Board) -> Self {
        match board.input_mode() {
            InputMode::Lasso => Self::Lasso,
            InputMode::Text => Self::Text,
            InputMode::Select => Self::Select,
            InputMode::Draw | InputMode::Pan => match board.current_kind() {
                BrushKind::Pencil => Self::Pencil,
                BrushKind::Marker => Self::Marker,
                BrushKind::Highlighter => Self::Highlighter,
                BrushKind::Eraser => Self::Eraser,
                BrushKind::Shape(_) => Self::Shape,
                BrushKind::Pen | BrushKind::Custom(_) => Self::Pen,
            },
        }
    }

    fn icon(self) -> Bytes {
        use freya::icons::lucide;
        match self {
            Self::Pen => lucide::pen_line(),
            Self::Pencil => lucide::pencil(),
            Self::Marker => lucide::paintbrush(),
            Self::Highlighter => lucide::highlighter(),
            Self::Eraser => lucide::eraser(),
            Self::Shape => lucide::shapes(),
            Self::Lasso => lucide::lasso(),
            Self::Text => lucide::icon_type(),
            Self::Select => lucide::text_cursor_input(),
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Pen => "Pluma",
            Self::Pencil => "Lápiz",
            Self::Marker => "Marcador",
            Self::Highlighter => "Resaltador",
            Self::Eraser => "Goma",
            Self::Shape => "Formas",
            Self::Lasso => "Lazo",
            Self::Text => "Texto",
            Self::Select => "Seleccionar texto",
        }
    }

    const fn preset(self) -> Option<BrushPreset> {
        match self {
            Self::Pen => Some(BrushPreset::pen()),
            Self::Pencil => Some(BrushPreset::pencil()),
            Self::Marker => Some(BrushPreset::marker()),
            Self::Highlighter => Some(BrushPreset::highlighter()),
            Self::Eraser => Some(BrushPreset::eraser()),
            Self::Shape => Some(BrushPreset::shape(ShapeMode::Line)),
            Self::Lasso | Self::Text | Self::Select => None,
        }
    }

    /// Make this the board's tool.
    fn activate(self, board: &Arc<Mutex<Board>>) {
        let mut guard = lock(board);
        match self {
            Self::Lasso => guard.set_input_mode(InputMode::Lasso),
            Self::Text => guard.set_input_mode(InputMode::Text),
            Self::Select => guard.set_input_mode(InputMode::Select),
            _ => {
                if let Some(preset) = self.preset() {
                    let keep_shape = matches!(
                        (self, guard.current_kind()),
                        (Self::Shape, BrushKind::Shape(_))
                    );
                    if !keep_shape {
                        guard.choose_preset(preset);
                    }
                    guard.set_input_mode(InputMode::Draw);
                }
            }
        }
    }

    const fn tunable(self) -> bool {
        !matches!(self, Self::Lasso | Self::Text | Self::Select)
    }
}

fn anchor(e: &Event<PressEventData>) -> Area {
    let at = match e.data() {
        PressEventData::Mouse(m) => m.global_location,
        _ => CursorPoint::new(0.0, 0.0),
    };
    #[allow(clippy::cast_possible_truncation)] // window coordinates
    Area::new((at.x as f32, at.y as f32).into(), (1.0, 1.0).into())
}

/// Live state the chrome reads, kept in step with the board by its
/// redraw pump.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InkState {
    pub tool: State<Tool>,
    pub color: State<[u8; 4]>,
    pub can_undo: State<bool>,
    pub can_redo: State<bool>,
    pub zoom: State<f32>,
    pub read_only: State<bool>,
    pub handwriting: State<bool>,
    /// Lasso selection bounds in surface pixels.
    pub selection: State<Option<(f32, f32, f32, f32)>>,
    /// The canvas is over a PDF: text can be selected.
    pub pdf: State<bool>,
}

impl std::fmt::Debug for InkState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InkState").finish_non_exhaustive()
    }
}

impl InkState {
    /// Allocate the state for one canvas.
    #[must_use]
    pub fn use_state() -> Self {
        Self {
            tool: use_state(|| Tool::Pen),
            color: use_state(|| INKS[0]),
            can_undo: use_state(|| false),
            can_redo: use_state(|| false),
            zoom: use_state(|| 1.0),
            read_only: use_state(|| false),
            handwriting: use_state(|| false),
            selection: use_state(|| None),
            pdf: use_state(|| false),
        }
    }

    /// Copy what changed on `board` into the reactive state.
    pub fn sync(mut self, board: &Board) {
        let tool = Tool::of(board);
        if *self.tool.peek() != tool {
            self.tool.set(tool);
        }
        let ink = board.current_color();
        if *self.color.peek() != ink {
            self.color.set(ink);
        }
        let (undo, redo) = (board.undo_depth() > 0, board.redo_depth() > 0);
        if *self.can_undo.peek() != undo {
            self.can_undo.set(undo);
        }
        if *self.can_redo.peek() != redo {
            self.can_redo.set(redo);
        }
        let zoom = board.viewport().scale;
        if (zoom - *self.zoom.peek()).abs() > 0.001 {
            self.zoom.set(zoom);
        }
        if *self.read_only.peek() != board.is_read_only() {
            self.read_only.set(board.is_read_only());
        }
        if *self.handwriting.peek() != board.handwriting_enabled() {
            self.handwriting.set(board.handwriting_enabled());
        }
        let pdf = board.background().attribution_mode() == freya_canvas_bg::AttributionMode::PerPage;
        if *self.pdf.peek() != pdf {
            self.pdf.set(pdf);
        }
        let selection = board.selection_screen_bounds();
        if *self.selection.peek() != selection {
            self.selection.set(selection);
        }
    }
}

/// Every overlay of one canvas.
#[derive(Clone)]
pub struct Chrome {
    pub board: Arc<Mutex<Board>>,
    pub state: InkState,
    pub dock_mask: MaskSlot,
    pub zoom_mask: MaskSlot,
    pub bar_mask: MaskSlot,
    /// Extra bottom offset (text toolbar while editing).
    pub lift: f32,
    /// The pane showing this canvas, when it sits in a mesa: the dock
    /// floats only in the focused pane.
    pub slot: Option<crate::library::ViewSlot>,
}

impl PartialEq for Chrome {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board)
            && self.state == other.state
            && self.lift.to_bits() == other.lift.to_bits()
    }
}

impl std::fmt::Debug for Chrome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chrome").finish_non_exhaustive()
    }
}

impl Component for Chrome {
    fn render(&self) -> impl IntoElement {
        let settings = crate::settings::SettingsStore::get().read();
        let pen_seen = crate::pen_pump::use_pen_seen();
        let finger_draws = settings.pen.finger_draws.finger_draws(*pen_seen.read());
        lock(&self.board).set_finger_draws(finger_draws);
        let read_only = *self.state.read_only.read();
        let focused = self.slot.is_none_or(|slot| {
            crate::state::LibraryView::get()
                .data()
                .and_then(|d| {
                    d.workspace(slot.workspace)
                        .and_then(crate::screens::mesa::Focus::of)
                })
                .is_none_or(|focus| focus.pane == slot.pane)
        });
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .position(Position::new_absolute().top(0.0).left(0.0))
            .layer(Layer::Overlay)
            .maybe_child((!read_only && focused).then(|| {
                Dock {
                    board: Arc::clone(&self.board),
                    state: self.state,
                    mask: self.dock_mask,
                    lift: self.lift,
                    mover: finger_draws,
                }
                .into_element()
            }))
            .maybe_child((*self.state.selection.read()).map(|bounds| {
                LassoBar {
                    board: Arc::clone(&self.board),
                    bounds,
                    mask: self.bar_mask,
                }
                .into_element()
            }))
            .child(Zoom {
                board: Arc::clone(&self.board),
                zoom: self.state.zoom,
                mask: self.zoom_mask,
                lift: self.lift,
            })
    }
}

/// The dock of tools.
#[derive(Clone)]
struct Dock {
    board: Arc<Mutex<Board>>,
    state: InkState,
    mask: MaskSlot,
    lift: f32,
    /// Show "Mover": fingers write, holding it pans.
    mover: bool,
}

impl PartialEq for Dock {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board)
            && self.state == other.state
            && self.mover == other.mover
            && self.lift.to_bits() == other.lift.to_bits()
    }
}

fn dock_button(icon: Bytes, label: &'static str, on: bool, tint: Option<Color>) -> Rect {
    let p = palette();
    rect()
        .width(Size::px(40.0))
        .height(Size::px(40.0))
        .corner_radius(tokens::radius::MD)
        .background(if on {
            p.accent_soft
        } else {
            Color::TRANSPARENT
        })
        .center()
        .a11y_role(AccessibilityRole::Button)
        .a11y_alt(label)
        .a11y_focusable(true)
        .child(Icon::new(icon).size(20.0).color(tint.unwrap_or(if on {
            p.accent_text
        } else {
            p.t1
        })))
}

fn divider() -> Rect {
    let p = palette();
    rect()
        .width(Size::px(1.0))
        .height(Size::px(24.0))
        .background(p.line)
}

impl Component for Dock {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let compact = width_class().is_compact();
        let tool = *self.state.tool.read();
        let ink = *self.state.color.read();
        let board = &self.board;
        let tool_button = |t: Tool| {
            let board = Arc::clone(board);
            let on = t == tool;
            let tint = (t == Tool::Highlighter && on).then(|| color(ink));
            dock_button(t.icon(), t.label(), on, tint)
                .on_press(move |e: Event<PressEventData>| {
                    if on && t.tunable() {
                        BrushPopover::open(Arc::clone(&board), t, anchor(&e));
                    } else {
                        t.activate(&board);
                    }
                })
                .into_element()
        };
        let mover = self.mover.then(|| {
            let down = Arc::clone(board);
            let up = Arc::clone(board);
            dock_button(lucide::hand(), "Mover (mantené apretado)", false, None)
                .on_pointer_down(move |_| lock(&down).set_pan_hold(true))
                .on_global_pointer_up(move |_| lock(&up).set_pan_hold(false))
                .into_element()
        });
        let swatch = {
            let board = Arc::clone(board);
            rect()
                .width(Size::px(40.0))
                .height(Size::px(40.0))
                .center()
                .a11y_role(AccessibilityRole::Button)
                .a11y_alt("Color")
                .on_press(move |e: Event<PressEventData>| {
                    ColorPopover::open(Arc::clone(&board), anchor(&e));
                })
                .child(
                    rect()
                        .width(Size::px(24.0))
                        .height(Size::px(24.0))
                        .corner_radius(tokens::radius::FULL)
                        .background(color(ink))
                        .border(
                            Border::new()
                                .fill(p.t1)
                                .width(2.0)
                                .alignment(BorderAlignment::Outer),
                        ),
                )
                .into_element()
        };
        let undo = {
            let board = Arc::clone(board);
            IconButton::new(lucide::undo_2(), "Deshacer")
                .enabled(*self.state.can_undo.read())
                .on_press(move |_| {
                    lock(&board).undo();
                })
                .into_element()
        };
        let redo = {
            let board = Arc::clone(board);
            IconButton::new(lucide::redo_2(), "Rehacer")
                .enabled(*self.state.can_redo.read())
                .on_press(move |_| {
                    lock(&board).redo();
                })
                .into_element()
        };
        let mut items: Vec<Element> = Vec::new();
        if let Some(mover) = mover {
            items.push(mover);
            items.push(divider().into_element());
        }
        if compact {
            for t in [Tool::Pen, Tool::Highlighter, Tool::Eraser, Tool::Lasso] {
                items.push(tool_button(t));
            }
            items.push(swatch);
            items.push(undo);
            let board = Arc::clone(board);
            let state = self.state;
            items.push(
                IconButton::new(lucide::ellipsis(), "Más herramientas")
                    .on_press(move |_| more_sheet(Arc::clone(&board), state))
                    .into_element(),
            );
        } else {
            for t in Tool::BRUSHES {
                items.push(tool_button(t));
            }
            items.push(tool_button(Tool::Eraser));
            items.push(tool_button(Tool::Shape));
            items.push(divider().into_element());
            items.push(tool_button(Tool::Lasso));
            if *self.state.pdf.read() {
                items.push(tool_button(Tool::Select));
            }
            items.push(tool_button(Tool::Text));
            let signature = {
                let board = Arc::clone(board);
                let on = *self.state.handwriting.read();
                dock_button(lucide::signature(), "Escritura suavizada", on, None)
                    .on_press(move |_| {
                        let mut guard = lock(&board);
                        let next = !guard.handwriting_enabled();
                        guard.set_handwriting(next);
                    })
                    .into_element()
            };
            items.push(signature);
            items.push(divider().into_element());
            items.push(swatch);
            items.push(divider().into_element());
            items.push(undo);
            items.push(redo);
            let board = Arc::clone(board);
            items.push(
                IconButton::new(lucide::layers(), "Capas")
                    .on_press(move |e: Event<PressEventData>| {
                        Layers::open(Arc::clone(&board), anchor(&e));
                    })
                    .into_element(),
            );
        }
        let mask = self.mask;
        rect()
            .position(Position::new_absolute().left(0.0).bottom(16.0 + self.lift))
            .width(Size::fill())
            .horizontal()
            .main_align(Alignment::Center)
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(2.0)
                    .padding((4.0, 6.0))
                    .corner_radius(tokens::radius::XL)
                    .background(p.s2)
                    .border(
                        Border::new()
                            .fill(p.line)
                            .width(1.0)
                            .alignment(BorderAlignment::Inner),
                    )
                    .shadow(Shadow::new().y(8.0).blur(24.0).color(p.shadow))
                    .on_sized(move |e: Event<SizedEventData>| mask.publish(e.area))
                    .children(items),
            )
    }
}

/// The tools the phone dock leaves out.
fn more_sheet(board: Arc<Mutex<Board>>, state: InkState) {
    use freya::icons::lucide;
    let mut tools = vec![Tool::Pencil, Tool::Marker, Tool::Shape, Tool::Text];
    if *state.pdf.peek() {
        tools.push(Tool::Select);
    }
    let rows: Vec<Element> = tools
        .into_iter()
        .map(|t| {
            let board = Arc::clone(&board);
            notein_ui_kit::ListRow::new(t.label())
                .icon(t.icon())
                .selected(*state.tool.peek() == t)
                .on_press(move |_| {
                    notein_ui_kit::ModalController::get().close();
                    t.activate(&board);
                })
                .into_element()
        })
        .chain([
            {
                let board = Arc::clone(&board);
                notein_ui_kit::ListRow::new("Rehacer")
                    .icon(lucide::redo_2())
                    .enabled(*state.can_redo.peek())
                    .on_press(move |_| {
                        notein_ui_kit::ModalController::get().close();
                        lock(&board).redo();
                    })
                    .into_element()
            },
            {
                let board = Arc::clone(&board);
                notein_ui_kit::ListRow::new("Escritura suavizada")
                    .icon(lucide::signature())
                    .trailing(notein_ui_kit::Trailing::Switch {
                        on: *state.handwriting.peek(),
                        on_toggle: EventHandler::new(move |on| lock(&board).set_handwriting(on)),
                    })
                    .into_element()
            },
            {
                let board = Arc::clone(&board);
                notein_ui_kit::ListRow::new("Capas")
                    .icon(lucide::layers())
                    .chevron()
                    .on_press(move |_| {
                        let size = *Platform::get().root_size.read();
                        Layers::open(
                            Arc::clone(&board),
                            Area::new((size.width / 2.0, size.height).into(), (1.0, 1.0).into()),
                        );
                    })
                    .into_element()
            },
        ])
        .collect();
    notein_ui_kit::Sheet::titled("Herramientas")
        .child(notein_ui_kit::Group::default().children(rows))
        .open();
}

/// Tuning of the active tool: size, colour and its own options.
#[derive(Clone)]
struct BrushPopover {
    board: Arc<Mutex<Board>>,
    tool: Tool,
}

impl PartialEq for BrushPopover {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board) && self.tool == other.tool
    }
}

impl BrushPopover {
    fn open(board: Arc<Mutex<Board>>, tool: Tool, at: Area) {
        Popover::new(at, Self { board, tool })
            .width(300.0)
            .side(PopoverSide::Above)
            .align(PopoverAlign::Center)
            .estimated_height(260.0)
            .sheet_title(tool.label())
            .open();
    }
}

impl Component for BrushPopover {
    fn render(&self) -> impl IntoElement {
        let revision = use_state(|| 0u32);
        let _ = revision.read();
        let bump = move || {
            let mut revision = revision;
            let next = revision.peek().wrapping_add(1);
            revision.set(next);
        };
        let guard = lock(&self.board);
        let kind = guard.current_kind();
        let size = guard.current_size();
        let ink = guard.current_color();
        let config = guard.brush_config(kind);
        drop(guard);
        let board = Arc::clone(&self.board);
        let size_board = Arc::clone(&self.board);
        let percent = f64::from(
            (size - crate::canvas::SIZE_SCALE_MIN)
                / (crate::canvas::SIZE_SCALE_MAX - crate::canvas::SIZE_SCALE_MIN)
                * 100.0,
        );
        let colors: &[[u8; 4]] = if self.tool == Tool::Highlighter {
            &MARKERS
        } else {
            &INKS
        };
        let swatches = rect()
            .horizontal()
            .spacing(8.0)
            .children(colors.iter().map(|c| {
                let c = *c;
                let board = Arc::clone(&board);
                let on = c == ink;
                rect()
                    .width(Size::px(28.0))
                    .height(Size::px(28.0))
                    .corner_radius(tokens::radius::FULL)
                    .background(color(c))
                    .border(
                        Border::new()
                            .fill(if on { palette().t1 } else { Color::TRANSPARENT })
                            .width(2.0)
                            .alignment(BorderAlignment::Outer),
                    )
                    .a11y_role(AccessibilityRole::RadioButton)
                    .on_press(move |_| {
                        lock(&board).set_current_color(c);
                        bump();
                    })
                    .into_element()
            }));
        let specific: Option<Element> = match (self.tool, config) {
            (Tool::Pen | Tool::Pencil | Tool::Marker, BrushConfig::Pen { curve, .. }) => {
                let options = [
                    (PressureCurve::Linear, "Lineal"),
                    (PressureCurve::Soft, "Suave"),
                    (PressureCurve::Hard, "Dura"),
                ];
                let selected = options.iter().position(|(c, _)| *c == curve).unwrap_or(0);
                let board = Arc::clone(&board);
                Some(
                    rect()
                        .width(Size::fill())
                        .spacing(6.0)
                        .child(Text::new("Presión").role(TextRole::Label))
                        .child(
                            Segmented::new(options.map(|(_, l)| l), selected)
                                .full_width()
                                .on_select(move |i: usize| {
                                    let mut guard = lock(&board);
                                    let mut next = guard.brush_config(kind);
                                    if let BrushConfig::Pen { curve, .. } = &mut next {
                                        *curve = options[i].0;
                                    }
                                    guard.set_brush_config(kind, next);
                                    drop(guard);
                                    bump();
                                }),
                        )
                        .into_element(),
                )
            }
            (Tool::Highlighter, BrushConfig::Highlighter { tip, .. }) => {
                let bevel = matches!(tip, HighlighterTip::Bevel { .. });
                let board = Arc::clone(&board);
                Some(
                    rect()
                        .width(Size::fill())
                        .spacing(6.0)
                        .child(Text::new("Punta").role(TextRole::Label))
                        .child(
                            Segmented::new(["Redonda", "Bisel"], usize::from(bevel))
                                .full_width()
                                .on_select(move |i: usize| {
                                    let mut guard = lock(&board);
                                    let mut next = guard.brush_config(kind);
                                    if let BrushConfig::Highlighter { tip, .. } = &mut next {
                                        *tip = if i == 1 {
                                            HighlighterTip::Bevel { angle_deg: 30.0 }
                                        } else {
                                            HighlighterTip::Round
                                        };
                                    }
                                    guard.set_brush_config(kind, next);
                                    drop(guard);
                                    bump();
                                }),
                        )
                        .into_element(),
                )
            }
            (Tool::Eraser, BrushConfig::Eraser { mode, .. }) => {
                let board = Arc::clone(&board);
                Some(
                    rect()
                        .width(Size::fill())
                        .spacing(6.0)
                        .child(Text::new("Borra").role(TextRole::Label))
                        .child(
                            Segmented::new(
                                ["Punto", "Trazo"],
                                usize::from(mode == EraserMode::Stroke),
                            )
                            .full_width()
                            .on_select(move |i: usize| {
                                let mut guard = lock(&board);
                                let mut next = guard.brush_config(kind);
                                if let BrushConfig::Eraser { mode, .. } = &mut next {
                                    *mode = if i == 1 {
                                        EraserMode::Stroke
                                    } else {
                                        EraserMode::Point
                                    };
                                }
                                guard.set_brush_config(kind, next);
                                drop(guard);
                                bump();
                            }),
                        )
                        .into_element(),
                )
            }
            (Tool::Shape, _) => {
                let modes = [
                    (ShapeMode::Line, "Línea"),
                    (ShapeMode::Dashed, "Guiones"),
                    (ShapeMode::Arrow, "Flecha"),
                    (ShapeMode::Rect, "Rect."),
                    (ShapeMode::Circle, "Círculo"),
                ];
                let current = match kind {
                    BrushKind::Shape(m) => m,
                    _ => ShapeMode::Line,
                };
                let selected = modes.iter().position(|(m, _)| *m == current).unwrap_or(0);
                let board = Arc::clone(&board);
                Some(
                    rect()
                        .width(Size::fill())
                        .spacing(6.0)
                        .child(Text::new("Forma").role(TextRole::Label))
                        .child(
                            Segmented::new(modes.map(|(_, l)| l), selected)
                                .full_width()
                                .on_select(move |i: usize| {
                                    let mut guard = lock(&board);
                                    let keep = guard.current_color();
                                    guard.choose_preset(BrushPreset::shape(modes[i].0));
                                    guard.set_current_color(keep);
                                    drop(guard);
                                    bump();
                                }),
                        )
                        .into_element(),
                )
            }
            _ => None,
        };
        rect()
            .width(Size::fill())
            .padding(14.0)
            .spacing(14.0)
            .child(
                rect()
                    .width(Size::fill())
                    .spacing(6.0)
                    .child(Text::new("Grosor").role(TextRole::Label))
                    .child(
                        Slider::new(move |value: f64| {
                            #[allow(clippy::cast_possible_truncation)] // a percentage
                            let t = (value / 100.0) as f32;
                            let scale = crate::canvas::SIZE_SCALE_MIN
                                + t * (crate::canvas::SIZE_SCALE_MAX
                                    - crate::canvas::SIZE_SCALE_MIN);
                            lock(&size_board).set_current_size(scale);
                        })
                        .value(percent),
                    ),
            )
            .maybe_child((self.tool != Tool::Eraser).then(|| {
                rect()
                    .width(Size::fill())
                    .spacing(6.0)
                    .child(Text::new("Color").role(TextRole::Label))
                    .child(swatches)
            }))
            .maybe_child(specific)
    }
}

/// Every ink colour plus a picker.
#[derive(Clone)]
struct ColorPopover {
    board: Arc<Mutex<Board>>,
}

impl PartialEq for ColorPopover {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board)
    }
}

impl ColorPopover {
    fn open(board: Arc<Mutex<Board>>, at: Area) {
        Popover::new(at, Self { board })
            .width(300.0)
            .side(PopoverSide::Above)
            .align(PopoverAlign::Center)
            .estimated_height(320.0)
            .sheet_title("Color")
            .open();
    }
}

impl Component for ColorPopover {
    fn render(&self) -> impl IntoElement {
        let board = Arc::clone(&self.board);
        let current = lock(&self.board).current_color();
        let swatches: Vec<Color> = INKS
            .iter()
            .chain(MARKERS.iter())
            .map(|c| color(*c))
            .collect();
        rect().padding(12.0).child(
            notein_ui_kit::ColorWheel::new()
                .swatches(swatches)
                .allow_custom(true)
                .initial(color(current))
                .width(276.0)
                .on_change(move |c: Color| {
                    lock(&board).set_current_color([c.r(), c.g(), c.b(), 0xFF]);
                }),
        )
    }
}

/// Actions over a lasso selection, floating above it.
#[derive(Clone)]
struct LassoBar {
    board: Arc<Mutex<Board>>,
    bounds: (f32, f32, f32, f32),
    mask: MaskSlot,
}

impl PartialEq for LassoBar {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board) && self.bounds == other.bounds
    }
}

impl Component for LassoBar {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let board = &self.board;
        let b = |f: fn(&mut Board)| {
            let board = Arc::clone(board);
            move |()| f(&mut lock(&board))
        };
        let color_board = Arc::clone(board);
        let text_board = Arc::clone(board);
        let items = vec![
            BarItem::action("Mover", lucide::hand_grab(), |()| {
                Toasts::get().show(Toast::new("Arrastrá la selección para moverla"));
            }),
            BarItem::Swatches {
                colors: INKS.iter().map(|c| color(*c)).collect(),
                active: None,
                on_pick: EventHandler::new(move |i: usize| {
                    lock(&color_board).selection_recolor(INKS[i]);
                }),
            },
            BarItem::action("Duplicar", lucide::copy(), b(Board::selection_duplicate)),
            BarItem::action("A texto", lucide::icon_type(), move |()| {
                let read = lock(&text_board).selection_to_text();
                if read.is_none() {
                    Toasts::get().show(Toast::new("No pudimos leer esa letra"));
                }
            }),
            BarItem::Divider,
            BarItem::action("Borrar", lucide::trash_2(), b(Board::selection_delete)),
        ];
        let (x0, y0, x1, _) = self.bounds;
        let mask = self.mask;
        let top = (y0 - 64.0).max(8.0);
        rect()
            .position(
                Position::new_absolute()
                    .left((f32::midpoint(x0, x1) - 220.0).max(8.0))
                    .top(top),
            )
            .on_sized(move |e: Event<SizedEventData>| mask.publish(e.area))
            .child(SelectionBar::new(items))
    }
}

/// Zoom percentage; tapping it fits the page again.
#[derive(Clone)]
struct Zoom {
    board: Arc<Mutex<Board>>,
    zoom: State<f32>,
    mask: MaskSlot,
    lift: f32,
}

impl PartialEq for Zoom {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board) && self.lift.to_bits() == other.lift.to_bits()
    }
}

impl Component for Zoom {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        #[allow(clippy::cast_possible_truncation)] // a percentage
        let pct = (*self.zoom.read() * 100.0).round() as i32;
        let board = Arc::clone(&self.board);
        let mask = self.mask;
        rect()
            .position(Position::new_absolute().right(12.0).top(12.0))
            .padding((6.0, 10.0))
            .corner_radius(tokens::radius::FULL)
            .background(p.s2)
            .border(
                Border::new()
                    .fill(p.line)
                    .width(1.0)
                    .alignment(BorderAlignment::Inner),
            )
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt("Volver al 100 %")
            .on_sized(move |e: Event<SizedEventData>| mask.publish(e.area))
            .on_press(move |_| lock(&board).reset_viewport())
            .child(Text::new(format!("{pct} %")).role(TextRole::Meta))
    }
}

/// The layers of the canvas.
#[derive(Clone)]
struct Layers {
    board: Arc<Mutex<Board>>,
}

impl PartialEq for Layers {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board)
    }
}

impl Layers {
    fn open(board: Arc<Mutex<Board>>, at: Area) {
        Popover::new(at, Self { board })
            .width(320.0)
            .side(PopoverSide::Above)
            .align(PopoverAlign::End)
            .estimated_height(320.0)
            .sheet_title("Capas")
            .open();
    }
}

impl Component for Layers {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let revision = use_state(|| 0u32);
        let _ = revision.read();
        let bump = move || {
            let mut revision = revision;
            let next = revision.peek().wrapping_add(1);
            revision.set(next);
        };
        let guard = lock(&self.board);
        let mut layers = guard.layers_snapshot();
        let active = guard.active_layer_id();
        drop(guard);
        layers.reverse();
        let rows: Vec<Element> = layers
            .into_iter()
            .map(|layer| {
                let id = layer.id;
                let (vis, lock_board, pick) = (
                    Arc::clone(&self.board),
                    Arc::clone(&self.board),
                    Arc::clone(&self.board),
                );
                notein_ui_kit::ListRow::new(layer.name.clone())
                    .subtitle(crate::format::count(layer.stroke_count, "trazo", "trazos"))
                    .selected(id == active)
                    .dense(true)
                    .on_press(move |_| {
                        lock(&pick).set_active_layer(id);
                        bump();
                    })
                    .trailing(notein_ui_kit::Trailing::Element(
                        rect()
                            .horizontal()
                            .spacing(2.0)
                            .child(
                                IconButton::new(
                                    if layer.visible {
                                        lucide::eye()
                                    } else {
                                        lucide::eye_off()
                                    },
                                    "Mostrar u ocultar",
                                )
                                .size(tokens::size::BUTTON_SM)
                                .on_press(move |_| {
                                    lock(&vis).set_layer_visible(id, !layer.visible);
                                    bump();
                                }),
                            )
                            .child(
                                IconButton::new(
                                    if layer.locked {
                                        lucide::lock()
                                    } else {
                                        lucide::lock_open()
                                    },
                                    "Bloquear",
                                )
                                .size(tokens::size::BUTTON_SM)
                                .on_press(move |_| {
                                    lock(&lock_board).set_layer_locked(id, !layer.locked);
                                    bump();
                                }),
                            )
                            .into_element(),
                    ))
                    .into_element()
            })
            .collect();
        let add = Arc::clone(&self.board);
        let remove = Arc::clone(&self.board);
        rect()
            .width(Size::fill())
            .padding(8.0)
            .spacing(8.0)
            .child(notein_ui_kit::Group::default().children(rows))
            .child(
                rect()
                    .horizontal()
                    .spacing(8.0)
                    .child(
                        notein_ui_kit::Button::new("Nueva capa")
                            .tonal()
                            .small()
                            .icon(lucide::plus())
                            .on_press(move |_| {
                                lock(&add).add_layer();
                                bump();
                            }),
                    )
                    .child(
                        notein_ui_kit::Button::new("Borrar capa")
                            .ghost()
                            .small()
                            .on_press(move |_| {
                                lock(&remove).remove_active_layer();
                                bump();
                            }),
                    ),
            )
            .child(
                Text::new("La capa elegida recibe lo que escribís.")
                    .role(TextRole::Hint)
                    .color(p.t3),
            )
    }
}

/// A bookmark picked on the canvas opens as an annotation: its note,
/// its colour, and Borrar.
#[derive(Clone)]
pub struct PinNotes {
    pub board: Arc<Mutex<Board>>,
    pub selected: State<Option<crate::ids::BookmarkId>>,
    /// Window position of the canvas, to point the popover at the pin.
    pub origin: (f32, f32),
    /// Document the canvas belongs to.
    pub item: Option<crate::library::ItemId>,
}

impl PartialEq for PinNotes {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board) && self.selected == other.selected && self.origin == other.origin
    }
}

impl std::fmt::Debug for PinNotes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinNotes").finish_non_exhaustive()
    }
}

impl Component for PinNotes {
    fn render(&self) -> impl IntoElement {
        let mut shown = use_state(|| None::<crate::ids::BookmarkId>);
        let current = *self.selected.read();
        if current != *shown.peek() {
            shown.set(current);
            if let Some(id) = current {
                self.open(id);
            }
        }
        rect()
    }
}

impl PinNotes {
    fn open(&self, id: crate::ids::BookmarkId) {
        use notein_plugin::services::{AnnotationOutcome, AnnotationRequest};
        let guard = lock(&self.board);
        let Some(bookmark) = guard.doc().bookmark(id).cloned() else {
            return;
        };
        let world = guard.bookmark_world_position(&bookmark);
        let (sx, sy) = guard.world_to_screen(world.x, world.y);
        drop(guard);
        let quote = bookmark.refs.iter().find_map(|r| match r {
            notein_plugin::bookmark::SourceRef::Plugin { selection, .. } => selection.clone(),
            _ => None,
        });
        let (tx, rx) = flume::bounded::<AnnotationOutcome>(1);
        let board = Arc::clone(&self.board);
        let mut selected = self.selected;
        let refs = bookmark.refs.clone();
        spawn_forever(async move {
            let outcome = rx.recv_async().await.unwrap_or(AnnotationOutcome::Cancelled);
            {
                let mut guard = lock(&board);
                match outcome {
                    AnnotationOutcome::Saved { color, body } => {
                        guard.update_bookmark(id, body, refs, color.map(|c| [c[0], c[1], c[2], 0xFF]), crate::format::now());
                    }
                    AnnotationOutcome::Deleted => {
                        guard.delete_bookmark(id);
                    }
                    AnnotationOutcome::Cancelled => {}
                }
            }
            selected.set(None);
        });
        crate::annotation::AnnotationPopover::open(
            AnnotationRequest {
                doc: self.item.map_or(notein_plugin::doc::DocId([0; 16]), crate::plugins::content::doc_id),
                kind: crate::plugins::builtin::canvas::KIND.to_owned(),
                title: "Marcador".to_owned(),
                quote,
                color: bookmark.color.map(|c| [c[0], c[1], c[2]]),
                body: bookmark.body,
                target: Some((self.origin.0 + sx - 6.0, self.origin.1 + sy - 6.0, 12.0, 12.0)),
                existing: true,
            },
            None,
            std::sync::Arc::new(move |outcome| {
                let _ = tx.send(outcome);
            }),
        );
    }
}

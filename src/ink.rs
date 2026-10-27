//! What floats over a canvas: the ink dock with every tool, the
//! popover that tunes the active one, the bar over a lasso selection,
//! the layers list and the zoom pill.

use std::sync::{Arc, Mutex};

use freya::prelude::*;
use notein_ui_kit::TapExt;
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
    notein_ui_kit::press_anchor(e.data())
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
        let pdf =
            board.background().attribution_mode() == freya_canvas_bg::AttributionMode::PerPage;
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
                level: self.state.zoom,
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

impl Dock {
    /// A tool; pressing the active one tunes it.
    fn tool_button(&self, t: Tool) -> Element {
        let board = Arc::clone(&self.board);
        let on = t == *self.state.tool.read();
        let tint = (t == Tool::Highlighter && on).then(|| color(*self.state.color.read()));
        dock_button(t.icon(), t.label(), on, tint)
            .on_tap(move |e: Event<PressEventData>| {
                if on && t.tunable() {
                    BrushPopover::open(Arc::clone(&board), t, anchor(&e));
                } else {
                    t.activate(&board);
                }
            })
            .into_element()
    }

    /// Hold to pan with the pen, for touchscreens without fingers.
    fn mover(&self) -> Element {
        let (down, up) = (Arc::clone(&self.board), Arc::clone(&self.board));
        dock_button(
            freya::icons::lucide::hand(),
            "Mover (mantené apretado)",
            false,
            None,
        )
        .on_pointer_down(move |_| lock(&down).set_pan_hold(true))
        .on_global_pointer_up(move |_| lock(&up).set_pan_hold(false))
        .into_element()
    }

    /// The current ink; pressing opens the colours.
    fn swatch(&self) -> Element {
        let p = palette();
        let board = Arc::clone(&self.board);
        rect()
            .width(Size::px(40.0))
            .height(Size::px(40.0))
            .center()
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt("Color")
            .on_tap(move |e: Event<PressEventData>| {
                ColorPopover::open(Arc::clone(&board), anchor(&e));
            })
            .child(
                rect()
                    .width(Size::px(24.0))
                    .height(Size::px(24.0))
                    .corner_radius(tokens::radius::FULL)
                    .background(color(*self.state.color.read()))
                    .border(
                        Border::new()
                            .fill(p.t1)
                            .width(2.0)
                            .alignment(BorderAlignment::Outer),
                    ),
            )
            .into_element()
    }

    fn undo(&self) -> Element {
        let board = Arc::clone(&self.board);
        IconButton::new(freya::icons::lucide::undo_2(), "Deshacer")
            .enabled(*self.state.can_undo.read())
            .on_press(move |_| {
                lock(&board).undo();
            })
            .into_element()
    }

    fn redo(&self) -> Element {
        let board = Arc::clone(&self.board);
        IconButton::new(freya::icons::lucide::redo_2(), "Rehacer")
            .enabled(*self.state.can_redo.read())
            .on_press(move |_| {
                lock(&board).redo();
            })
            .into_element()
    }

    /// Phones: the everyday tools; the rest live under "Más".
    fn phone_items(&self) -> Vec<Element> {
        let mut items: Vec<Element> = [Tool::Pen, Tool::Highlighter, Tool::Eraser, Tool::Lasso]
            .into_iter()
            .map(|t| self.tool_button(t))
            .collect();
        items.push(self.swatch());
        items.push(self.undo());
        let (board, state) = (Arc::clone(&self.board), self.state);
        items.push(
            IconButton::new(freya::icons::lucide::ellipsis(), "Más herramientas")
                .on_press(move |_| more_sheet(&board, &state))
                .into_element(),
        );
        items
    }

    /// Wider screens: every tool, grouped.
    fn wide_items(&self) -> Vec<Element> {
        use freya::icons::lucide;
        let mut items: Vec<Element> = Tool::BRUSHES
            .into_iter()
            .map(|t| self.tool_button(t))
            .collect();
        items.push(self.tool_button(Tool::Eraser));
        items.push(self.tool_button(Tool::Shape));
        items.push(divider().into_element());
        items.push(self.tool_button(Tool::Lasso));
        if *self.state.pdf.read() {
            items.push(self.tool_button(Tool::Select));
        }
        items.push(self.tool_button(Tool::Text));
        let board = Arc::clone(&self.board);
        items.push(
            dock_button(
                lucide::signature(),
                "Escritura suavizada",
                *self.state.handwriting.read(),
                None,
            )
            .on_tap(move |_| {
                let mut guard = lock(&board);
                let next = !guard.handwriting_enabled();
                guard.set_handwriting(next);
            })
            .into_element(),
        );
        items.push(divider().into_element());
        items.push(self.swatch());
        items.push(divider().into_element());
        items.push(self.undo());
        items.push(self.redo());
        let board = Arc::clone(&self.board);
        items.push(
            IconButton::new(lucide::layers(), "Capas")
                .on_press(move |e: Event<PressEventData>| {
                    Layers::open(Arc::clone(&board), anchor(&e));
                })
                .into_element(),
        );
        items
    }
}

impl Component for Dock {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut items: Vec<Element> = Vec::new();
        if self.mover {
            items.push(self.mover());
            items.push(divider().into_element());
        }
        items.extend(if width_class().is_compact() {
            self.phone_items()
        } else {
            self.wide_items()
        });
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
fn more_sheet(board: &Arc<Mutex<Board>>, state: &InkState) {
    use freya::icons::lucide;
    let mut tools = vec![Tool::Pencil, Tool::Marker, Tool::Shape, Tool::Text];
    if *state.pdf.peek() {
        tools.push(Tool::Select);
    }
    let rows: Vec<Element> = tools
        .into_iter()
        .map(|t| {
            let board = Arc::clone(board);
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
                let board = Arc::clone(board);
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
                let board = Arc::clone(board);
                notein_ui_kit::ListRow::new("Escritura suavizada")
                    .icon(lucide::signature())
                    .trailing(notein_ui_kit::Trailing::Switch {
                        on: *state.handwriting.peek(),
                        on_toggle: EventHandler::new(move |on| lock(&board).set_handwriting(on)),
                    })
                    .into_element()
            },
            {
                let board = Arc::clone(board);
                notein_ui_kit::ListRow::new("Capas")
                    .icon(lucide::layers())
                    .chevron()
                    .on_press(move |_| {
                        let size = *Platform::get().current_window().root_size.read();
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

/// A labelled choice between a few options.
fn choice(
    title: &'static str,
    labels: impl IntoIterator<Item = &'static str>,
    selected: usize,
    on_select: impl FnMut(usize) + 'static,
) -> Element {
    rect()
        .width(Size::fill())
        .spacing(6.0)
        .child(Text::new(title).role(TextRole::Label))
        .child(
            Segmented::new(labels, selected)
                .full_width()
                .on_select(on_select),
        )
        .into_element()
}

/// Change the settings of brush `kind`.
fn edit_config(board: &Arc<Mutex<Board>>, kind: BrushKind, edit: impl FnOnce(&mut BrushConfig)) {
    let mut guard = lock(board);
    let mut next = guard.brush_config(kind);
    edit(&mut next);
    guard.set_brush_config(kind, next);
}

impl BrushPopover {
    /// The ink swatches of the tool (markers for the highlighter).
    fn swatches(&self, ink: [u8; 4], bump: impl Fn() + Copy + 'static) -> Rect {
        let colors: &[[u8; 4]] = if self.tool == Tool::Highlighter {
            &MARKERS
        } else {
            &INKS
        };
        rect()
            .horizontal()
            .spacing(8.0)
            .children(colors.iter().map(|&c| {
                let board = Arc::clone(&self.board);
                rect()
                    .width(Size::px(28.0))
                    .height(Size::px(28.0))
                    .corner_radius(tokens::radius::FULL)
                    .background(color(c))
                    .border(
                        Border::new()
                            .fill(if c == ink {
                                palette().t1
                            } else {
                                Color::TRANSPARENT
                            })
                            .width(2.0)
                            .alignment(BorderAlignment::Outer),
                    )
                    .a11y_role(AccessibilityRole::RadioButton)
                    .on_tap(move |_| {
                        lock(&board).set_current_color(c);
                        bump();
                    })
                    .into_element()
            }))
    }

    /// What only this tool has: pressure, tip, what the eraser takes,
    /// or the shape.
    fn specific(
        &self,
        kind: BrushKind,
        config: BrushConfig,
        bump: impl Fn() + Copy + 'static,
    ) -> Option<Element> {
        let board = Arc::clone(&self.board);
        match (self.tool, config) {
            (Tool::Pen | Tool::Pencil | Tool::Marker, BrushConfig::Pen { curve, .. }) => {
                let curves = [
                    PressureCurve::Linear,
                    PressureCurve::Soft,
                    PressureCurve::Hard,
                ];
                let selected = curves.iter().position(|c| *c == curve).unwrap_or(0);
                Some(choice(
                    "Presión",
                    ["Lineal", "Suave", "Dura"],
                    selected,
                    move |i| {
                        edit_config(&board, kind, |config| {
                            if let BrushConfig::Pen { curve, .. } = config {
                                *curve = curves[i];
                            }
                        });
                        bump();
                    },
                ))
            }
            (Tool::Highlighter, BrushConfig::Highlighter { tip, .. }) => {
                let bevel = matches!(tip, HighlighterTip::Bevel { .. });
                Some(choice(
                    "Punta",
                    ["Redonda", "Bisel"],
                    usize::from(bevel),
                    move |i| {
                        edit_config(&board, kind, |config| {
                            if let BrushConfig::Highlighter { tip, .. } = config {
                                *tip = if i == 1 {
                                    HighlighterTip::Bevel { angle_deg: 30.0 }
                                } else {
                                    HighlighterTip::Round
                                };
                            }
                        });
                        bump();
                    },
                ))
            }
            (Tool::Eraser, BrushConfig::Eraser { mode, .. }) => {
                let stroke = usize::from(mode == EraserMode::Stroke);
                Some(choice("Borra", ["Punto", "Trazo"], stroke, move |i| {
                    edit_config(&board, kind, |config| {
                        if let BrushConfig::Eraser { mode, .. } = config {
                            *mode = if i == 1 {
                                EraserMode::Stroke
                            } else {
                                EraserMode::Point
                            };
                        }
                    });
                    bump();
                }))
            }
            (Tool::Shape, _) => {
                let modes = [
                    ShapeMode::Line,
                    ShapeMode::Dashed,
                    ShapeMode::Arrow,
                    ShapeMode::Rect,
                    ShapeMode::Circle,
                ];
                let current = match kind {
                    BrushKind::Shape(m) => m,
                    _ => ShapeMode::Line,
                };
                let selected = modes.iter().position(|m| *m == current).unwrap_or(0);
                let labels = ["Línea", "Guiones", "Flecha", "Rect.", "Círculo"];
                Some(choice("Forma", labels, selected, move |i| {
                    let mut guard = lock(&board);
                    let keep = guard.current_color();
                    guard.choose_preset(BrushPreset::shape(modes[i]));
                    guard.set_current_color(keep);
                    drop(guard);
                    bump();
                }))
            }
            _ => None,
        }
    }
}

impl Component for BrushPopover {
    fn render(&self) -> impl IntoElement {
        let revision = use_state(|| 0u32);
        // Re-render after each change made here; the board is not reactive.
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
        let size_board = Arc::clone(&self.board);
        let percent = f64::from(
            (size - crate::canvas::SIZE_SCALE_MIN)
                / (crate::canvas::SIZE_SCALE_MAX - crate::canvas::SIZE_SCALE_MIN)
                * 100.0,
        );
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
                    .child(self.swatches(ink, bump))
            }))
            .maybe_child(self.specific(kind, config, bump))
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
        // The selection is in pane coordinates; the bar floats over it in
        // the window, kept inside the pane.
        let (x0, y0, x1, y1) = self.bounds;
        let mask = self.mask;
        let pane = try_consume_context::<notein_ui_kit::PaneArea>()
            .map(notein_ui_kit::PaneArea::get)
            .unwrap_or_default();
        let selection = Area::new(
            (pane.min_x() + x0, pane.min_y() + y0 - 8.0).into(),
            (x1 - x0, y1 - y0 + 16.0).into(),
        );
        let floating = notein_ui_kit::Floating::new(
            selection,
            rect()
                .on_sized(move |e: Event<SizedEventData>| mask.publish(e.area))
                .child(SelectionBar::new(items)),
        )
        .above()
        .centered();
        if pane.width() > 0.0 {
            floating.bounds(pane)
        } else {
            floating
        }
    }
}

/// Zoom percentage; tapping it fits the page again.
#[derive(Clone)]
struct Zoom {
    board: Arc<Mutex<Board>>,
    /// Current zoom factor, 1.0 = 100 %.
    level: State<f32>,
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
        let pct = (*self.level.read() * 100.0).round() as i32;
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
            .on_tap(move |_| crate::canvas::glide(&board, crate::canvas::Viewport::default()))
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

impl Layers {
    /// One layer: pick it, show or hide it, lock it.
    fn row(
        &self,
        layer: &crate::canvas::LayerSnapshot,
        active: bool,
        bump: impl Fn() + Copy + 'static,
    ) -> Element {
        use freya::icons::lucide;
        let id = layer.id;
        let (visible, locked) = (layer.visible, layer.locked);
        let (vis, lock_board, pick) = (
            Arc::clone(&self.board),
            Arc::clone(&self.board),
            Arc::clone(&self.board),
        );
        notein_ui_kit::ListRow::new(layer.name.clone())
            .subtitle(crate::format::count(layer.stroke_count, "trazo", "trazos"))
            .selected(active)
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
                            if visible {
                                lucide::eye()
                            } else {
                                lucide::eye_off()
                            },
                            "Mostrar u ocultar",
                        )
                        .size(tokens::size::BUTTON_SM)
                        .on_press(move |_| {
                            lock(&vis).set_layer_visible(id, !visible);
                            bump();
                        }),
                    )
                    .child(
                        IconButton::new(
                            if locked {
                                lucide::lock()
                            } else {
                                lucide::lock_open()
                            },
                            "Bloquear",
                        )
                        .size(tokens::size::BUTTON_SM)
                        .on_press(move |_| {
                            lock(&lock_board).set_layer_locked(id, !locked);
                            bump();
                        }),
                    )
                    .into_element(),
            ))
            .into_element()
    }

    /// Nueva capa and Borrar capa.
    fn actions(&self, bump: impl Fn() + Copy + 'static) -> Rect {
        let (add, remove) = (Arc::clone(&self.board), Arc::clone(&self.board));
        rect()
            .horizontal()
            .spacing(8.0)
            .child(
                notein_ui_kit::Button::new("Nueva capa")
                    .tonal()
                    .small()
                    .icon(freya::icons::lucide::plus())
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
            )
    }
}

impl Component for Layers {
    fn render(&self) -> impl IntoElement {
        let revision = use_state(|| 0u32);
        // Re-render after each change made here; the board is not reactive.
        let _ = revision.read();
        let bump = move || {
            let mut revision = revision;
            let next = revision.peek().wrapping_add(1);
            revision.set(next);
        };
        let guard = lock(&self.board);
        let layers = guard.layers_snapshot();
        let active = guard.active_layer_id();
        drop(guard);
        // Top layer first, as they stack.
        let rows: Vec<Element> = layers
            .iter()
            .rev()
            .map(|layer| self.row(layer, layer.id == active, bump))
            .collect();
        rect()
            .width(Size::fill())
            .padding(8.0)
            .spacing(8.0)
            .child(notein_ui_kit::Group::default().children(rows))
            .child(self.actions(bump))
            .child(
                Text::new("La capa elegida recibe lo que escribís.")
                    .role(TextRole::Hint)
                    .color(palette().t3),
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
        Arc::ptr_eq(&self.board, &other.board)
            && self.selected == other.selected
            && self.origin == other.origin
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
        spawn_in_window(async move {
            let outcome = rx
                .recv_async()
                .await
                .unwrap_or(AnnotationOutcome::Cancelled);
            {
                let mut guard = lock(&board);
                match outcome {
                    AnnotationOutcome::Saved { color, body } => {
                        guard.update_bookmark(
                            id,
                            body,
                            refs,
                            color.map(|c| [c[0], c[1], c[2], 0xFF]),
                            crate::format::now(),
                        );
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
                doc: self.item.map_or(
                    notein_plugin::doc::DocId([0; 16]),
                    crate::plugins::content::doc_id,
                ),
                kind: crate::plugins::builtin::canvas::KIND.to_owned(),
                title: "Marcador".to_owned(),
                quote,
                color: bookmark.color.map(|c| [c[0], c[1], c[2]]),
                body: bookmark.body,
                target: Some((
                    self.origin.0 + sx - 6.0,
                    self.origin.1 + sy - 6.0,
                    12.0,
                    12.0,
                )),
                existing: true,
            },
            None,
            std::sync::Arc::new(move |outcome| {
                let _ = tx.send(outcome);
            }),
        );
    }
}

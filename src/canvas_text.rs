//! Canvas UI for text boxes.
//!
//! [`CanvasLayers`] replaces the single drawing canvas with one canvas per
//! slice of layers and lays each layer's text boxes out right above its
//! strokes, so a box keeps its place in the layer stack. Boxes are note
//! editors in world space: the editor theme is zoomed with the viewport
//! (see [`EditorTheme::zoomed`]) so text wraps, measures and hit tests at
//! its real on-screen size.
//!
//! The box under edit gets handles to move, resize (wrap width) and rotate
//! it, and [`text_toolbar`] docks the formatting bar above the soft
//! keyboard. Session rules live on the [`Board`] (see `canvas::text`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use freya::icons::lucide::{check, grip, image_plus, rotate_cw, trash_2};
use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use istmo::plugins::EdgeInsets;
use notein_i18n::tr;
use notein_ui_kit::TapExt;

use crate::canvas::{
    Board, InkCache, PaintPass, SceneText, SurfaceBounds, TextLayerView, TextScene, lock,
};
use crate::editor::images::{AssetImage, ItemImages};
use crate::image_box::{IMAGE_MIN_SIDE, ImageBox};
use crate::library::model::ItemId;
use crate::text_box::{TEXT_BASE_SIZE, TEXT_MIN_WIDTH, TextBoxId, TextFrame};
use crate::ui_mask::{MaskSlot, use_mask_slot};

/// Family name of the default canvas text font.
pub const FONT_FAMILY: &str = "Excalifont";
/// Excalifont (by Excalidraw), registered at launch.
pub const FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/Excalifont.ttf");

/// Diameter of the move / rotate / resize handles, in screen pixels.
const HANDLE: f32 = 26.0;
/// Distance from the top edge to the rotate handle, in screen pixels.
const ROTATE_OFFSET: f32 = 34.0;
/// Rotations this close (degrees) to a multiple of 45° snap onto it.
const ROTATE_SNAP: f32 = 4.0;
/// Inner space between a box's bounds and its text, in screen pixels.
const BOX_PADDING: (f32, f32) = (4.0, 6.0);

/// Editor shared by every text box of a pane.
///
/// The controller belongs to the pane rather than to the box under edit:
/// when a session ends the box unmounts, but the toolbar and the editor's
/// own blur handling may still touch the controller for a moment, and
/// controller state owned by the unmounted box would already be gone.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TextEditorSlot {
    pub controller: EditorController,
    /// Box currently mounted as an editor; the toolbar shows while set.
    pub editing: State<Option<TextBoxId>>,
    /// Pen mask of the box under edit.
    edit_mask: MaskSlot,
    /// Pen mask of the formatting bar.
    toolbar_mask: MaskSlot,
    /// Pen mask of the image bar.
    image_mask: MaskSlot,
    /// Pen mask of the open editor or toolbar popup (menus, pickers).
    popup_mask: MaskSlot,
}

impl std::fmt::Debug for TextEditorSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEditorSlot")
            .field("editing", &*self.editing.peek())
            .finish_non_exhaustive()
    }
}

impl TextEditorSlot {
    /// Allocate the slot. Call once per pane, at the top of its render.
    #[must_use]
    pub fn use_slot() -> Self {
        Self {
            controller: use_editor_controller(),
            editing: use_state(|| None),
            edit_mask: use_mask_slot(),
            toolbar_mask: use_mask_slot(),
            image_mask: use_mask_slot(),
            popup_mask: use_mask_slot(),
        }
    }

    /// Reporter the editor and toolbar call with the area of their open
    /// popup, so a pen tap on a menu does not reach the canvas below.
    #[must_use]
    pub fn popup_reporter(self) -> PopupReporter {
        let mask = self.popup_mask;
        PopupReporter::new(move |area| match area {
            Some(area) => mask.publish(area),
            None => mask.clear(),
        })
    }
}

/// Theme for a box whose body colour is `color` (RGBA) at viewport `scale`.
#[sismo::function("canvas_text::theme")]
fn canvas_theme(color: [u8; 4], scale: f32) -> EditorTheme {
    let base = crate::editor::theme(&notein_ui_kit::palette());
    let mut theme = base.clone().zoomed(TEXT_BASE_SIZE / base.body_size * scale);
    let [r, g, b, a] = color;
    let text = Color::from_argb(a, r, g, b);
    theme.colors.text = text;
    theme.colors.caret = text;
    theme.colors.surface = Color::TRANSPARENT;
    theme.fonts.insert(0, FONT_FAMILY.into());
    theme
}

/// Canvas passes plus the text boxes between them. Fills the drawing
/// surface; pointer input falls through to it except on the box under edit
/// and its handles.
#[derive(Clone)]
pub struct CanvasLayers {
    pub board: Arc<Mutex<Board>>,
    /// Editor of the box under edit, shared with [`text_toolbar`].
    pub editor: TextEditorSlot,
    /// Library item whose `assets/` folder holds the canvas images;
    /// `None` on scratch surfaces, which show no images.
    pub item: Option<ItemId>,
}

impl PartialEq for CanvasLayers {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board)
            && self.editor == other.editor
            && self.item == other.item
    }
}

impl std::fmt::Debug for CanvasLayers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanvasLayers").finish_non_exhaustive()
    }
}

/// Geometry shared by every box of a render: viewport and the surface
/// origin in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    tx: f32,
    ty: f32,
    scale: f32,
    origin: (f32, f32),
}

impl View {
    fn to_screen(self, wx: f32, wy: f32) -> (f32, f32) {
        (
            wx.mul_add(self.scale, self.tx),
            wy.mul_add(self.scale, self.ty),
        )
    }
}

impl Component for CanvasLayers {
    #[sismo::function("canvas_text::layers_render")]
    fn render(&self) -> impl IntoElement {
        let board = Arc::clone(&self.board);
        let accent = notein_ui_kit::palette().accent;
        lock(&board).set_lasso_accent([accent.r(), accent.g(), accent.b()]);
        let scene = use_state({
            let board = Arc::clone(&board);
            move || lock(&board).text_scene()
        });
        let viewport = use_state({
            let board = Arc::clone(&board);
            move || lock(&board).viewport()
        });
        let mut origin = use_state(|| (0.0_f32, 0.0_f32));
        use_hook({
            let board = Arc::clone(&board);
            move || watch_board(&board, scene, viewport)
        });

        let scene = scene.read();
        let vp = *viewport.read();
        let view = View {
            tx: vp.tx,
            ty: vp.ty,
            scale: vp.scale,
            origin: *origin.read(),
        };
        let passes = PaintPass::split(scene.paint_passes());
        let children = passes.into_iter().enumerate().flat_map(|(index, pass)| {
            let canvas = pass_canvas(&board, pass, index);
            let texts = scene
                .layers
                .get(index)
                .map(|layer| text_layer(&board, layer, &scene, view, self.editor, self.item));
            std::iter::once(canvas).chain(texts)
        });

        rect()
            .width(Size::fill())
            .height(Size::fill())
            // Boxes past the pane edge must not draw over a split neighbour.
            .overflow(Overflow::Clip)
            .on_sized(move |event: Event<SizedEventData>| {
                let next = (event.area.min_x(), event.area.min_y());
                if *origin.peek() != next {
                    origin.set(next);
                }
            })
            .children(children.collect::<Vec<_>>())
    }
}

/// Follow board redraws: refresh the scene when its revision moves, and the
/// viewport while there is text to place.
#[sismo::function("canvas_text::watch_board")]
fn watch_board(
    board: &Arc<Mutex<Board>>,
    mut scene: State<TextScene>,
    mut viewport: State<crate::canvas::Viewport>,
) {
    let (rx, mut revision) = {
        let mut guard = lock(board);
        (guard.watch(), guard.text_revision())
    };
    let board = Arc::clone(board);
    spawn(async move {
        while rx.recv_async().await.is_ok() {
            let (next_revision, next_viewport) = {
                let guard = lock(&board);
                (guard.text_revision(), guard.viewport())
            };
            if next_revision != revision {
                revision = next_revision;
                let next = lock(&board).text_scene();
                if *scene.peek() != next {
                    scene.set(next);
                }
            }
            if !scene.peek().layers.is_empty() && *viewport.peek() != next_viewport {
                viewport.set(next_viewport);
            }
        }
    });
}

#[sismo::function("canvas_text::pass_canvas")]
fn pass_canvas(board: &Arc<Mutex<Board>>, pass: PaintPass, index: usize) -> Element {
    // Render callbacks never compare unequal, so a pass that changes shape
    // must remount: the key carries the whole pass.
    let key = format!(
        "pass-{index}-{}-{}-{}-{}",
        pass.layers.start, pass.layers.end, pass.base, pass.overlays
    );
    let board = Arc::clone(board);
    let mut cache = InkCache::default();
    rect()
        .key(key)
        .position(Position::new_absolute().top(0.).left(0.))
        .width(Size::fill())
        .height(Size::fill())
        .child(
            rect()
                .background(RenderCallback::new(move |ctx| {
                    sismo::scope!("canvas::pass_render");
                    let RenderTarget::Background { canvas, .. } = &ctx.target else {
                        return;
                    };
                    let bounds = SurfaceBounds {
                        min_x: 0.0,
                        min_y: 0.0,
                        max_x: ctx.size.width,
                        max_y: ctx.size.height,
                    };
                    lock(&board).paint(canvas, bounds, &pass, &mut cache);
                }))
                .width(Size::fill())
                .height(Size::fill()),
        )
        .into_element()
}

/// The images and text boxes of one layer, images below. The handles of
/// the box under edit or the selected image sit on top.
#[sismo::function("canvas_text::text_layer")]
fn text_layer(
    board: &Arc<Mutex<Board>>,
    layer: &TextLayerView,
    scene: &TextScene,
    view: View,
    editor: TextEditorSlot,
    item: Option<ItemId>,
) -> Element {
    let images = layer.images.iter().filter_map(|image| {
        Some(
            CanvasImageBox {
                item: item?,
                image: image.clone(),
                view,
                selected: scene.selected_image == Some(image.id),
                outlined: scene.image_mode,
            }
            .into_element(),
        )
    });
    let boxes = layer.texts.iter().map(|text| {
        let editing = scene.editing == Some(text.text.id);
        CanvasTextBox {
            board: Arc::clone(board),
            text: text.clone(),
            view,
            editing,
            outlined: scene.text_mode,
            editor,
        }
        .into_element()
    });
    let text_handles = layer
        .texts
        .iter()
        .find(|text| scene.editing == Some(text.text.id))
        .map(|text| BoxHandles {
            board: Arc::clone(board),
            target: HandleTarget::Text,
            frame: text.text.frame,
            height: text.height,
            view,
        });
    let image_handles = layer
        .images
        .iter()
        .find(|image| scene.selected_image == Some(image.id))
        .map(|image| BoxHandles {
            board: Arc::clone(board),
            target: HandleTarget::Image,
            frame: image.frame,
            height: image.height,
            view,
        });
    rect()
        .key(format!("texts-{}", layer.index))
        .position(Position::new_absolute().top(0.).left(0.))
        .width(Size::fill())
        .height(Size::fill())
        .maybe(layer.opacity < 0.999, |r| r.opacity(layer.opacity))
        .children(images)
        .children(boxes)
        .maybe_child(text_handles)
        .maybe_child(image_handles)
        .into_element()
}

/// One image on the canvas. Never takes pointer input: taps reach the
/// surface, which selects it through the board.
#[derive(Clone, PartialEq)]
struct CanvasImageBox {
    item: ItemId,
    image: ImageBox,
    view: View,
    selected: bool,
    /// Show the bounds even while idle, so images can be found in image
    /// mode.
    outlined: bool,
}

impl Component for CanvasImageBox {
    #[sismo::function("canvas_text::image_box_render")]
    fn render(&self) -> impl IntoElement {
        let view = self.view;
        let frame = self.image.frame;
        let (left, top) = view.to_screen(frame.x, frame.y);
        rect()
            .key(self.image.id.0)
            .position(Position::new_absolute().left(left).top(top))
            .width(Size::px(frame.width * view.scale))
            .height(Size::px(self.image.height * view.scale))
            .rotate(frame.rotation)
            .interactive(false)
            .overflow(Overflow::Clip)
            .map(
                outline(self.selected, self.outlined),
                |r, (width, color)| {
                    r.border(
                        Border::new()
                            .width(width)
                            .alignment(BorderAlignment::Inner)
                            .fill(color),
                    )
                },
            )
            .child(AssetImage {
                item: self.item,
                name: self.image.asset.clone(),
            })
    }

    fn render_key(&self) -> DiffKey {
        DiffKey::from(&self.image.id.0)
    }
}

/// One text box: read only while idle, a live editor while edited.
#[derive(Clone)]
struct CanvasTextBox {
    board: Arc<Mutex<Board>>,
    text: SceneText,
    view: View,
    editing: bool,
    /// Show the bounds even while idle, so boxes can be found in text mode.
    outlined: bool,
    editor: TextEditorSlot,
}

impl PartialEq for CanvasTextBox {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board)
            && self.text == other.text
            && self.view == other.view
            && self.editing == other.editing
            && self.outlined == other.outlined
            && self.editor == other.editor
    }
}

impl Component for CanvasTextBox {
    #[sismo::function("canvas_text::text_box_render")]
    fn render(&self) -> impl IntoElement {
        let text = &self.text.text;
        let view = self.view;
        let frame = text.frame;
        let (left, top) = view.to_screen(frame.x, frame.y);
        let theme = canvas_theme(text.color, view.scale);
        let id = text.id;
        let board = Arc::clone(&self.board);
        let body = if self.editing {
            TextBoxEditor {
                board: Arc::clone(&self.board),
                id,
                markdown: text.markdown.clone(),
                theme,
                editor: self.editor,
            }
            .into_element()
        } else {
            VisualText {
                markdown: text.markdown.clone(),
                theme,
            }
            .into_element()
        };
        rect()
            .key(id.0)
            .position(Position::new_absolute().left(left).top(top))
            .width(Size::px(frame.width * view.scale))
            .rotate(frame.rotation)
            // Idle boxes keep links and chips live, except in text mode,
            // where a tap on a box means "edit it".
            .interactive(self.editing || !self.outlined)
            .on_sized(move |event: Event<SizedEventData>| {
                lock(&board).set_text_height(id, event.area.height() / view.scale);
            })
            .padding(BOX_PADDING)
            .map(outline(self.editing, self.outlined), |r, (width, color)| {
                r.border(
                    Border::new()
                        .width(width)
                        .alignment(BorderAlignment::Inner)
                        .fill(color),
                )
            })
            .maybe(self.editing, |r| {
                r.on_touch_start(|event: Event<TouchEventData>| event.stop_propagation())
                    .on_touch_move(|event: Event<TouchEventData>| event.stop_propagation())
                    .on_touch_end(|event: Event<TouchEventData>| event.stop_propagation())
                    .on_mouse_down(|event: Event<MouseEventData>| event.stop_propagation())
                    // The canvas starts its bookmark long press on pointer down.
                    .on_pointer_down(|event: Event<PointerEventData>| event.stop_propagation())
            })
            .child(body)
    }

    fn render_key(&self) -> DiffKey {
        DiffKey::from(&self.text.text.id.0)
    }
}

/// Border `(width, colour)` around a box: strong while edited, faint while
/// text mode is on, none otherwise.
fn outline(editing: bool, outlined: bool) -> Option<(f32, Color)> {
    if editing {
        Some((2.0, notein_ui_kit::palette().accent))
    } else if outlined {
        Some((1.0, notein_ui_kit::palette().accent.with_a(90)))
    } else {
        None
    }
}

/// Read only rendering of a box body.
#[derive(Clone, PartialEq)]
struct VisualText {
    markdown: String,
    theme: EditorTheme,
}

impl Component for VisualText {
    #[sismo::function("canvas_text::visual_text_render")]
    fn render(&self) -> impl IntoElement {
        let controller = use_editor_controller();
        let mut value = use_state(|| self.markdown.clone());
        if *value.peek() != self.markdown {
            value.set(self.markdown.clone());
        }
        crate::editor::editor(value, controller)
            .read_only(true)
            .theme(self.theme.clone())
            .font_family(FONT_FAMILY)
            .width(Size::fill())
    }
}

/// Live editor of the box under edit. Mounted only while editing, so focus
/// and keyboard state go away with the session; the controller is the
/// pane's (see [`TextEditorSlot`]).
#[derive(Clone)]
struct TextBoxEditor {
    board: Arc<Mutex<Board>>,
    id: TextBoxId,
    markdown: String,
    theme: EditorTheme,
    editor: TextEditorSlot,
}

impl PartialEq for TextBoxEditor {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board)
            && self.id == other.id
            && self.theme == other.theme
            && self.editor == other.editor
    }
}

impl Component for TextBoxEditor {
    #[sismo::function("canvas_text::editor_render")]
    fn render(&self) -> impl IntoElement {
        let controller = self.editor.controller;
        let value = use_state(|| self.markdown.clone());
        use_side_effect({
            let board = Arc::clone(&self.board);
            move || {
                let markdown = value.read();
                lock(&board).set_session_markdown(&markdown);
            }
        });
        let mut editing = self.editor.editing;
        let mask = self.editor.edit_mask;
        let id = self.id;
        use_hook(move || {
            editing.set(Some(id));
            controller.focus_end();
            spawn(async move {
                // Let the caret land before asking for the keyboard.
                async_io::Timer::after(Duration::from_millis(60)).await;
                crate::soft_keyboard::show();
            });
        });
        use_drop(move || {
            sismo::scope!("canvas_text::editor_drop");
            // Switching boxes mounts the next editor before this one drops.
            if *editing.peek() == Some(id) {
                editing.set(None);
                mask.clear();
            }
        });

        rect()
            .width(Size::fill())
            .on_sized(move |event: Event<SizedEventData>| mask.publish(event.area))
            .child(
                crate::editor::editor(value, controller)
                    .theme(self.theme.clone())
                    .font_family(FONT_FAMILY)
                    .placeholder(tr!("canvas-text-placeholder"))
                    .width(Size::fill()),
            )
    }
}

/// A corner of a box, named by its unrotated position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    const ALL: [Self; 4] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    /// Unrotated local position of the corner of a `width` × `height`
    /// box.
    const fn local(self, width: f32, height: f32) -> (f32, f32) {
        match self {
            Self::TopLeft => (0.0, 0.0),
            Self::TopRight => (width, 0.0),
            Self::BottomLeft => (0.0, height),
            Self::BottomRight => (width, height),
        }
    }

    const fn opposite(self) -> Self {
        match self {
            Self::TopLeft => Self::BottomRight,
            Self::TopRight => Self::BottomLeft,
            Self::BottomLeft => Self::TopRight,
            Self::BottomRight => Self::TopLeft,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    Move,
    Rotate,
    /// Text boxes: drag the wrap width from the left edge.
    Left,
    /// Text boxes: drag the wrap width from the right edge.
    Right,
    /// Images: scale around the opposite corner, keeping the aspect.
    Corner(Corner),
}

/// Drag in progress on one of the handles.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HandleDrag {
    handle: Handle,
    /// Pointer position at press, window coordinates.
    start: (f32, f32),
    frame: TextFrame,
    height: f32,
}

impl HandleDrag {
    /// Frame and height after dragging the pointer to `(px, py)` (window
    /// coordinates).
    #[sismo::function("canvas_text::handle_frame_at")]
    fn frame_at(&self, view: View, px: f32, py: f32) -> (TextFrame, f32) {
        let start = self.frame;
        let (dx, dy) = (
            (px - self.start.0) / view.scale,
            (py - self.start.1) / view.scale,
        );
        match self.handle {
            Handle::Move => (
                TextFrame {
                    x: start.x + dx,
                    y: start.y + dy,
                    ..start
                },
                self.height,
            ),
            Handle::Corner(corner) => {
                // Pointer in the box's own unrotated space; the opposite
                // corner stays put and the box scales towards the pointer.
                let wx = (px - view.origin.0 - view.tx) / view.scale;
                let wy = (py - view.origin.1 - view.ty) / view.scale;
                let (lx, ly) = start.to_local(self.height, wx, wy);
                let anchor = corner.opposite();
                let (ax, ay) = anchor.local(start.width, self.height);
                let aspect = start.width / self.height.max(f32::EPSILON);
                let min_width = IMAGE_MIN_SIDE.max(IMAGE_MIN_SIDE * aspect);
                let width = (lx - ax).abs().max((ly - ay).abs() * aspect).max(min_width);
                let height = width / aspect;
                let anchor_world = start.to_world(self.height, ax, ay);
                let (nax, nay) = anchor.local(width, height);
                let (sin, cos) = start.rotation.to_radians().sin_cos();
                let (ox, oy) = (width.mul_add(0.5, -nax), height.mul_add(0.5, -nay));
                let center = (
                    ox.mul_add(cos, -oy * sin) + anchor_world.0,
                    ox.mul_add(sin, oy * cos) + anchor_world.1,
                );
                (
                    TextFrame {
                        x: width.mul_add(-0.5, center.0),
                        y: height.mul_add(-0.5, center.1),
                        width,
                        ..start
                    },
                    height,
                )
            }
            Handle::Left | Handle::Right => {
                let (sin, cos) = start.rotation.to_radians().sin_cos();
                let along = dx.mul_add(cos, dy * sin);
                let (width, anchor_side) = if self.handle == Handle::Right {
                    ((start.width + along).max(TEXT_MIN_WIDTH), -1.0)
                } else {
                    ((start.width - along).max(TEXT_MIN_WIDTH), 1.0)
                };
                // Keep the opposite edge where it is: rotation pivots on
                // the centre, so the centre slides along the box axis.
                let (cx, cy) = start.center(self.height);
                let anchor = (
                    (anchor_side * start.width * 0.5).mul_add(cos, cx),
                    (anchor_side * start.width * 0.5).mul_add(sin, cy),
                );
                let center = (
                    (-anchor_side * width * 0.5).mul_add(cos, anchor.0),
                    (-anchor_side * width * 0.5).mul_add(sin, anchor.1),
                );
                (
                    TextFrame {
                        x: width.mul_add(-0.5, center.0),
                        y: self.height.mul_add(-0.5, center.1),
                        width,
                        ..start
                    },
                    self.height,
                )
            }
            Handle::Rotate => {
                let (cx, cy) = start.center(self.height);
                let (cx, cy) = view.to_screen(cx, cy);
                let (cx, cy) = (cx + view.origin.0, cy + view.origin.1);
                let from = (self.start.1 - cy).atan2(self.start.0 - cx);
                let to = (py - cy).atan2(px - cx);
                let raw = start.rotation + (to - from).to_degrees();
                (
                    TextFrame {
                        rotation: snap_rotation(raw),
                        ..start
                    },
                    self.height,
                )
            }
        }
    }
}

/// Normalise to `(-180, 180]` and snap near multiples of 45°.
fn snap_rotation(degrees: f32) -> f32 {
    let mut angle = degrees.rem_euclid(360.0);
    if angle > 180.0 {
        angle -= 360.0;
    }
    let nearest = (angle / 45.0).round() * 45.0;
    if (angle - nearest).abs() <= ROTATE_SNAP {
        nearest
    } else {
        angle
    }
}

/// What a set of handles edits: the box under edit (wrap width from the
/// sides) or the selected image (scaled from the corners).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandleTarget {
    Text,
    Image,
}

/// Handles of the box under edit or the selected image. Drawn unrotated
/// at the rotated positions, so hit testing matches what the user sees.
#[derive(Clone)]
struct BoxHandles {
    board: Arc<Mutex<Board>>,
    target: HandleTarget,
    frame: TextFrame,
    height: f32,
    view: View,
}

impl PartialEq for BoxHandles {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board)
            && self.target == other.target
            && self.frame == other.frame
            && self.height == other.height
            && self.view == other.view
    }
}

impl Component for BoxHandles {
    #[sismo::function("canvas_text::handles_render")]
    fn render(&self) -> impl IntoElement {
        let mut drag = use_state(|| Option::<HandleDrag>::None);
        let view = self.view;
        let frame = self.frame;
        let height = self.height;
        let target = self.target;
        let scale = view.scale;
        let place = |lx: f32, ly: f32| {
            let (wx, wy) = frame.to_world(height, lx, ly);
            view.to_screen(wx, wy)
        };
        let mut handles = vec![
            (Handle::Move, place(-HANDLE / scale, -HANDLE / scale)),
            (
                Handle::Rotate,
                place(frame.width * 0.5, -ROTATE_OFFSET / scale),
            ),
        ];
        match target {
            HandleTarget::Text => handles.extend([
                (Handle::Left, place(0.0, height * 0.5)),
                (Handle::Right, place(frame.width, height * 0.5)),
            ]),
            HandleTarget::Image => handles.extend(Corner::ALL.into_iter().map(|corner| {
                let (lx, ly) = corner.local(frame.width, height);
                (Handle::Corner(corner), place(lx, ly))
            })),
        }
        let move_board = Arc::clone(&self.board);
        rect()
            .position(Position::new_absolute().top(0.).left(0.))
            .width(Size::fill())
            .height(Size::fill())
            .on_global_pointer_move(move |event: Event<PointerEventData>| {
                let Some(active) = *drag.peek() else {
                    return;
                };
                let at = event.global_location();
                #[allow(clippy::cast_possible_truncation, reason = "screen coordinates")]
                let (next, next_height) = active.frame_at(view, at.x as f32, at.y as f32);
                let mut board = lock(&move_board);
                match target {
                    HandleTarget::Text => board.set_session_frame(next),
                    HandleTarget::Image => board.set_session_image_frame(next, next_height),
                }
            })
            .on_global_pointer_up(move |_| {
                if drag.peek().is_some() {
                    drag.set(None);
                }
            })
            .children(handles.into_iter().map(|(handle, (x, y))| {
                handle_dot(handle, x, y, frame.rotation, move |start| {
                    drag.set(Some(HandleDrag {
                        handle,
                        start,
                        frame,
                        height,
                    }));
                })
            }))
    }
}

/// One handle centred on `(x, y)`. Resize pills turn with the box
/// (`rotation`, degrees) so they stay parallel to its edges; round handles
/// look the same at any angle.
#[sismo::function("canvas_text::handle_dot")]
fn handle_dot(
    handle: Handle,
    x: f32,
    y: f32,
    rotation: f32,
    mut on_start: impl FnMut((f32, f32)) + 'static,
) -> Element {
    let icon = match handle {
        Handle::Move => Some(grip()),
        Handle::Rotate => Some(rotate_cw()),
        Handle::Left | Handle::Right | Handle::Corner(_) => None,
    };
    let (width, height) = match handle {
        Handle::Left | Handle::Right => (HANDLE * 0.5, HANDLE),
        Handle::Corner(_) => (HANDLE * 0.7, HANDLE * 0.7),
        Handle::Move | Handle::Rotate => (HANDLE, HANDLE),
    };
    rect()
        .key(format!("{handle:?}"))
        .position(
            Position::new_absolute()
                .left(width.mul_add(-0.5, x))
                .top(height.mul_add(-0.5, y)),
        )
        .width(Size::px(width))
        .height(Size::px(height))
        .maybe(matches!(handle, Handle::Left | Handle::Right), |pill| {
            pill.rotate(rotation)
        })
        .center()
        .corner_radius(HANDLE * 0.5)
        .background(notein_ui_kit::palette().s2)
        .border(
            Border::new()
                .width(1.5)
                .alignment(BorderAlignment::Inner)
                .fill(notein_ui_kit::palette().accent),
        )
        .cursor(match handle {
            Handle::Move => CursorIcon::Move,
            Handle::Rotate => CursorIcon::Grab,
            Handle::Left | Handle::Right => CursorIcon::EwResize,
            Handle::Corner(Corner::TopLeft | Corner::BottomRight) => CursorIcon::NwseResize,
            Handle::Corner(Corner::TopRight | Corner::BottomLeft) => CursorIcon::NeswResize,
        })
        .on_touch_start(|event: Event<TouchEventData>| event.stop_propagation())
        .on_touch_move(|event: Event<TouchEventData>| event.stop_propagation())
        .on_touch_end(|event: Event<TouchEventData>| event.stop_propagation())
        .on_mouse_down(|event: Event<MouseEventData>| event.stop_propagation())
        .on_pointer_down(move |event: Event<PointerEventData>| {
            event.stop_propagation();
            let at = event.global_location();
            #[allow(clippy::cast_possible_truncation, reason = "screen coordinates")]
            on_start((at.x as f32, at.y as f32));
        })
        .maybe_child(icon.map(|icon| {
            SvgViewer::new(icon)
                .color(notein_ui_kit::palette().accent)
                .width(Size::px(HANDLE * 0.55))
                .height(Size::px(HANDLE * 0.55))
        }))
        .into_element()
}

/// Formatting bar of the box under edit, docked at the bottom of the pane.
///
/// Sits above the soft keyboard on mobile. The editor's toolbar wraps by
/// default; here it stays one row inside a [`SideScroll`]. Publishes its
/// window area in `area` (stale while hidden, so read it only while a box
/// is edited) for other bottom overlays to sit above it. Renders nothing
/// while no box is edited.
#[sismo::function("canvas_text::toolbar")]
pub fn text_toolbar(
    board: &Arc<Mutex<Board>>,
    editor: TextEditorSlot,
    mut area: State<Area>,
    pad: EdgeInsets,
) -> Element {
    let controller = editor.controller;
    if editor.editing.read().is_none() {
        editor.toolbar_mask.clear();
        return rect().into_element();
    }
    let mut theme = crate::editor::theme(&notein_ui_kit::palette());
    theme.fonts.insert(0, FONT_FAMILY.into());
    // Menus must stay above the bar: below it there is only the soft
    // keyboard. Around it, what the system bars leave of the window.
    let bar = *area.read();
    let frame = notein_ui_kit::safe_frame();
    let popup_area = Area::new(
        frame.origin,
        (frame.width(), (bar.max_y() - frame.min_y()).max(0.)).into(),
    );
    let delete_board = Arc::clone(board);
    let done_board = Arc::clone(board);
    let bar_content = rect()
        .horizontal()
        .cross_align(Alignment::center())
        .spacing(6.)
        .child(toolbar_action(check(), move || {
            lock(&done_board).end_text_session();
        }))
        .child(toolbar_action(trash_2(), move || {
            lock(&delete_board).delete_session_text();
        }))
        .child(
            rect()
                .width(Size::px(1.))
                .height(Size::px(18.))
                .margin((0., 4.))
                .background(notein_ui_kit::palette().line),
        )
        .child(
            MarkdownToolbar::new(controller)
                .theme(theme)
                .mode_toggle(false)
                .wrap(false)
                .maybe(bar.height() > 0., |toolbar| toolbar.popup_area(popup_area)),
        )
        .into_element();
    rect()
        .position(
            Position::new_absolute()
                .left(pad.left)
                .right(pad.right)
                .bottom(pad.bottom),
        )
        .width(Size::fill())
        .layer(Layer::Overlay)
        .padding((6., 8.))
        .background(notein_ui_kit::palette().s2)
        .border(
            Border::new()
                .width(BorderWidth {
                    top: 1.,
                    right: 0.,
                    bottom: 0.,
                    left: 0.,
                })
                .alignment(BorderAlignment::Inner)
                .fill(notein_ui_kit::palette().line),
        )
        .on_sized(move |event: Event<SizedEventData>| {
            editor.toolbar_mask.publish(event.area);
            area.set_if_modified(event.area);
        })
        .child(SideScroll {
            content: bar_content,
        })
        .into_element()
}

/// Bar of image mode, docked at the bottom of the pane.
///
/// Like [`text_toolbar`]: add an image, drop the selection, delete the
/// selected image. Publishes its window area in `area` and renders nothing
/// outside image mode.
#[derive(Clone)]
pub struct ImageBar {
    pub board: Arc<Mutex<Board>>,
    /// Item the images are stored in; scratch surfaces (`None`) cannot
    /// add any.
    pub item: Option<ItemId>,
    pub tool: State<crate::ink::Tool>,
    pub editor: TextEditorSlot,
    pub area: State<Area>,
    pub pad: EdgeInsets,
}

impl PartialEq for ImageBar {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.board, &other.board)
            && self.item == other.item
            && self.tool == other.tool
            && self.editor == other.editor
            && self.area == other.area
            && self.pad == other.pad
    }
}

impl std::fmt::Debug for ImageBar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageBar").finish_non_exhaustive()
    }
}

impl Component for ImageBar {
    #[sismo::function("canvas_text::image_bar_render")]
    fn render(&self) -> impl IntoElement {
        let board = Arc::clone(&self.board);
        let selected = use_state({
            let board = Arc::clone(&board);
            move || lock(&board).image_session().is_some()
        });
        use_hook({
            let board = Arc::clone(&board);
            move || watch_selection(&board, selected)
        });
        let editor = self.editor;
        let mut area = self.area;
        if *self.tool.read() != crate::ink::Tool::Image {
            editor.image_mask.clear();
            return rect().into_element();
        }
        let has_selection = *selected.read();
        let p = notein_ui_kit::palette();
        let add_board = Arc::clone(&board);
        let done_board = Arc::clone(&board);
        let delete_board = Arc::clone(&board);
        let item = self.item;
        let pad = self.pad;
        rect()
            .position(
                Position::new_absolute()
                    .left(pad.left)
                    .right(pad.right)
                    .bottom(pad.bottom),
            )
            .width(Size::fill())
            .layer(Layer::Overlay)
            .padding((6., 8.))
            .background(p.s2)
            .border(
                Border::new()
                    .width(BorderWidth {
                        top: 1.,
                        right: 0.,
                        bottom: 0.,
                        left: 0.,
                    })
                    .alignment(BorderAlignment::Inner)
                    .fill(p.line),
            )
            .on_sized(move |event: Event<SizedEventData>| {
                editor.image_mask.publish(event.area);
                area.set_if_modified(event.area);
            })
            .horizontal()
            .cross_align(Alignment::center())
            .spacing(6.)
            .maybe_child(item.map(|item| {
                notein_ui_kit::Button::new(tr!("canvas-image-add"))
                    .icon(image_plus())
                    .on_press(move |_| ItemImages::new(item).add_to_canvas(Arc::clone(&add_board)))
            }))
            .maybe(has_selection, |bar| {
                bar.child(toolbar_action(check(), move || {
                    lock(&done_board).end_image_session();
                }))
                .child(toolbar_action(trash_2(), move || {
                    lock(&delete_board).delete_session_image();
                }))
            })
            .into_element()
    }
}

/// Keep `selected` in step with whether the board has an image selected.
#[sismo::function("canvas_text::watch_selection")]
fn watch_selection(board: &Arc<Mutex<Board>>, mut selected: State<bool>) {
    let rx = lock(board).watch();
    let board = Arc::clone(board);
    spawn(async move {
        while rx.recv_async().await.is_ok() {
            let next = lock(&board).image_session().is_some();
            selected.set_if_modified(next);
        }
    });
}

/// One row that scrolls sideways: wheel on desktop, drag on touch screens,
/// with a thin bar showing there is more.
///
/// Freya's `ScrollView` takes the focus when dragged, which would close the
/// soft keyboard of the box under edit; this one never touches the focus.
#[derive(Clone, PartialEq)]
struct SideScroll {
    content: Element,
}

impl Component for SideScroll {
    #[sismo::function("canvas_text::side_scroll_render")]
    fn render(&self) -> impl IntoElement {
        let mut offset = use_state(|| 0.0_f32);
        let mut viewport = use_state(|| 0.0_f32);
        let mut content = use_state(|| 0.0_f32);
        let mut drag = use_state(|| Option::<(f32, f32)>::None);
        let max = (*content.read() - *viewport.read()).max(0.0);
        let current = offset.read().clamp(0.0, max);
        let mut scroll_to = move |to: f32| {
            let next = to.clamp(0.0, max);
            if (*offset.peek() - next).abs() > f32::EPSILON {
                offset.set(next);
            }
        };
        let thumb = (max > 0.0).then(|| {
            let track = *viewport.read();
            let width = (track * track / *content.read()).max(24.0);
            rect()
                .width(Size::px(width))
                .height(Size::px(3.))
                .margin((0., 0., 0., current / max * (track - width)))
                .corner_radius(2.)
                .background(notein_ui_kit::palette().t2)
        });
        rect()
            .width(Size::fill())
            .overflow(Overflow::Clip)
            .on_sized(move |event: Event<SizedEventData>| {
                viewport.set_if_modified(event.area.width());
            })
            .on_wheel(move |event: Event<WheelEventData>| {
                #[allow(clippy::cast_possible_truncation, reason = "pixel deltas")]
                let (dx, dy) = (event.delta_x as f32, event.delta_y as f32);
                let delta = if dx.abs() > dy.abs() { dx } else { dy };
                scroll_to(current - delta);
            })
            .on_pointer_down(move |event: Event<PointerEventData>| {
                if matches!(event.data(), PointerEventData::Touch(_)) {
                    #[allow(clippy::cast_possible_truncation, reason = "screen coordinates")]
                    let x = event.global_location().x as f32;
                    drag.set(Some((x, current)));
                }
            })
            .on_global_pointer_move(move |event: Event<PointerEventData>| {
                let Some((start_x, start_offset)) = *drag.peek() else {
                    return;
                };
                #[allow(clippy::cast_possible_truncation, reason = "screen coordinates")]
                let x = event.global_location().x as f32;
                scroll_to(start_offset - (x - start_x));
            })
            .on_global_pointer_up(move |_| {
                if drag.peek().is_some() {
                    drag.set(None);
                }
            })
            .child(
                rect()
                    .offset_x(-current)
                    .on_sized(move |event: Event<SizedEventData>| {
                        content.set_if_modified(event.area.width());
                    })
                    .child(self.content.clone()),
            )
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(3.))
                    .margin((6., 0., 0., 0.))
                    .maybe_child(thumb),
            )
    }
}

fn toolbar_action(icon: Bytes, on_press: impl Fn() + 'static) -> Element {
    rect()
        .width(Size::px(32.))
        .height(Size::px(32.))
        .center()
        .corner_radius(8.)
        .cursor(CursorIcon::Pointer)
        .on_tap(move |_| on_press())
        .child(
            SvgViewer::new(icon)
                .color(notein_ui_kit::palette().t1)
                .width(Size::px(16.))
                .height(Size::px(16.)),
        )
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_snaps_and_wraps() {
        assert!((snap_rotation(43.0) - 45.0).abs() < f32::EPSILON);
        assert!((snap_rotation(30.0) - 30.0).abs() < f32::EPSILON);
        assert!((snap_rotation(350.0) - -10.0).abs() < 1e-4);
    }

    #[test]
    fn right_handle_keeps_left_edge() {
        let view = View {
            tx: 0.0,
            ty: 0.0,
            scale: 1.0,
            origin: (0.0, 0.0),
        };
        let drag = HandleDrag {
            handle: Handle::Right,
            start: (0.0, 0.0),
            frame: TextFrame {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                rotation: 30.0,
            },
            height: 40.0,
        };
        let before = drag.frame.to_world(40.0, 0.0, 20.0);
        let (next, height) = drag.frame_at(view, 50.0, 0.0);
        let after = next.to_world(40.0, 0.0, 20.0);
        assert!(next.width > 100.0);
        assert!((height - 40.0).abs() < f32::EPSILON);
        assert!((before.0 - after.0).abs() < 1e-3 && (before.1 - after.1).abs() < 1e-3);
    }

    #[test]
    fn corner_keeps_the_aspect_and_the_opposite_corner() {
        let view = View {
            tx: 0.0,
            ty: 0.0,
            scale: 1.0,
            origin: (0.0, 0.0),
        };
        let frame = TextFrame {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            rotation: 20.0,
        };
        let drag = HandleDrag {
            handle: Handle::Corner(Corner::BottomRight),
            start: (0.0, 0.0),
            frame,
            height: 100.0,
        };
        let anchor_before = frame.to_world(100.0, 0.0, 0.0);
        // The pointer lands where the bottom-right corner would be at
        // double size.
        let (px, py) = frame.to_world(100.0, 400.0, 200.0);
        let (next, height) = drag.frame_at(view, px, py);
        assert!((next.width - 400.0).abs() < 1e-2);
        assert!((height - 200.0).abs() < 1e-2);
        let anchor_after = next.to_world(height, 0.0, 0.0);
        assert!((anchor_before.0 - anchor_after.0).abs() < 1e-2);
        assert!((anchor_before.1 - anchor_after.1).abs() < 1e-2);
    }
}

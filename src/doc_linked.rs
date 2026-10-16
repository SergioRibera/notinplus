//! Shell for a [`crate::library::ItemKind::DocLinked`] bundle.
//!
//! A DocLinked item owns two independent docs: a "source" face
//! rendered by an arbitrary plugin (named by the item's
//! `source_plugin` field) and a free-form "infinite canvas" face
//! whose strokes land at
//! [`crate::library::bodies::canvas_doc_path`]. [`DocLinkedView`]
//! exposes both: overlay mode renders the source face full-screen,
//! split mode lays the source face next to the canvas face with a
//! draggable divider.
//!
//! Source rendering is deferred to the plugin via the
//! [`crate::linked_source`] registry. The shell knows nothing about
//! how PDF (or any future source like Bible, EPUB, …) composes its
//! face — it just passes the item id and renders whatever element
//! [`crate::linked_source::LinkedDocSource::render_face`] returns.
//!
//! Split layout is served by the shared
//! [`crate::components::pane_tree_view::PaneTreeView`] with
//! [`crate::components::pane_tree_view::PaneTreeView::show_tabs`] off
//! — DocLinked has no tabs, just two fixed faces. The divider commits
//! its final ratio to the per-item sidecar (see
//! [`crate::doc_linked_prefs`]) on release.
//!
//! Each face owns its own backing resources: the source plugin
//! manages its own storage; the canvas face autosaves to
//! `canvas_doc_path(id)` through
//! [`crate::app::CanvasMount::DocLinkedCanvas`].
//!
//! [`SourcePdfView`] stays available for standalone
//! [`crate::library::ItemKind::SourcePdf`] items (future flows).

use freya::prelude::*;
use istmo::plugins::EdgeInsets;

use crate::app::{CanvasMount, CanvasPane};
use crate::components::pane_tree_view::{PaneTreeCallbacks, PaneTreeView, SplitEdge};
use crate::doc_linked_prefs::{self, DocLinkedSplit};
use crate::hooks::provide_safe_area_override;
use crate::library::{
    ItemId, ItemKind, Pane, PaneId, PaneLayout, PaneTab, SplitOrientation, SplitSide,
};
use crate::linked_source;

/// Deterministic pane ids for the DocLinked split faces. They never
/// hit disk — DocLinked's split layout is not a persisted workspace,
/// so the `PaneTreeView`'s pane-id-keyed hashing only needs them to be
/// distinct within a single render tree.
const PANE_ID_SOURCE: PaneId = PaneId::from_bytes([1; 16]);
const PANE_ID_CANVAS: PaneId = PaneId::from_bytes([2; 16]);

/// `PaneTab::plugin_id` sentinel assigned to the canvas face so the
/// body dispatch can tell the two leaves apart without peeking at
/// pane ids.
const CANVAS_FACE_PLUGIN: &str = "io.notein.doc-linked.canvas";

/// Context value a `DocLinked` shell installs so the chrome's
/// top-right stack can render the split-mode buttons without the chrome
/// code having to know about linked-doc specifics.
///
/// Shells that do not install this value get the default chrome layout
/// (no split buttons) — [`crate::app::top_right_toolbar`] falls back to
/// its baseline button row via `try_consume_context`.
#[derive(Clone)]
pub struct DocLinkedChromeExt {
    /// Whether the shell is currently in split mode. Flips the top-row
    /// visibility: split buttons when `false`, the close `×` when
    /// `true`.
    pub is_split: bool,
    /// Trigger a horizontal split — source left, canvas right.
    pub on_split_horizontal: NoArgCallback<()>,
    /// Trigger a vertical split — source top, canvas bottom.
    pub on_split_vertical: NoArgCallback<()>,
    /// Collapse the split back to overlay mode. Rendered on every
    /// visible chrome instance in split mode so either face can
    /// dismiss the split.
    pub on_close: NoArgCallback<()>,
}

impl std::fmt::Debug for DocLinkedChromeExt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocLinkedChromeExt")
            .field("is_split", &self.is_split)
            .finish_non_exhaustive()
    }
}

/// Linked-doc bundle route. The source face plugin is resolved from
/// the item's [`ItemKind::DocLinked`] payload at mount time.
#[derive(Debug, PartialEq)]
pub struct DocLinkedView {
    pub id: ItemId,
}

impl Component for DocLinkedView {
    fn render(&self) -> impl IntoElement {
        let bundle = self.id;
        let insets = crate::hooks::use_safe_area_insets();
        let pad = *insets.read();
        // Carve the safe area at this shell; nested chrome (overlays,
        // modals) sees zero so it does not re-apply the same inset.
        provide_safe_area_override(EdgeInsets::default());

        // Async-resolve the item's source plugin on first mount.
        // Until it lands we render a blank slate (background color
        // only) so the shell never flashes a wrong plugin's face.
        let source_plugin = use_state(|| Option::<String>::None);
        use_hook({
            let mut setter = source_plugin;
            move || {
                spawn(async move {
                    match crate::home::open_library().await {
                        Ok(handle) => {
                            let lib = handle.lock().await;
                            if let Some(item) = lib.item(bundle) {
                                if let ItemKind::DocLinked { source_plugin } = &item.kind {
                                    setter.set(Some(source_plugin.clone()));
                                } else {
                                    log::error!(
                                        "DocLinkedView mounted on non-DocLinked item {bundle:?}",
                                    );
                                }
                            } else {
                                log::error!("DocLinkedView item {bundle:?} not found");
                            }
                        }
                        Err(err) => log::error!("open library on DocLinkedView mount: {err}"),
                    }
                });
            }
        });

        let split_state = use_state(move || doc_linked_prefs::load(bundle));
        let split = *split_state.read();

        // Persist the new split preference in the background. Save
        // failures log and are ignored — the next successful save
        // supersedes the on-disk state, and the UI already reflects
        // the intent.
        let apply = move |mut setter: State<DocLinkedSplit>, next: DocLinkedSplit| {
            setter.set(next);
            spawn(async move {
                if let Err(err) = doc_linked_prefs::save(bundle, &next) {
                    log::warn!("doc_linked_prefs::save: {err}");
                }
            });
        };

        let on_split_horizontal: NoArgCallback<()> = NoArgCallback::new(move || {
            apply(
                split_state,
                DocLinkedSplit {
                    orientation: Some(SplitOrientation::Horizontal),
                    ratio: 0.5,
                },
            );
        });
        let on_split_vertical: NoArgCallback<()> = NoArgCallback::new(move || {
            apply(
                split_state,
                DocLinkedSplit {
                    orientation: Some(SplitOrientation::Vertical),
                    ratio: 0.5,
                },
            );
        });
        let on_close: NoArgCallback<()> = NoArgCallback::new(move || {
            apply(
                split_state,
                DocLinkedSplit {
                    orientation: None,
                    ratio: split.ratio,
                },
            );
        });

        // Divider commit must land in the parent-owned `split_state`
        // alongside the sidecar save. `SplitNode::PartialEq` returns
        // false, so the drag-release render reads `self.ratio` back
        // from the parent prop — persisting without updating state
        // snaps the divider back and only the next remount shows the
        // new position.
        let on_resize: Callback<f32, ()> = Callback::new(move |new_ratio: f32| {
            apply(
                split_state,
                DocLinkedSplit {
                    orientation: split.orientation,
                    ratio: new_ratio,
                },
            );
        });

        let body = source_plugin.read().clone().map_or_else(
            || loading_placeholder().into_element(),
            |plugin_id| {
                doc_linked_body(
                    bundle,
                    plugin_id,
                    split,
                    on_split_horizontal.clone(),
                    on_split_vertical.clone(),
                    on_close.clone(),
                    on_resize,
                )
            },
        );

        safe_area_shell(pad)
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .content(Content::Flex)
            .child(body)
    }
}

/// Body renderer. Overlay mode mounts the source face full-screen;
/// split mode delegates to the shared pane tree with tabs disabled.
fn doc_linked_body(
    id: ItemId,
    source_plugin: String,
    split: DocLinkedSplit,
    on_split_horizontal: NoArgCallback<()>,
    on_split_vertical: NoArgCallback<()>,
    on_close: NoArgCallback<()>,
    on_resize: Callback<f32, ()>,
) -> Element {
    match split.orientation {
        None => DocLinkedOverlay {
            id,
            source_plugin,
            on_split_horizontal,
            on_split_vertical,
        }
        .into_element(),
        Some(orientation) => DocLinkedSplitBody {
            id,
            source_plugin,
            orientation,
            ratio: split.ratio,
            on_close,
            on_resize,
        }
        .into_element(),
    }
}

/// Default DocLinked body: the source face full-screen. The split-mode
/// buttons live in the chrome's top-right stack via the
/// [`DocLinkedChromeExt`] context installed here (`is_split = false`).
#[derive(Clone, PartialEq)]
struct DocLinkedOverlay {
    id: ItemId,
    source_plugin: String,
    on_split_horizontal: NoArgCallback<()>,
    on_split_vertical: NoArgCallback<()>,
}

impl std::fmt::Debug for DocLinkedOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocLinkedOverlay")
            .field("id", &self.id)
            .field("source_plugin", &self.source_plugin)
            .finish_non_exhaustive()
    }
}

impl Component for DocLinkedOverlay {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        use_provide_context({
            let on_split_horizontal = self.on_split_horizontal.clone();
            let on_split_vertical = self.on_split_vertical.clone();
            move || DocLinkedChromeExt {
                is_split: false,
                on_split_horizontal: on_split_horizontal.clone(),
                on_split_vertical: on_split_vertical.clone(),
                on_close: NoArgCallback::new(|| {}),
            }
        });
        rect()
            .width(Size::fill())
            .height(Size::flex(1.0))
            .content(Content::Flex)
            .child(source_face_element(id, &self.source_plugin))
    }
}

/// Split DocLinked body: source face next to the infinite canvas
/// face, served by the shared pane tree with tabs off so the divider
/// stays resizable without exposing tab-strip chrome. The
/// `DocLinkedChromeExt` (`is_split = true`) is installed once at this
/// level and read by every descendant [`CanvasPane`]'s chrome.
#[derive(Clone, PartialEq)]
struct DocLinkedSplitBody {
    id: ItemId,
    source_plugin: String,
    orientation: SplitOrientation,
    ratio: f32,
    on_close: NoArgCallback<()>,
    on_resize: Callback<f32, ()>,
}

impl std::fmt::Debug for DocLinkedSplitBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocLinkedSplitBody")
            .field("id", &self.id)
            .field("source_plugin", &self.source_plugin)
            .field("orientation", &self.orientation)
            .field("ratio", &self.ratio)
            .finish_non_exhaustive()
    }
}

impl Component for DocLinkedSplitBody {
    fn render(&self) -> impl IntoElement {
        let bundle = self.id;
        let orientation = self.orientation;
        let ratio = self.ratio;
        let source_plugin = self.source_plugin.clone();
        let on_resize = self.on_resize.clone();

        use_provide_context({
            let on_close = self.on_close.clone();
            move || DocLinkedChromeExt {
                is_split: true,
                on_split_horizontal: NoArgCallback::new(|| {}),
                on_split_vertical: NoArgCallback::new(|| {}),
                on_close: on_close.clone(),
            }
        });

        let layout = PaneLayout::Split {
            orientation,
            ratio,
            first: Box::new(PaneLayout::Leaf(Pane {
                id: PANE_ID_SOURCE,
                tabs: vec![PaneTab::with_plugin(bundle, source_plugin.clone())],
                active_tab: 0,
            })),
            second: Box::new(PaneLayout::Leaf(Pane {
                id: PANE_ID_CANVAS,
                tabs: vec![PaneTab::with_plugin(bundle, CANVAS_FACE_PLUGIN)],
                active_tab: 0,
            })),
        };

        let callbacks = PaneTreeCallbacks {
            on_tab_select: Callback::new(|_: (PaneId, usize)| {}),
            on_tab_close: Callback::new(|_: (PaneId, usize)| {}),
            on_tab_new: Callback::new(|_: PaneId| {}),
            on_split_resize: Callback::new(move |(_path, new_ratio): (Vec<SplitSide>, f32)| {
                on_resize.call(new_ratio);
            }),
            on_tab_move: Callback::new(|_: (PaneId, usize, PaneId)| {}),
            on_tab_split: Callback::new(|_: (PaneId, usize, PaneId, SplitEdge)| {}),
            on_pane_close: Callback::new(|_: PaneId| {}),
        };

        let labels = Callback::<PaneTab, String>::new(|_: PaneTab| String::new());
        let body =
            Callback::<(PaneId, PaneTab), Element>::new(move |(_pane, tab): (PaneId, PaneTab)| {
                face_element(tab.item, tab.plugin_id.as_deref())
            });

        PaneTreeView::new(layout, callbacks, labels, body)
            .show_tabs(false)
            .into_element()
    }
}

/// Render the element for one split face. `plugin_id` is read from
/// the `PaneTab`: our canvas-face sentinel maps to the infinite
/// canvas mount; anything else goes through the
/// [`linked_source`] registry so any registered source plugin wins.
fn face_element(id: ItemId, plugin_id: Option<&str>) -> Element {
    match plugin_id {
        Some(CANVAS_FACE_PLUGIN) => CanvasPane {
            mount: CanvasMount::DocLinkedCanvas(id),
        }
        .into_element(),
        Some(plugin) => source_face_element(id, plugin),
        None => missing_source_placeholder("(sin plugin)").into_element(),
    }
}

/// Look up the source backend for `plugin_id` and render its face.
/// Unknown plugins render a visible placeholder instead of panicking
/// so a bundle whose plugin was uninstalled stays navigable.
fn source_face_element(id: ItemId, plugin_id: &str) -> Element {
    match linked_source::get(plugin_id) {
        Some(backend) => backend.render_face(id),
        None => missing_source_placeholder(plugin_id).into_element(),
    }
}

fn loading_placeholder() -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::flex(1.0))
        .background(Color::from_rgb(15, 15, 18))
}

fn missing_source_placeholder(plugin_id: &str) -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(Color::from_rgb(28, 28, 32))
        .center()
        .child(
            label()
                .color(Color::from_rgb(220, 180, 100))
                .font_size(14.0)
                .text(format!("Plugin «{plugin_id}» no registrado")),
        )
}

/// Readonly PDF viewer for a persisted
/// [`crate::library::ItemKind::SourcePdf`] item.
#[derive(Debug, PartialEq)]
pub struct SourcePdfView {
    pub id: ItemId,
}

impl Component for SourcePdfView {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        let insets = crate::hooks::use_safe_area_insets();
        let pad = *insets.read();
        provide_safe_area_override(EdgeInsets::default());

        safe_area_shell(pad)
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .content(Content::Flex)
            .child(crate::builtin::pdf_linked::PdfFace { item: id })
    }
}

/// Full-window rect that reserves platform safe-area insets via padding.
/// Keep this consistent with [`crate::workspace_view`]'s sister helper —
/// absolute positioning + `.expanded()` would shift the origin without
/// shrinking the box, so the shell would hang off the right/bottom
/// edges of the window.
fn safe_area_shell(pad: EdgeInsets) -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .padding((pad.top, pad.right, pad.bottom, pad.left))
}

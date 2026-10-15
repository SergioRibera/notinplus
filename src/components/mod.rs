//! Reusable UI primitives layered on top of freya's base elements.
//!
//! Every component here is a builder in the freya idiom: no `.build()`
//! terminator, chainable setters consume `self`, and the type itself
//! implements [`Component`] (or `Into<Element>`) so it drops into
//! `.child(...)` positions unchanged.

pub mod bookmark_body_editor;
pub mod canvas_create;
pub mod chip;
pub mod color_wheel;
pub mod drag_drop;
pub mod export_bundle;
pub mod fab_menu;
pub mod folder_create;
pub mod form_input;
pub mod modal;
pub mod pane_tree_view;
pub mod settings_modal;
pub mod sidebar_modal;
pub mod tab_bar;
pub mod tag;
pub mod tag_picker;
pub mod theme;
pub mod workspace_switcher;

pub use bookmark_body_editor::BookmarkBodyEditor;
pub use canvas_create::{CanvasCreateSheet, CreateCanvasRequest};
pub use chip::Chip;
pub use color_wheel::{ColorWheel, DEFAULT_SWATCHES, auto_color};
pub use drag_drop::{DragZone, DropZone, use_drag};
pub use export_bundle::{ExportBundleRequest, ExportBundleSheet};
pub use fab_menu::{FabMenu, FabMenuEntry};
pub use folder_create::{CreateFolderRequest, FolderCreateSheet};
pub use form_input::FormInput;
pub use modal::{Modal, ModalController, ModalPlacement, ModalPortal};
pub use pane_tree_view::{PaneTreeCallbacks, PaneTreeView};
pub use settings_modal::SettingsModal;
pub use sidebar_modal::{SidebarModal, SidebarTab};
pub use tab_bar::{TabBar, TabBarEntry, TabDragPayload};
pub use tag::Tag;
pub use tag_picker::TagPicker;
pub use workspace_switcher::{WorkspaceSwitcher, WorkspaceSwitcherEntry};

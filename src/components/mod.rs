//! Reusable UI primitives layered on top of freya's base elements.
//!
//! Every component here is a builder in the freya idiom: no `.build()`
//! terminator, chainable setters consume `self`, and the type itself
//! implements [`Component`] (or `Into<Element>`) so it drops into
//! `.child(...)` positions unchanged.

pub mod canvas_create;
pub mod chip;
pub mod color_wheel;
pub mod doc_actions_menu;
pub mod drag_drop;
pub mod export_bundle;
pub mod fab_menu;
pub mod folder_create;
pub mod form_input;
pub mod graph_view;
pub mod item_picker;
pub mod modal;
pub mod note_editor;
pub mod notebook_chooser;
pub mod pane_tree_view;
pub mod settings_modal;
pub mod sidebar_modal;
pub mod space_edit;
pub mod space_strip;
pub mod tab_bar;
pub mod tag;
pub mod tag_picker;
pub mod theme;

pub use canvas_create::{CanvasCreateSheet, CreateCanvasRequest};
pub use chip::Chip;
pub use color_wheel::{ColorWheel, DEFAULT_SWATCHES, auto_color};
pub use doc_actions_menu::DocActionsMenu;
pub use drag_drop::{DragZone, DropZone, use_drag};
pub use export_bundle::{ExportBundleRequest, ExportBundleSheet};
pub use fab_menu::{FabMenu, FabMenuEntry};
pub use folder_create::{CreateFolderRequest, FolderCreateSheet};
pub use form_input::FormInput;
pub use graph_view::GraphView;
pub use item_picker::{ItemPicker, PickerChoice};
pub use modal::{Modal, ModalController, ModalPlacement, ModalPortal};
pub use note_editor::{
    ChipTapCloseCallback, NoteEditor, set_show_markdown_syntax, show_markdown_syntax,
};
pub use notebook_chooser::NotebookChooser;
pub use pane_tree_view::{PaneTreeCallbacks, PaneTreeView};
pub use settings_modal::SettingsModal;
pub use sidebar_modal::{SidebarModal, SidebarTab};
pub use space_edit::{SpaceEditRequest, SpaceEditSheet};
pub use space_strip::{SpaceCardData, SpaceStrip};
pub use tab_bar::{TabBar, TabBarEntry, TabDragPayload};
pub use tag::Tag;
pub use tag_picker::TagPicker;

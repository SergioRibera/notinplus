//! Palette constants shared across the notinplus UI.
//!
//! Colour constants live in [`notein_ui_kit::theme`]; this module
//! re-exports them so existing call sites keep working while plugins
//! consume the same values directly from the kit. Brush asset bytes
//! stay here because they ship with the stroke canvas, not with the
//! generic UI kit.

use freya::prelude::Bytes;

pub use notein_ui_kit::theme::{
    ACCENT, BORDER, BORDER_FOCUS, CARD_BG, ERROR, PANEL_BG, PRIMARY, SECONDARY, SIDEBAR_BG,
    SIDEBAR_WIDTH, SUCCESS, SURFACE_PRIMARY, SURFACE_SECONDARY, SURFACE_TERTIARY, TEXT_INVERSE,
    TEXT_MUTED, TEXT_PLACEHOLDER, TEXT_PRIMARY, TEXT_SECONDARY, WARNING,
};

pub const BALL_PEN: Bytes = Bytes::from_static(include_bytes!("../../assets/brushes/ball_pen.svg"));
pub const BRUSH_PEN: Bytes =
    Bytes::from_static(include_bytes!("../../assets/brushes/brush_pen.svg"));
pub const FOUNTAIN_PEN: Bytes =
    Bytes::from_static(include_bytes!("../../assets/brushes/fountain_pen.svg"));
pub const PENCIL: Bytes = Bytes::from_static(include_bytes!("../../assets/brushes/pencil.svg"));
pub const HIGHLIGHTER: Bytes =
    Bytes::from_static(include_bytes!("../../assets/brushes/highlighter.svg"));
pub const ERASER: Bytes = Bytes::from_static(include_bytes!("../../assets/brushes/eraser.svg"));

pub const BALL_PEN_FILL: Bytes =
    Bytes::from_static(include_bytes!("../../assets/brushes/ball_pen_fill.svg"));
pub const BRUSH_PEN_FILL: Bytes =
    Bytes::from_static(include_bytes!("../../assets/brushes/brush_pen_fill.svg"));
pub const FOUNTAIN_PEN_FILL: Bytes =
    Bytes::from_static(include_bytes!("../../assets/brushes/fountain_pen_fill.svg"));
pub const PENCIL_FILL: Bytes =
    Bytes::from_static(include_bytes!("../../assets/brushes/pencil_fill.svg"));
pub const HIGHLIGHTER_FILL: Bytes =
    Bytes::from_static(include_bytes!("../../assets/brushes/highlighter_fill.svg"));


//! Palette constants shared across the notinplus UI.
//!
//! Dark-first. Older sites (home, canvas_create) still hand-inline the
//! same rgb triples; migrate them here opportunistically as they get
//! touched. New components should reach for these names first.

use freya::prelude::{Bytes, Color};

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

pub const SURFACE_PRIMARY: Color = Color::from_rgb(15, 15, 18);
pub const SURFACE_SECONDARY: Color = Color::from_rgb(30, 30, 36);
pub const SURFACE_TERTIARY: Color = Color::from_rgb(45, 45, 52);

pub const BORDER: Color = Color::from_rgb(60, 60, 70);
pub const BORDER_FOCUS: Color = Color::from_rgb(70, 140, 250);

pub const TEXT_PRIMARY: Color = Color::from_rgb(230, 230, 240);
pub const TEXT_SECONDARY: Color = Color::from_rgb(150, 150, 165);
pub const TEXT_PLACEHOLDER: Color = Color::from_rgb(110, 110, 125);
pub const TEXT_INVERSE: Color = Color::from_rgb(15, 15, 18);

pub const PRIMARY: Color = Color::from_rgb(70, 140, 250);
pub const SECONDARY: Color = Color::from_rgb(100, 130, 200);

pub const SUCCESS: Color = Color::from_rgb(76, 175, 120);
pub const WARNING: Color = Color::from_rgb(230, 190, 105);
pub const ERROR: Color = Color::from_rgb(220, 70, 70);

pub const PANEL_BG: Color = Color::from_rgb(30, 30, 34);
pub const SIDEBAR_BG: Color = Color::from_rgb(22, 22, 26);
pub const CARD_BG: Color = Color::from_rgb(38, 38, 44);
pub const TEXT_MUTED: Color = Color::from_rgb(160, 160, 175);
pub const ACCENT: Color = Color::from_rgb(120, 170, 255);

pub const SIDEBAR_WIDTH: f32 = 320.0;

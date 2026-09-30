//! Palette constants shared across the notinplus UI.
//!
//! Dark-first. Older sites (home, canvas_create) still hand-inline the
//! same rgb triples; migrate them here opportunistically as they get
//! touched. New components should reach for these names first.

use freya::prelude::Color;

pub const SURFACE_PRIMARY: Color = Color::from_rgb(15, 15, 18);
pub const SURFACE_SECONDARY: Color = Color::from_rgb(30, 30, 36);
pub const SURFACE_TERTIARY: Color = Color::from_rgb(45, 45, 52);

pub const BORDER: Color = Color::from_rgb(70, 70, 80);
pub const BORDER_FOCUS: Color = Color::from_rgb(70, 140, 250);

pub const TEXT_PRIMARY: Color = Color::from_rgb(240, 240, 245);
pub const TEXT_SECONDARY: Color = Color::from_rgb(150, 150, 165);
pub const TEXT_PLACEHOLDER: Color = Color::from_rgb(110, 110, 125);
pub const TEXT_INVERSE: Color = Color::from_rgb(15, 15, 18);

pub const PRIMARY: Color = Color::from_rgb(70, 140, 250);
pub const SECONDARY: Color = Color::from_rgb(100, 130, 200);

pub const SUCCESS: Color = Color::from_rgb(76, 175, 120);
pub const WARNING: Color = Color::from_rgb(230, 190, 105);
pub const ERROR: Color = Color::from_rgb(220, 70, 70);

//! User settings (Ajustes): appearance, pen, notes and library.
//!
//! One record on disk, one reactive [`SettingsStore`] in the app root.
//! Screens read a section, edit it through [`SettingsStore::update`],
//! and the store writes the file and re-applies the appearance.

use freya::prelude::*;
use notein_ui_kit::theme::{Appearance, TextSize, ThemeChoice, ThemeHandle, ThemeKind};

use crate::ink_glyph::GlyphFont;
use crate::persist::Persisted;

/// Theme the user picked.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ThemePref {
    Automatic,
    #[default]
    Noche,
    Dia,
    Sepia,
    Contraste,
    Oled,
}

impl ThemePref {
    /// Settings order.
    pub const ALL: [Self; 6] = [
        Self::Automatic,
        Self::Noche,
        Self::Dia,
        Self::Sepia,
        Self::Contraste,
        Self::Oled,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automático",
            Self::Noche => "Noche",
            Self::Dia => "Día",
            Self::Sepia => "Sepia",
            Self::Contraste => "Alto contraste",
            Self::Oled => "OLED",
        }
    }
}

impl From<ThemePref> for ThemeChoice {
    fn from(pref: ThemePref) -> Self {
        match pref {
            ThemePref::Automatic => Self::Automatic,
            ThemePref::Noche => Self::Fixed(ThemeKind::Noche),
            ThemePref::Dia => Self::Fixed(ThemeKind::Dia),
            ThemePref::Sepia => Self::Fixed(ThemeKind::Sepia),
            ThemePref::Contraste => Self::Fixed(ThemeKind::Contraste),
            ThemePref::Oled => Self::Fixed(ThemeKind::Oled),
        }
    }
}

/// Interface text size step.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextSizePref {
    Small,
    #[default]
    Default,
    Large,
    Larger,
}

impl TextSizePref {
    pub const ALL: [Self; 4] = [Self::Small, Self::Default, Self::Large, Self::Larger];
}

impl From<TextSizePref> for TextSize {
    fn from(pref: TextSizePref) -> Self {
        match pref {
            TextSizePref::Small => Self::Small,
            TextSizePref::Default => Self::Default,
            TextSizePref::Large => Self::Large,
            TextSizePref::Larger => Self::Larger,
        }
    }
}

/// Settings › Apariencia.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct AppearanceSettings {
    pub theme: ThemePref,
    /// Interface accent as RGB; `None` keeps the theme's own.
    pub accent: Option<[u8; 3]>,
    pub text_size: TextSizePref,
    pub reduce_motion: bool,
}

impl From<AppearanceSettings> for Appearance {
    fn from(s: AppearanceSettings) -> Self {
        Self {
            theme: s.theme.into(),
            accent: s.accent.map(|[r, g, b]| Color::from_rgb(r, g, b)),
            text_size: s.text_size.into(),
            reduce_motion: s.reduce_motion,
        }
    }
}

/// Whether a finger draws on the canvas.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FingerDraw {
    /// Draws until a pen is seen on this device; then the finger pans.
    #[default]
    Automatic,
    Always,
    Never,
}

impl FingerDraw {
    pub const ALL: [Self; 3] = [Self::Automatic, Self::Always, Self::Never];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automático",
            Self::Always => "Sí",
            Self::Never => "No",
        }
    }

    /// Resolve against whether this device has shown a pen.
    #[must_use]
    pub const fn finger_draws(self, pen_seen: bool) -> bool {
        match self {
            Self::Automatic => !pen_seen,
            Self::Always => true,
            Self::Never => false,
        }
    }
}

/// What a pen's double tap (where supported) does.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PenDoubleTap {
    #[default]
    Eraser,
    LastBrush,
    Nothing,
}

impl PenDoubleTap {
    pub const ALL: [Self; 3] = [Self::Eraser, Self::LastBrush, Self::Nothing];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Eraser => "Cambiar a goma",
            Self::LastBrush => "Volver al último pincel",
            Self::Nothing => "Nada",
        }
    }
}

/// A creation entry by its plugin and entry ids.
#[istmo::message]
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryKey {
    pub plugin: String,
    pub entry: String,
}

impl EntryKey {
    #[must_use]
    pub fn new(plugin: impl Into<String>, entry: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            entry: entry.into(),
        }
    }
}

/// Settings › Lápiz y escritura.
#[istmo::message]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PenSettings {
    pub finger_draws: FingerDraw,
    /// Ignore hand contacts while the pen is down.
    pub palm_rejection: bool,
    pub double_tap: PenDoubleTap,
    /// Font recognised letters are redrawn in by the signature tool.
    pub smooth_font: GlyphFont,
    /// The two bubbles above the + button.
    pub fab_shortcuts: [Option<EntryKey>; 2],
}

impl Default for PenSettings {
    fn default() -> Self {
        Self {
            finger_draws: FingerDraw::Automatic,
            palm_rejection: true,
            double_tap: PenDoubleTap::Eraser,
            smooth_font: GlyphFont::Excalifont,
            fab_shortcuts: [None, None],
        }
    }
}

/// Settings › Notas.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotesSettings {
    /// Show `**markdown**` marks near the caret instead of only the
    /// formatted text.
    pub show_markdown_marks: bool,
    /// Tapping a reference chip opens the target in the other pane.
    pub open_refs_beside: bool,
    /// Quoted content keeps showing what its source holds now, instead
    /// of being copied as text of the note.
    pub live_quotes: bool,
}

impl Default for NotesSettings {
    fn default() -> Self {
        Self {
            show_markdown_marks: false,
            open_refs_beside: true,
            live_quotes: true,
        }
    }
}

/// How long trashed records are kept.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrashRetention {
    Days7,
    #[default]
    Days30,
    Days90,
    Never,
}

impl TrashRetention {
    pub const ALL: [Self; 4] = [Self::Days7, Self::Days30, Self::Days90, Self::Never];

    #[must_use]
    pub const fn days(self) -> Option<u32> {
        match self {
            Self::Days7 => Some(7),
            Self::Days30 => Some(30),
            Self::Days90 => Some(90),
            Self::Never => None,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Days7 => "7 días",
            Self::Days30 => "30 días",
            Self::Days90 => "90 días",
            Self::Never => "Nunca",
        }
    }

    /// Retention in milliseconds; `None` keeps forever.
    #[must_use]
    pub const fn millis(self) -> Option<u64> {
        crate::library::retention_millis(self.days())
    }
}

/// Order of the library grid.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Modified,
    Created,
    Name,
    Kind,
}

impl SortOrder {
    pub const ALL: [Self; 4] = [Self::Modified, Self::Created, Self::Name, Self::Kind];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Modified => "Modificado",
            Self::Created => "Creado",
            Self::Name => "Nombre",
            Self::Kind => "Tipo",
        }
    }
}

/// How the library shows its documents.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShelfLayout {
    #[default]
    Grid,
    List,
}

/// Settings › Plugins.
#[istmo::message]
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PluginPrefs {
    /// Ids of plugins the user turned off.
    pub disabled: Vec<String>,
}

/// Settings › Biblioteca, plus how Home was last arranged.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct LibrarySettings {
    pub trash_retention: TrashRetention,
    pub sort: SortOrder,
    pub layout: ShelfLayout,
}

/// Every user setting.
#[istmo::message]
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Settings {
    pub appearance: AppearanceSettings,
    pub pen: PenSettings,
    pub notes: NotesSettings,
    pub library: LibrarySettings,
    pub plugins: PluginPrefs,
}

impl Persisted for Settings {
    const FILE: &'static str = "settings.v2.bin";
}

static PEN_DOUBLE_TAP: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

impl Settings {
    /// The pen shortcut action, readable off the UI thread (the pen
    /// pump runs on its own).
    #[must_use]
    pub fn current_pen_double_tap() -> PenDoubleTap {
        match PEN_DOUBLE_TAP.load(std::sync::atomic::Ordering::Relaxed) {
            1 => PenDoubleTap::LastBrush,
            2 => PenDoubleTap::Nothing,
            _ => PenDoubleTap::Eraser,
        }
    }

    fn publish(&self) {
        let code = match self.pen.double_tap {
            PenDoubleTap::Eraser => 0,
            PenDoubleTap::LastBrush => 1,
            PenDoubleTap::Nothing => 2,
        };
        PEN_DOUBLE_TAP.store(code, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Reactive settings, provided once at the app root.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SettingsStore {
    state: State<Settings>,
    theme: ThemeHandle,
}

impl std::fmt::Debug for SettingsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsStore").finish_non_exhaustive()
    }
}

impl SettingsStore {
    /// The store installed by [`use_settings_root`].
    #[must_use]
    pub fn get() -> Self {
        consume_root_context::<Self>()
    }

    /// The store, when called inside the app.
    #[must_use]
    pub fn try_get() -> Option<Self> {
        try_consume_root_context::<Self>()
    }

    /// Current settings; subscribes the caller.
    #[must_use]
    pub fn read(&self) -> Settings {
        self.state.read().clone()
    }

    /// Current settings without subscribing (event handlers).
    #[must_use]
    pub fn peek(&self) -> Settings {
        self.state.peek().clone()
    }

    /// Edit, persist and re-apply.
    pub fn update(&mut self, edit: impl FnOnce(&mut Settings)) {
        edit(&mut self.state.write());
        let settings = self.state.peek().clone();
        settings.publish();
        self.theme.set(settings.appearance.into());
        if let Err(err) = settings.save() {
            log::error!("settings save: {err}");
        }
    }
}

/// Load settings and install both the store and the theme root.
#[must_use]
pub fn use_settings_root() -> SettingsStore {
    let state = use_state(|| {
        let settings = Settings::load();
        settings.publish();
        settings
    });
    let theme = notein_ui_kit::theme::use_theme_root(move || state.peek().appearance.into());
    use_hook(move || {
        let store = SettingsStore { state, theme };
        provide_context_for_scope_id(store, ScopeId::ROOT);
        store
    })
}

#[cfg(test)]
mod tests {
    use super::FingerDraw;

    #[test]
    fn automatic_finger_draw_follows_pen_presence() {
        assert!(FingerDraw::Automatic.finger_draws(false));
        assert!(!FingerDraw::Automatic.finger_draws(true));
        assert!(FingerDraw::Always.finger_draws(true));
        assert!(!FingerDraw::Never.finger_draws(false));
    }
}

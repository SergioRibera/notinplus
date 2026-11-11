//! Reactive appearance: what the user chose, and the palette it
//! resolves to for the current operating system preference.

use freya::prelude::*;

use super::kind::{ThemeChoice, ThemeKind};
use super::palette::Palette;

/// Interface text size. Applied as a scale factor on the whole shell;
/// the Bible and PDF faces keep their own zoom on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Large,
    Larger,
}

impl TextSize {
    /// Every size, smallest first.
    pub const ALL: [Self; 4] = [Self::Small, Self::Default, Self::Large, Self::Larger];

    /// Narrowest short side, in points, a larger size may leave the
    /// window with; past it the text grows no further, so a phone at
    /// «Más grande» keeps a usable layout.
    pub const MIN_SHORT_SIDE: f64 = 360.0;

    /// Multiplier applied to the platform scale factor, before
    /// [`Self::MIN_SHORT_SIDE`] caps it.
    #[must_use]
    pub const fn scale(self) -> f64 {
        match self {
            Self::Small => 0.9,
            Self::Default => 1.0,
            Self::Large => 1.12,
            Self::Larger => 1.25,
        }
    }
}

/// Everything Settings › Apariencia stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Appearance {
    pub theme: ThemeChoice,
    /// Interface accent; `None` keeps the theme's own.
    pub accent: Option<Color>,
    pub text_size: TextSize,
    /// No panel animations or flashes on reference jumps.
    pub reduce_motion: bool,
}

/// The appearance resolved against the operating system.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedTheme {
    pub kind: ThemeKind,
    pub palette: Palette,
    /// The chosen accent had to be lightened or darkened in this theme
    /// to stay readable.
    pub accent_adjusted: bool,
}

impl ResolvedTheme {
    /// Resolve `appearance` with the operating system preferring dark
    /// (`os_dark`) or light surfaces.
    #[must_use]
    pub fn new(appearance: Appearance, os_dark: bool) -> Self {
        let kind = appearance.theme.resolve(os_dark);
        let base = kind.base();
        let (palette, accent_adjusted) = appearance
            .accent
            .map_or((base, false), |accent| base.with_accent(accent));
        Self {
            kind,
            palette,
            accent_adjusted,
        }
    }
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        Self::new(Appearance::default(), true)
    }
}

/// Handle to the app-wide appearance, provided once at the root by
/// [`use_theme_root`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ThemeHandle {
    appearance: State<Appearance>,
    resolved: Memo<ResolvedTheme>,
}

impl std::fmt::Debug for ThemeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThemeHandle").finish_non_exhaustive()
    }
}

impl ThemeHandle {
    /// Current appearance; subscribes the caller.
    #[must_use]
    pub fn appearance(&self) -> Appearance {
        *self.appearance.read()
    }

    /// Current resolved theme; subscribes the caller.
    #[must_use]
    pub fn resolved(&self) -> ResolvedTheme {
        *self.resolved.read()
    }

    /// Replace the appearance. Every component that read the palette
    /// re-renders.
    pub fn set(&mut self, appearance: Appearance) {
        self.appearance.set(appearance);
    }

    /// Apply `edit` to the current appearance.
    pub fn update(&mut self, edit: impl FnOnce(&mut Appearance)) {
        edit(&mut self.appearance.write());
    }
}

/// Install the appearance at the app root. Also keeps Freya's own
/// component theme and the interface scale in step with it.
#[cfg_attr(feature = "sismo", sismo::function("kit::use_theme_root"))]
pub fn use_theme_root(initial: impl FnOnce() -> Appearance) -> ThemeHandle {
    let appearance = use_state(initial);
    let resolved = use_memo(move || {
        #[cfg(feature = "sismo")]
        sismo::scope!("kit::theme_resolve");
        let os_dark =
            *Platform::get().current_window().preferred_theme.read() == PreferredTheme::Dark;
        ResolvedTheme::new(*appearance.read(), os_dark)
    });
    let handle = use_hook(|| {
        let handle = ThemeHandle {
            appearance,
            resolved,
        };
        provide_context_for_scope_id(handle, ScopeId::ROOT);
        handle
    });
    let mut freya_theme = use_init_theme(|| freya_theme_for(&resolved.read().palette));
    use_side_effect(move || {
        #[cfg(feature = "sismo")]
        sismo::scope!("kit::theme_apply");
        let palette = resolved.read().palette;
        freya_theme.set(freya_theme_for(&palette));
    });
    use_side_effect(move || {
        #[cfg(feature = "sismo")]
        sismo::scope!("kit::scale_fit");
        let platform = Platform::get().current_window();
        let wanted = appearance.read().text_size.scale();
        let size = *platform.root_size.read();
        // The window's short side as it would be at scale 1.
        let short = f64::from(size.width.min(size.height)) * *platform.custom_scale_factor.peek();
        let fits = if short > 0.0 {
            (short / TextSize::MIN_SHORT_SIDE).max(1.0)
        } else {
            wanted
        };
        platform.set_custom_scale_factor(wanted.min(fits));
    });
    handle
}

/// The app-wide handle. `None` outside an app root (tests, previews).
#[must_use]
pub fn try_theme() -> Option<ThemeHandle> {
    try_consume_root_context::<ThemeHandle>()
}

/// Palette of the current theme. Subscribes the calling component, so
/// a theme switch re-renders it. Falls back to Noche when no root
/// provided one.
#[must_use]
pub fn palette() -> Palette {
    try_theme().map_or_else(
        || ThemeKind::Noche.base(),
        |handle| handle.resolved().palette,
    )
}

/// Whether the user asked for reduced motion.
#[must_use]
pub fn reduce_motion() -> bool {
    try_theme().is_some_and(|handle| handle.appearance().reduce_motion)
}

#[cfg_attr(feature = "sismo", sismo::function("kit::theme_freya_theme_for"))]
fn freya_theme_for(palette: &Palette) -> Theme {
    let mut theme = if palette.dark {
        dark_theme()
    } else {
        light_theme()
    };
    theme.colors = ColorsSheet {
        primary: palette.accent,
        secondary: palette.accent_soft,
        tertiary: palette.accent_text,
        success: palette.ok,
        warning: palette.warn,
        error: palette.danger,
        info: palette.kinds.canvas,
        background: palette.bg,
        surface_primary: palette.s3,
        surface_secondary: palette.s2,
        surface_tertiary: palette.s1,
        surface_inverse: palette.inv,
        surface_inverse_secondary: palette.inv_line,
        surface_inverse_tertiary: palette.inv_text,
        border: palette.line,
        border_focus: palette.focus,
        border_disabled: palette.line,
        text_primary: palette.t1,
        text_secondary: palette.t2,
        text_placeholder: palette.t3,
        text_inverse: palette.on_accent,
        text_highlight: palette.accent,
        focus: palette.accent_soft,
        active: palette.s4,
        disabled: palette.s3,
        overlay: palette.scrim,
        shadow: palette.shadow,
    };
    theme
}

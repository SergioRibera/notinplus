//! The five built-in themes and the user's choice among them.

use freya::prelude::Color;

use super::palette::{KindColors, Palette};

const fn hex(rgb: u32) -> Color {
    Color::new(0xFF00_0000 | rgb)
}

/// One of the shipped colour themes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ThemeKind {
    /// Default. Long sessions in low light.
    #[default]
    Noche,
    /// Bright rooms: white surfaces, darker ink.
    Dia,
    /// Warm paper for long reading at night.
    Sepia,
    /// Text ≥ 7:1, visible borders, 3 px yellow focus.
    Contraste,
    /// Pure black for AMOLED screens.
    Oled,
}

impl ThemeKind {
    /// Every theme, in the order the settings show them.
    pub const ALL: [Self; 5] = [
        Self::Noche,
        Self::Dia,
        Self::Sepia,
        Self::Contraste,
        Self::Oled,
    ];

    /// User-facing name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Noche => "Noche",
            Self::Dia => "Día",
            Self::Sepia => "Sepia",
            Self::Contraste => "Alto contraste",
            Self::Oled => "OLED",
        }
    }

    /// Tokens of the theme with its own default accent.
    #[must_use]
    pub const fn base(self) -> Palette {
        match self {
            Self::Noche => NOCHE,
            Self::Dia => DIA,
            Self::Sepia => SEPIA,
            Self::Contraste => CONTRASTE,
            Self::Oled => OLED,
        }
    }
}

/// What the user picked in Settings › Apariencia.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThemeChoice {
    /// Follow the operating system: Día when it is light, Noche when
    /// it is dark.
    Automatic,
    Fixed(ThemeKind),
}

impl Default for ThemeChoice {
    fn default() -> Self {
        Self::Fixed(ThemeKind::Noche)
    }
}

impl ThemeChoice {
    /// Concrete theme for an operating system preference.
    #[must_use]
    pub const fn resolve(self, os_prefers_dark: bool) -> ThemeKind {
        match self {
            Self::Automatic if os_prefers_dark => ThemeKind::Noche,
            Self::Automatic => ThemeKind::Dia,
            Self::Fixed(kind) => kind,
        }
    }
}

const HIGHLIGHTERS: [Color; 5] = [
    hex(0xF2_D5_6B),
    hex(0x9B_D6_7C),
    hex(0xF2_9B_B0),
    hex(0x7F_C4_F0),
    hex(0xB7_9C_F2),
];

const NOCHE: Palette = Palette {
    dark: true,
    bg: hex(0x11_11_13),
    s1: hex(0x18_18_1B),
    s2: hex(0x20_20_24),
    s3: hex(0x2A_2A_2F),
    s4: hex(0x34_34_3A),
    line: hex(0x2E_2E_34),
    line_strong: hex(0x45_45_4D),
    t1: hex(0xED_EB_E6),
    t2: hex(0xA8_A5_9F),
    t3: hex(0x9A_97_91),
    accent: hex(0x7B_8C_FF),
    accent_soft: hex(0x26_2A_45),
    accent_text: hex(0xC9_D0_FF),
    on_accent: hex(0x0E_10_24),
    kinds: KindColors {
        canvas: hex(0x7B_8C_FF),
        pdf: hex(0xE8_73_5E),
        note: hex(0x6D_B8_8B),
        bible: hex(0xD9_A5_4A),
        folder: hex(0x9A_95_8C),
        scratch: hex(0x9B_7B_EA),
    },
    highlighters: HIGHLIGHTERS,
    paper: hex(0xF7_F5_F0),
    paper_line: hex(0xDE_DA_D0),
    paper_ink: hex(0x26_25_2A),
    danger: hex(0xE5_60_4F),
    ok: hex(0x6D_B8_8B),
    warn: hex(0xE3_B3_5B),
    inv: hex(0x2C_2C_33),
    inv_text: hex(0xED_EB_E6),
    inv_line: hex(0x45_45_4D),
    focus: hex(0x7B_8C_FF),
    focus_width: 2.0,
    scrim: Color::from_argb(128, 0, 0, 0),
    shadow: Color::from_argb(77, 0, 0, 0),
};

const DIA: Palette = Palette {
    dark: false,
    bg: hex(0xF4_F3_EF),
    s1: hex(0xFF_FF_FF),
    s2: hex(0xFF_FF_FF),
    s3: hex(0xEE_EC_E7),
    s4: hex(0xE3_E0_D9),
    line: hex(0xE3_E0_D9),
    line_strong: hex(0xCB_C7_BE),
    t1: hex(0x1C_1B_20),
    t2: hex(0x57_55_4F),
    t3: hex(0x6B_68_61),
    accent: hex(0x44_57_E0),
    accent_soft: hex(0xE5_E8_FC),
    accent_text: hex(0x2C_3A_A8),
    on_accent: hex(0xFF_FF_FF),
    kinds: KindColors {
        canvas: hex(0x44_57_E0),
        pdf: hex(0xC8_49_2F),
        note: hex(0x2E_8A_57),
        bible: hex(0xA8_74_12),
        folder: hex(0x76_71_6A),
        scratch: hex(0x73_50_D0),
    },
    highlighters: HIGHLIGHTERS,
    paper: hex(0xFF_FF_FF),
    paper_line: hex(0xDE_DA_D0),
    paper_ink: hex(0x26_25_2A),
    danger: hex(0xC8_39_2B),
    ok: hex(0x2E_8A_57),
    warn: hex(0xA8_74_12),
    inv: hex(0x26_25_2B),
    inv_text: hex(0xF4_F3_EF),
    inv_line: hex(0x45_45_4D),
    focus: hex(0x44_57_E0),
    focus_width: 2.0,
    scrim: Color::from_argb(102, 0, 0, 0),
    shadow: Color::from_argb(46, 0, 0, 0),
};

const SEPIA: Palette = Palette {
    dark: false,
    bg: hex(0xF1_E7_D3),
    s1: hex(0xF7_EF_DF),
    s2: hex(0xFB_F6_EA),
    s3: hex(0xE9_DC_C3),
    s4: hex(0xDE_CD_AE),
    line: hex(0xE2_D4_B9),
    line_strong: hex(0xC8_B6_93),
    t1: hex(0x3A_2E_21),
    t2: hex(0x64_54_3F),
    t3: hex(0x6E_5D_47),
    accent: hex(0x8A_4F_1D),
    accent_soft: hex(0xEB_D9_BB),
    accent_text: hex(0x6A_3A_12),
    on_accent: hex(0xFF_FF_FF),
    kinds: KindColors {
        canvas: hex(0x5A_63_B8),
        pdf: hex(0xB4_47_2F),
        note: hex(0x3E_7D_52),
        bible: hex(0x9A_65_12),
        folder: hex(0x7A_6A_53),
        scratch: hex(0x7A_55_B5),
    },
    highlighters: HIGHLIGHTERS,
    paper: hex(0xFB_F6_EA),
    paper_line: hex(0xDE_DA_D0),
    paper_ink: hex(0x26_25_2A),
    danger: hex(0xB0_3A_28),
    ok: hex(0x3E_7D_52),
    warn: hex(0x9A_65_12),
    inv: hex(0x3A_2E_21),
    inv_text: hex(0xF7_EF_DF),
    inv_line: hex(0x45_45_4D),
    focus: hex(0x8A_4F_1D),
    focus_width: 2.0,
    scrim: Color::from_argb(102, 40, 28, 12),
    shadow: Color::from_argb(46, 40, 28, 12),
};

const CONTRASTE: Palette = Palette {
    dark: true,
    bg: hex(0x00_00_00),
    s1: hex(0x0A_0A_0A),
    s2: hex(0x14_14_14),
    s3: hex(0x22_22_22),
    s4: hex(0x33_33_33),
    line: hex(0x9A_9A_9A),
    line_strong: hex(0xFF_FF_FF),
    t1: hex(0xFF_FF_FF),
    t2: hex(0xED_ED_ED),
    t3: hex(0xCF_CF_CF),
    accent: hex(0x9D_B6_FF),
    accent_soft: hex(0x1C_2A_5C),
    accent_text: hex(0xFF_FF_FF),
    on_accent: hex(0x00_00_00),
    kinds: KindColors {
        canvas: hex(0x9D_B6_FF),
        pdf: hex(0xFF_9C_87),
        note: hex(0x7F_E0_A6),
        bible: hex(0xFF_D1_66),
        folder: hex(0xD6_D2_CA),
        scratch: hex(0xC9_A8_FF),
    },
    highlighters: HIGHLIGHTERS,
    paper: hex(0xF7_F5_F0),
    paper_line: hex(0xDE_DA_D0),
    paper_ink: hex(0x26_25_2A),
    danger: hex(0xFF_7A_6B),
    ok: hex(0x7F_E0_A6),
    warn: hex(0xFF_D1_66),
    inv: hex(0xFF_FF_FF),
    inv_text: hex(0x00_00_00),
    inv_line: hex(0x00_00_00),
    focus: hex(0xFF_D4_00),
    focus_width: 3.0,
    scrim: Color::from_argb(179, 0, 0, 0),
    shadow: Color::from_argb(0, 0, 0, 0),
};

const OLED: Palette = Palette {
    bg: hex(0x00_00_00),
    s1: hex(0x0A_0A_0B),
    s2: hex(0x13_13_15),
    s3: hex(0x1C_1C_1F),
    s4: hex(0x28_28_2C),
    line: hex(0x1F_1F_23),
    line_strong: hex(0x36_36_3C),
    accent_soft: hex(0x1A_1D_33),
    inv: hex(0x1C_1C_1F),
    inv_text: hex(0xEC_EA_E6),
    ..NOCHE
};

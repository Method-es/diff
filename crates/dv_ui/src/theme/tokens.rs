//! The Glass token tables (DESIGN §1.1 dark, §1.2 light), as the design canvas's `tokens()`
//! writes them: every surface token with its Acrylic / Mica / Opaque value, then the solid,
//! diff and syntax tokens, which are the same over every material.
//!
//! Translucent surfaces carry more tint over Mica, which samples only the wallpaper, than over
//! Acrylic, and none over Opaque (DESIGN §1). The tables are plain sRGB values; [`super::DvColors`]
//! resolves one appearance × material into the colours views use.

use gpui::Rgba;

/// `#rrggbb`.
pub const fn hex(rgb: u32) -> Rgba {
    rgba(
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
        1.,
    )
}

/// `rgba(r, g, b, a)` as CSS writes it.
pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a,
    }
}

/// A surface token's three values (DESIGN §1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub acrylic: Rgba,
    pub mica: Rgba,
    pub opaque: Rgba,
}

const fn surface(acrylic: Rgba, mica: Rgba, opaque: Rgba) -> Surface {
    Surface {
        acrylic,
        mica,
        opaque,
    }
}

/// A surface whose value doesn't change with the material.
const fn same(value: Rgba) -> Surface {
    surface(value, value, value)
}

/// The material-dependent surfaces (DESIGN §1.1 table; `recess` is the canvas's inset well).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceTokens {
    /// Title bar, status bar, PR strip.
    pub chrome: Surface,
    /// File list / comments column.
    pub panel: Surface,
    /// The diff surface: near-opaque for legibility.
    pub code: Surface,
    /// The file header row in the diff.
    pub code_head: Surface,
    /// Inset wells (the canvas's `recess`).
    pub recess: Surface,
    /// Cards on material.
    pub layer: Surface,
    /// Selected / active cards.
    pub layer_strong: Surface,
    /// The inline thread card.
    pub thread: Surface,
    /// Sheets, palette, menus.
    pub overlay: Surface,
    /// Button fill.
    pub ctrl: Surface,
    /// Text inputs.
    pub input_bg: Surface,
}

/// Tokens that are the same over every material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolidTokens {
    pub fg: Rgba,
    pub fg2: Rgba,
    /// The lowest text tier; at least 4.5:1 on every surface.
    pub fg3: Rgba,
    /// Accent as a fill.
    pub accent: Rgba,
    /// Text and glyphs on an accent fill.
    pub accent_fg: Rgba,
    /// Accent as text.
    pub accent_text: Rgba,
    pub accent_soft: Rgba,
    /// The primary button's outline.
    pub accent_edge: Rgba,
    /// The open-thread dot and focus rings.
    pub accent_ring: Rgba,
    pub green: Rgba,
    pub red: Rgba,
    pub yellow: Rgba,
    pub blue: Rgba,
    pub green_soft: Rgba,
    pub red_soft: Rgba,
    pub yellow_soft: Rgba,
    pub blue_soft: Rgba,
    /// The outline of a warning row.
    pub warn_line: Rgba,
    pub line: Rgba,
    pub line2: Rgba,
    pub ctrl_line: Rgba,
    pub overlay_line: Rgba,
    pub scrim: Rgba,
    /// The selected segment of a segmented control.
    pub seg_on: Rgba,
    /// Keycap fill (`kbd`).
    pub kbd_bg: Rgba,
    /// Selected rows and text.
    pub sel: Rgba,
    /// Progress tracks.
    pub track: Rgba,
    /// Inline code fill.
    pub icode_bg: Rgba,
    pub hunk: Rgba,
    pub add_bg: Rgba,
    pub add_gut: Rgba,
    pub add_word: Rgba,
    pub del_bg: Rgba,
    pub del_gut: Rgba,
    pub del_word: Rgba,
}

/// Syntax token kinds' colours (DESIGN §1.1, §1.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntaxTokens {
    pub keyword: Rgba,
    pub function: Rgba,
    pub string: Rgba,
    pub number: Rgba,
    pub ty: Rgba,
    pub constant: Rgba,
    pub property: Rgba,
    pub punctuation: Rgba,
    pub plain: Rgba,
}

/// One appearance's full token set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenSet {
    pub surfaces: SurfaceTokens,
    pub solid: SolidTokens,
    pub syntax: SyntaxTokens,
}

/// Aura Dark on glass (DESIGN §1.1).
pub const DARK: TokenSet = TokenSet {
    surfaces: SurfaceTokens {
        chrome: surface(
            rgba(26, 24, 36, 0.56),
            rgba(28, 26, 37, 0.82),
            hex(0x110f18),
        ),
        panel: surface(
            rgba(20, 19, 29, 0.44),
            rgba(24, 22, 32, 0.76),
            hex(0x13121a),
        ),
        code: surface(
            rgba(21, 20, 27, 0.94),
            rgba(21, 20, 27, 0.975),
            hex(0x15141b),
        ),
        code_head: surface(
            rgba(28, 26, 37, 0.95),
            rgba(27, 26, 35, 0.98),
            hex(0x1b1a23),
        ),
        recess: surface(rgba(0, 0, 0, 0.20), rgba(0, 0, 0, 0.20), hex(0x100f15)),
        layer: surface(
            rgba(255, 255, 255, 0.045),
            rgba(255, 255, 255, 0.04),
            hex(0x1a1922),
        ),
        layer_strong: surface(
            rgba(32, 30, 44, 0.82),
            rgba(32, 30, 43, 0.90),
            hex(0x1e1c28),
        ),
        thread: surface(
            rgba(31, 29, 41, 0.98),
            rgba(31, 29, 41, 0.99),
            hex(0x1d1b26),
        ),
        overlay: surface(
            rgba(31, 29, 42, 0.965),
            rgba(31, 29, 42, 0.975),
            hex(0x1d1b27),
        ),
        ctrl: surface(
            rgba(255, 255, 255, 0.06),
            rgba(255, 255, 255, 0.055),
            hex(0x1f1d29),
        ),
        input_bg: surface(rgba(0, 0, 0, 0.24), rgba(0, 0, 0, 0.22), hex(0x0f0e15)),
    },
    solid: SolidTokens {
        fg: hex(0xedecee),
        fg2: hex(0xc6c4d8),
        fg3: hex(0x9d9ab3),
        accent: hex(0xa277ff),
        accent_fg: hex(0x15141b),
        accent_text: hex(0xc4a9ff),
        accent_soft: rgba(162, 119, 255, 0.18),
        accent_edge: hex(0xb591ff),
        accent_ring: rgba(162, 119, 255, 0.55),
        green: hex(0x61ffca),
        red: hex(0xff6767),
        yellow: hex(0xffca85),
        blue: hex(0x82e2ff),
        green_soft: rgba(97, 255, 202, 0.12),
        red_soft: rgba(255, 103, 103, 0.12),
        yellow_soft: rgba(255, 202, 133, 0.12),
        blue_soft: rgba(130, 226, 255, 0.13),
        warn_line: rgba(255, 202, 133, 0.38),
        line: rgba(255, 255, 255, 0.08),
        line2: rgba(255, 255, 255, 0.14),
        ctrl_line: rgba(255, 255, 255, 0.11),
        overlay_line: rgba(255, 255, 255, 0.13),
        scrim: rgba(6, 5, 10, 0.56),
        seg_on: rgba(255, 255, 255, 0.12),
        kbd_bg: rgba(255, 255, 255, 0.06),
        sel: rgba(162, 119, 255, 0.22),
        track: rgba(255, 255, 255, 0.10),
        icode_bg: rgba(255, 255, 255, 0.08),
        hunk: rgba(130, 226, 255, 0.06),
        add_bg: rgba(97, 255, 202, 0.085),
        add_gut: rgba(97, 255, 202, 0.15),
        add_word: rgba(97, 255, 202, 0.30),
        del_bg: rgba(255, 103, 103, 0.09),
        del_gut: rgba(255, 103, 103, 0.16),
        del_word: rgba(255, 103, 103, 0.34),
    },
    syntax: SyntaxTokens {
        keyword: hex(0xa277ff),
        function: hex(0xffca85),
        string: hex(0x61ffca),
        number: hex(0x61ffca),
        ty: hex(0xa277ff),
        constant: hex(0x82e2ff),
        property: hex(0xf694ff),
        punctuation: hex(0xedecee),
        plain: hex(0xedecee),
    },
};

/// Aura Light on glass (DESIGN §1.2; the full set is the canvas's `tokens()`).
pub const LIGHT: TokenSet = TokenSet {
    surfaces: SurfaceTokens {
        chrome: surface(
            rgba(247, 246, 251, 0.58),
            rgba(239, 238, 244, 0.85),
            hex(0xefeef4),
        ),
        panel: surface(
            rgba(251, 250, 254, 0.46),
            rgba(243, 242, 248, 0.80),
            hex(0xf5f4f9),
        ),
        code: surface(
            rgba(255, 255, 255, 0.93),
            rgba(254, 254, 255, 0.975),
            hex(0xfdfdfe),
        ),
        code_head: surface(
            rgba(249, 248, 252, 0.95),
            rgba(248, 247, 251, 0.98),
            hex(0xf7f6fa),
        ),
        recess: same(rgba(28, 24, 56, 0.035)),
        layer: surface(
            rgba(255, 255, 255, 0.50),
            rgba(255, 255, 255, 0.55),
            hex(0xfafafc),
        ),
        layer_strong: surface(
            rgba(255, 255, 255, 0.86),
            rgba(255, 255, 255, 0.93),
            hex(0xffffff),
        ),
        thread: same(hex(0xf7f6fb)),
        overlay: surface(
            rgba(252, 251, 255, 0.97),
            rgba(252, 251, 255, 0.98),
            hex(0xffffff),
        ),
        ctrl: surface(
            rgba(255, 255, 255, 0.70),
            rgba(255, 255, 255, 0.75),
            hex(0xffffff),
        ),
        input_bg: surface(
            rgba(255, 255, 255, 0.72),
            rgba(255, 255, 255, 0.80),
            hex(0xffffff),
        ),
    },
    solid: SolidTokens {
        fg: hex(0x1d1b26),
        fg2: hex(0x46434f),
        fg3: hex(0x5e5a6f),
        accent: hex(0x6d43e0),
        accent_fg: hex(0xffffff),
        accent_text: hex(0x5a32cc),
        accent_soft: rgba(109, 67, 224, 0.12),
        accent_edge: hex(0x5d35d0),
        accent_ring: rgba(109, 67, 224, 0.45),
        green: hex(0x0d7553),
        red: hex(0xbf2537),
        yellow: hex(0x875600),
        blue: hex(0x0b6687),
        green_soft: rgba(13, 117, 83, 0.10),
        red_soft: rgba(191, 37, 55, 0.09),
        yellow_soft: rgba(160, 104, 0, 0.11),
        blue_soft: rgba(11, 102, 135, 0.10),
        warn_line: rgba(135, 86, 0, 0.32),
        line: rgba(28, 24, 56, 0.09),
        line2: rgba(28, 24, 56, 0.16),
        ctrl_line: rgba(28, 24, 56, 0.14),
        overlay_line: rgba(28, 24, 56, 0.14),
        scrim: rgba(36, 32, 58, 0.30),
        seg_on: hex(0xffffff),
        kbd_bg: rgba(255, 255, 255, 0.80),
        sel: rgba(109, 67, 224, 0.13),
        track: rgba(28, 24, 56, 0.10),
        icode_bg: rgba(28, 24, 56, 0.07),
        hunk: rgba(11, 102, 135, 0.06),
        add_bg: rgba(18, 170, 115, 0.10),
        add_gut: rgba(18, 170, 115, 0.17),
        add_word: rgba(18, 170, 115, 0.30),
        del_bg: rgba(225, 45, 70, 0.075),
        del_gut: rgba(225, 45, 70, 0.14),
        del_word: rgba(225, 45, 70, 0.24),
    },
    syntax: SyntaxTokens {
        keyword: hex(0x6236d6),
        function: hex(0x8a4f00),
        string: hex(0x0d7553),
        number: hex(0x0d7553),
        ty: hex(0x6236d6),
        constant: hex(0x0b6687),
        property: hex(0x9a2694),
        punctuation: hex(0x3d3a48),
        plain: hex(0x1d1b26),
    },
};

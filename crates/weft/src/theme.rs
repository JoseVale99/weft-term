//! Built-in color themes.
//!
//! A theme maps the terminal's default colors and the 16 ANSI indexes to RGB.
//! 256-index colors and direct RGB values pass through untouched, because the
//! program that emitted them already chose a color.
//!
//! Themes are defined here instead of in the configuration file so a missing or
//! misspelled name falls back to a known palette and never renders unreadable
//! text.

/// An RGB color.
pub type Rgb = (u8, u8, u8);

/// A palette plus the default foreground and background.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Stable name written in the configuration file.
    pub name: &'static str,
    /// Default text color.
    pub foreground: Rgb,
    /// Default background color.
    pub background: Rgb,
    /// ANSI colors 0 through 15.
    pub palette: [Rgb; 16],
}

/// Default dark theme.
pub const DARK: Theme = Theme {
    name: "weft-dark",
    foreground: (0xd6, 0xd9, 0xe0),
    background: (0x12, 0x15, 0x1a),
    palette: [
        (0x2a, 0x2f, 0x38),
        (0xe0, 0x6c, 0x75),
        (0x98, 0xc3, 0x79),
        (0xe5, 0xc0, 0x7b),
        (0x61, 0xaf, 0xef),
        (0xc6, 0x78, 0xdd),
        (0x56, 0xb6, 0xc2),
        (0xdf, 0xe6, 0xee),
        (0x5c, 0x63, 0x70),
        (0xef, 0x7d, 0x87),
        (0xb3, 0xd8, 0x9c),
        (0xf0, 0xd1, 0x97),
        (0x82, 0xc3, 0xf5),
        (0xd9, 0x9a, 0xe9),
        (0x7f, 0xd1, 0xdb),
        (0xff, 0xff, 0xff),
    ],
};

/// Light theme for bright rooms and projectors.
pub const LIGHT: Theme = Theme {
    name: "weft-light",
    foreground: (0x30, 0x34, 0x3b),
    background: (0xfa, 0xfa, 0xfa),
    palette: [
        (0x30, 0x34, 0x3b),
        (0xc1, 0x3a, 0x44),
        (0x3f, 0x7d, 0x32),
        (0x8a, 0x63, 0x00),
        (0x1f, 0x5f, 0xa8),
        (0x8a, 0x3f, 0xa8),
        (0x0f, 0x76, 0x7d),
        (0xe8, 0xe8, 0xe8),
        (0x6b, 0x70, 0x78),
        (0xdb, 0x50, 0x5a),
        (0x5c, 0x9a, 0x4f),
        (0xa8, 0x7d, 0x00),
        (0x36, 0x7d, 0xc8),
        (0xa8, 0x5a, 0xc6),
        (0x2a, 0x98, 0xa0),
        (0x00, 0x00, 0x00),
    ],
};

/// Every theme the binary knows, in the order shown to the user.
pub const BUILTIN: [&Theme; 2] = [&DARK, &LIGHT];

/// Resolves a configured theme name, falling back to [`DARK`].
///
/// `dark` and `light` are accepted as aliases so a hand-written configuration
/// does not need to know the exact product name.
pub fn resolve(name: &str) -> &'static Theme {
    match name.trim().to_ascii_lowercase().as_str() {
        "weft-light" | "light" => &LIGHT,
        _ => &DARK,
    }
}

/// Names of the built-in themes.
pub fn names() -> Vec<&'static str> {
    BUILTIN.iter().map(|theme| theme.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_aliases_resolve() {
        assert_eq!(resolve("weft-light").name, "weft-light");
        assert_eq!(resolve(" Light ").name, "weft-light");
        assert_eq!(resolve("weft-dark").name, "weft-dark");
        assert_eq!(resolve("nope").name, "weft-dark");
    }

    #[test]
    fn every_palette_has_sixteen_colors() {
        for theme in BUILTIN {
            assert_eq!(theme.palette.len(), 16);
            assert_eq!(names().contains(&theme.name), true);
        }
    }

    #[test]
    fn dark_and_light_are_readable() {
        let contrast = |a: Rgb, b: Rgb| {
            let luma = |c: Rgb| {
                0.2126 * f64::from(c.0) + 0.7152 * f64::from(c.1) + 0.0722 * f64::from(c.2)
            };
            (luma(a) - luma(b)).abs()
        };
        assert!(contrast(DARK.foreground, DARK.background) > 100.0);
        assert!(contrast(LIGHT.foreground, LIGHT.background) > 100.0);
    }
}

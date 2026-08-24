use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThemePreset {
    #[default]
    #[serde(alias = "midnight")]
    TokyoNight,
    #[serde(alias = "graphite")]
    Dracula,
    Nord,
    CatppuccinLatte,
    CatppuccinFrappe,
    CatppuccinMacchiato,
    CatppuccinMocha,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TerminalTheme {
    pub(crate) foreground: [u8; 3],
    pub(crate) background: [u8; 3],
    pub(crate) cursor: [u8; 3],
    pub(crate) selection_background: [u8; 3],
    pub(crate) palette: [[u8; 3]; 16],
}

impl ThemePreset {
    pub(crate) const ALL: [Self; 7] = [
        Self::TokyoNight,
        Self::Dracula,
        Self::Nord,
        Self::CatppuccinLatte,
        Self::CatppuccinFrappe,
        Self::CatppuccinMacchiato,
        Self::CatppuccinMocha,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::TokyoNight => "TokyoNight",
            Self::Dracula => "Dracula",
            Self::Nord => "Nord",
            Self::CatppuccinLatte => "Catppuccin Latte",
            Self::CatppuccinFrappe => "Catppuccin Frappé",
            Self::CatppuccinMacchiato => "Catppuccin Macchiato",
            Self::CatppuccinMocha => "Catppuccin Mocha",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::TokyoNight => "Bundled Ghostty theme",
            Self::Dracula => "Bundled Ghostty theme",
            Self::Nord => "Bundled Ghostty theme",
            Self::CatppuccinLatte => "Bundled Ghostty light theme",
            Self::CatppuccinFrappe => "Bundled Ghostty theme",
            Self::CatppuccinMacchiato => "Bundled Ghostty theme",
            Self::CatppuccinMocha => "Bundled Ghostty theme",
        }
    }

    pub(crate) const fn terminal_theme(self) -> TerminalTheme {
        // Exact values from the Ghostty theme archive pinned by
        // vendor/ghostty/build.zig.zon (`iterm2_themes`). Keeping these here
        // makes the same Ghostty palette authoritative for both the terminal
        // and the surrounding application shell.
        match self {
            Self::TokyoNight => theme(
                0xc0caf5,
                0x1a1b26,
                0xc0caf5,
                0x33467c,
                [
                    0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
                    0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
                ],
            ),
            Self::Dracula => theme(
                0xf8f8f2,
                0x282a36,
                0xf8f8f2,
                0x44475a,
                [
                    0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
                    0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
                ],
            ),
            Self::Nord => theme(
                0xd8dee9,
                0x2e3440,
                0xeceff4,
                0xeceff4,
                [
                    0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
                    0x596377, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
                ],
            ),
            Self::CatppuccinLatte => theme(
                0x4c4f69,
                0xeff1f5,
                0xdc8a78,
                0xdc8a78,
                [
                    0xbcc0cc, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0x5c5f77,
                    0xacb0be, 0xe7103f, 0x46b02f, 0xe49931, 0x3878f6, 0xef95d7, 0x19a1a8, 0x6c6f85,
                ],
            ),
            Self::CatppuccinFrappe => theme(
                0xc6d0f5,
                0x303446,
                0xf2d5cf,
                0xf2d5cf,
                [
                    0x51576d, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xb5bfe2,
                    0x626880, 0xeda0a2, 0xb9dba2, 0xecd7ae, 0xadc2f3, 0xf38ed8, 0x98d2ca, 0xa5adce,
                ],
            ),
            Self::CatppuccinMacchiato => theme(
                0xcad3f5,
                0x24273a,
                0xf4dbd6,
                0xf4dbd6,
                [
                    0x494d64, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xb8c0e0,
                    0x5b6078, 0xf2a7b2, 0xbde3b0, 0xf4e3c1, 0xadc5f7, 0xf493da, 0xa5ded6, 0xa5adcb,
                ],
            ),
            Self::CatppuccinMocha => theme(
                0xcdd6f4,
                0x1e1e2e,
                0xf5e0dc,
                0xf5e0dc,
                [
                    0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
                    0x585b70, 0xf7aec2, 0xc2ecbf, 0xfcd682, 0xaeccfc, 0xf398da, 0xb1eae1, 0xa6adc8,
                ],
            ),
        }
    }
}

const fn theme(
    foreground: u32,
    background: u32,
    cursor: u32,
    selection_background: u32,
    palette: [u32; 16],
) -> TerminalTheme {
    let mut colors = [[0; 3]; 16];
    let mut index = 0;
    while index < palette.len() {
        colors[index] = color(palette[index]);
        index += 1;
    }
    TerminalTheme {
        foreground: color(foreground),
        background: color(background),
        cursor: color(cursor),
        selection_background: color(selection_background),
        palette: colors,
    }
}

const fn color(value: u32) -> [u8; 3] {
    [
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::ThemePreset;

    #[test]
    fn terminal_values_match_the_bundled_ghostty_themes() {
        let tokyo_night = ThemePreset::TokyoNight.terminal_theme();
        assert_eq!(tokyo_night.background, [0x1a, 0x1b, 0x26]);
        assert_eq!(tokyo_night.selection_background, [0x33, 0x46, 0x7c]);

        let dracula = ThemePreset::Dracula.terminal_theme();
        assert_eq!(dracula.foreground, [0xf8, 0xf8, 0xf2]);
        assert_eq!(dracula.palette[12], [0xd6, 0xac, 0xff]);

        let nord = ThemePreset::Nord.terminal_theme();
        assert_eq!(nord.cursor, [0xec, 0xef, 0xf4]);
        assert_eq!(nord.palette[8], [0x59, 0x63, 0x77]);

        let mocha = ThemePreset::CatppuccinMocha.terminal_theme();
        assert_eq!(mocha.background, [0x1e, 0x1e, 0x2e]);
        assert_eq!(mocha.foreground, [0xcd, 0xd6, 0xf4]);
        assert_eq!(mocha.cursor, [0xf5, 0xe0, 0xdc]);
        assert_eq!(mocha.palette[1], [0xf3, 0x8b, 0xa8]);
        assert_eq!(mocha.palette[4], [0x89, 0xb4, 0xfa]);
        assert_eq!(mocha.palette[15], [0xa6, 0xad, 0xc8]);
    }
}

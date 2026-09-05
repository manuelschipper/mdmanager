use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy)]
pub(crate) struct Theme {
    pub(crate) background: Color,
    pub(crate) surface: Color,
    pub(crate) surface_raised: Color,
    pub(crate) primary: Color,
    pub(crate) secondary: Color,
    pub(crate) success: Color,
    pub(crate) warning: Color,
    pub(crate) error: Color,
    pub(crate) text: Color,
    pub(crate) muted: Color,
    pub(crate) dim: Color,
    selected_text: Color,
    selected_background: Color,
}

impl Theme {
    pub(crate) fn selected(self) -> Style {
        Style::new()
            .fg(self.selected_text)
            .bg(self.selected_background)
            .add_modifier(Modifier::BOLD)
    }
}

pub(crate) const DEFAULT_NAME: &str = "gruvbox-dark";

struct Seed {
    id: &'static str,
    background: (u8, u8, u8),
    text: (u8, u8, u8),
    accent: (u8, u8, u8),
}

const SEEDS: &[Seed] = &[
    Seed {
        id: "ayu-dark",
        background: (11, 14, 20),
        text: (179, 177, 173),
        accent: (230, 180, 80),
    },
    Seed {
        id: "ayu-light",
        background: (250, 250, 250),
        text: (92, 103, 115),
        accent: (255, 153, 0),
    },
    Seed {
        id: "catppuccin-frappe",
        background: (48, 52, 70),
        text: (198, 208, 245),
        accent: (140, 170, 238),
    },
    Seed {
        id: "catppuccin-latte",
        background: (239, 241, 245),
        text: (76, 79, 105),
        accent: (30, 102, 245),
    },
    Seed {
        id: "catppuccin-macchiato",
        background: (36, 39, 58),
        text: (202, 211, 245),
        accent: (138, 173, 244),
    },
    Seed {
        id: "catppuccin-mocha",
        background: (30, 30, 46),
        text: (205, 214, 244),
        accent: (137, 180, 250),
    },
    Seed {
        id: "dracula",
        background: (40, 42, 54),
        text: (248, 248, 242),
        accent: (189, 147, 249),
    },
    Seed {
        id: "everforest-dark",
        background: (45, 53, 59),
        text: (211, 198, 170),
        accent: (167, 192, 128),
    },
    Seed {
        id: "github-dark",
        background: (13, 17, 23),
        text: (201, 209, 217),
        accent: (88, 166, 255),
    },
    Seed {
        id: "github-light",
        background: (255, 255, 255),
        text: (36, 41, 47),
        accent: (9, 105, 218),
    },
    Seed {
        id: "gruvbox-dark",
        background: (40, 40, 40),
        text: (235, 219, 178),
        accent: (215, 153, 33),
    },
    Seed {
        id: "gruvbox-light",
        background: (251, 241, 199),
        text: (60, 56, 54),
        accent: (181, 118, 20),
    },
    Seed {
        id: "kanagawa-wave",
        background: (31, 31, 40),
        text: (220, 215, 186),
        accent: (126, 156, 216),
    },
    Seed {
        id: "material-ocean",
        background: (15, 17, 26),
        text: (166, 172, 205),
        accent: (130, 170, 255),
    },
    Seed {
        id: "monokai",
        background: (39, 40, 34),
        text: (248, 248, 242),
        accent: (249, 38, 114),
    },
    Seed {
        id: "night-owl",
        background: (1, 22, 39),
        text: (214, 222, 235),
        accent: (130, 170, 255),
    },
    Seed {
        id: "nord",
        background: (46, 52, 64),
        text: (216, 222, 233),
        accent: (136, 192, 208),
    },
    Seed {
        id: "one-dark-pro",
        background: (40, 44, 52),
        text: (171, 178, 191),
        accent: (97, 175, 239),
    },
    Seed {
        id: "rose-pine",
        background: (25, 23, 36),
        text: (224, 222, 244),
        accent: (196, 167, 231),
    },
    Seed {
        id: "rose-pine-dawn",
        background: (250, 244, 237),
        text: (87, 82, 121),
        accent: (144, 122, 169),
    },
    Seed {
        id: "solarized-dark",
        background: (0, 43, 54),
        text: (131, 148, 150),
        accent: (38, 139, 210),
    },
    Seed {
        id: "solarized-light",
        background: (253, 246, 227),
        text: (101, 123, 131),
        accent: (38, 139, 210),
    },
    Seed {
        id: "tokyo-night",
        background: (26, 27, 38),
        text: (192, 202, 245),
        accent: (122, 162, 247),
    },
    Seed {
        id: "vitesse-dark",
        background: (18, 18, 18),
        text: (219, 215, 202),
        accent: (77, 147, 117),
    },
];

pub(crate) fn resolve(id: &str) -> Result<Theme, String> {
    SEEDS
        .iter()
        .find(|seed| seed.id == id)
        .map(derive)
        .ok_or_else(|| {
            format!(
                "unknown theme {id}; expected one of {}",
                names().collect::<Vec<_>>().join(", ")
            )
        })
}

pub(crate) fn names() -> impl Iterator<Item = &'static str> {
    SEEDS.iter().map(|seed| seed.id)
}

pub(crate) fn default_theme() -> Theme {
    resolve(DEFAULT_NAME).expect("default theme must exist in the catalog")
}

fn derive(seed: &Seed) -> Theme {
    let light = luminance(seed.background) > 150;
    let surface_weight = if light { 5 } else { 4 };
    let raised_weight = if light { 10 } else { 8 };
    let text = ensure_contrast(seed.text, seed.background, 7.0);
    let selection = ensure_contrast(seed.accent, seed.background, 3.0);
    let selected_background = blend_for_contrast(selection, seed.background, 3.0);
    let primary = ensure_contrast(seed.accent, seed.background, 4.5);
    let success = ensure_contrast(
        if light {
            (35, 134, 54)
        } else {
            (126, 186, 145)
        },
        seed.background,
        4.5,
    );
    let warning = ensure_contrast(
        if light {
            (154, 103, 0)
        } else {
            (209, 177, 111)
        },
        seed.background,
        4.5,
    );
    let error = ensure_contrast(
        if light {
            (207, 34, 46)
        } else {
            (204, 118, 118)
        },
        seed.background,
        4.5,
    );
    Theme {
        background: rgb(seed.background),
        surface: rgb(blend(text, seed.background, surface_weight)),
        surface_raised: rgb(blend(text, seed.background, raised_weight)),
        primary: rgb(primary),
        secondary: rgb(ensure_contrast(
            blend(primary, text, 58),
            seed.background,
            4.5,
        )),
        success: rgb(success),
        warning: rgb(warning),
        error: rgb(error),
        text: rgb(text),
        muted: rgb(blend_for_contrast(text, seed.background, 4.5)),
        dim: rgb(blend_for_contrast(text, seed.background, 3.0)),
        selected_text: rgb(ensure_contrast(text, selected_background, 4.5)),
        selected_background: rgb(selected_background),
    }
}

fn rgb((red, green, blue): (u8, u8, u8)) -> Color {
    Color::Rgb(red, green, blue)
}

fn blend(foreground: (u8, u8, u8), background: (u8, u8, u8), weight: u16) -> (u8, u8, u8) {
    let channel = |foreground: u8, background: u8| {
        ((u16::from(foreground) * weight + u16::from(background) * (100 - weight)) / 100) as u8
    };
    (
        channel(foreground.0, background.0),
        channel(foreground.1, background.1),
        channel(foreground.2, background.2),
    )
}

fn blend_for_contrast(
    foreground: (u8, u8, u8),
    background: (u8, u8, u8),
    minimum: f64,
) -> (u8, u8, u8) {
    (1..=100)
        .map(|weight| blend(foreground, background, weight))
        .find(|candidate| contrast(*candidate, background) >= minimum)
        .unwrap_or(foreground)
}

fn ensure_contrast(
    candidate: (u8, u8, u8),
    background: (u8, u8, u8),
    minimum: f64,
) -> (u8, u8, u8) {
    if contrast(candidate, background) >= minimum {
        return candidate;
    }
    let black = (0, 0, 0);
    let white = (255, 255, 255);
    let endpoint = if contrast(black, background) >= contrast(white, background) {
        black
    } else {
        white
    };
    (1..=100)
        .map(|weight| blend(endpoint, candidate, weight))
        .find(|adjusted| contrast(*adjusted, background) >= minimum)
        .unwrap_or(endpoint)
}

fn contrast(left: (u8, u8, u8), right: (u8, u8, u8)) -> f64 {
    let left = relative_luminance(left);
    let right = relative_luminance(right);
    (left.max(right) + 0.05) / (left.min(right) + 0.05)
}

fn relative_luminance((red, green, blue): (u8, u8, u8)) -> f64 {
    let channel = |value: u8| {
        let value = f64::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    channel(red) * 0.2126 + channel(green) * 0.7152 + channel(blue) * 0.0722
}

fn luminance((red, green, blue): (u8, u8, u8)) -> u16 {
    ((u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114) / 1000) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_entry_resolves() {
        for name in names() {
            let theme = resolve(name).unwrap();
            let color = |color| match color {
                Color::Rgb(red, green, blue) => (red, green, blue),
                _ => panic!("themes must use RGB colors"),
            };
            let background = color(theme.background);
            assert!(
                contrast(color(theme.text), background) >= 4.5,
                "{name} text"
            );
            for (role, role_color) in [
                ("primary", theme.primary),
                ("secondary", theme.secondary),
                ("success", theme.success),
                ("warning", theme.warning),
                ("error", theme.error),
            ] {
                assert!(
                    contrast(color(role_color), background) >= 4.5,
                    "{name} {role}"
                );
            }
            assert!(
                contrast(color(theme.muted), background) >= 4.5,
                "{name} muted"
            );
            assert!(
                contrast(color(theme.selected_background), background) >= 3.0,
                "{name} selection"
            );
            assert!(
                contrast(color(theme.selected_text), color(theme.selected_background)) >= 4.5,
                "{name} selected text"
            );
        }
        assert_eq!(
            default_theme().background,
            resolve(DEFAULT_NAME).unwrap().background
        );
        assert!(resolve("mdmanager").is_err());
    }
}

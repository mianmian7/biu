use gpui_kit::*;

#[derive(Debug, Clone, Copy)]
pub struct BiuPalette {
    pub dark: bool,
    pub bg: Hsla,
    pub sidebar_bg: Hsla,
    pub surface: Hsla,
    pub surface_hover: Hsla,
    pub surface_active: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_dim: Hsla,
    pub accent: Hsla,
    pub accent_fg: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub radius: Pixels,
}

impl BiuPalette {
    pub fn from_settings(theme_mode: &str, primary_hex: &str, border_radius: u32, system_dark: bool) -> Self {
        let dark = match theme_mode {
            "light" => false,
            "dark" => true,
            _ => system_dark,
        };
        let accent = parse_hex_hsla(primary_hex).unwrap_or_else(|| rgb(0x1ed760).into());
        let radius = px(border_radius.clamp(2, 16) as f32);

        if dark {
            Self {
                dark: true,
                bg: rgb(0x0f1115).into(),
                sidebar_bg: rgb(0x14171d).into(),
                surface: rgb(0x1b1f27).into(),
                surface_hover: rgb(0x232934).into(),
                surface_active: rgb(0x293140).into(),
                border: hsla(220.0 / 360.0, 0.14, 0.24, 0.55),
                text: rgb(0xf3f5f8).into(),
                text_muted: rgb(0x9ca5b4).into(),
                text_dim: rgb(0x6e7787).into(),
                accent,
                accent_fg: rgb(0x08130c).into(),
                danger: rgb(0xef4444).into(),
                warning: rgb(0xf59e0b).into(),
                radius,
            }
        } else {
            Self {
                dark: false,
                bg: rgb(0xf7f8fa).into(),
                sidebar_bg: rgb(0xeeeff3).into(),
                surface: rgb(0xffffff).into(),
                surface_hover: rgb(0xe9ecf2).into(),
                surface_active: rgb(0xdde3ee).into(),
                border: hsla(220.0 / 360.0, 0.14, 0.78, 0.8),
                text: rgb(0x14171f).into(),
                text_muted: rgb(0x525b6c).into(),
                text_dim: rgb(0x8891a2).into(),
                accent,
                accent_fg: rgb(0xffffff).into(),
                danger: rgb(0xdc2626).into(),
                warning: rgb(0xd97706).into(),
                radius,
            }
        }
    }
}

pub fn parse_hex_hsla(hex: &str) -> Option<Hsla> {
    let trimmed = hex.trim().trim_start_matches('#');
    if trimmed.len() == 6 {
        let val = u32::from_str_radix(trimmed, 16).ok()?;
        Some(rgb(val).into())
    } else if trimmed.len() == 8 {
        let val = u32::from_str_radix(trimmed, 16).ok()?;
        Some(rgba(val).into())
    } else {
        None
    }
}

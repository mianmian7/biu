use gpui_kit::*;

#[derive(Debug, Clone, Copy)]
pub struct BiuPalette {
    pub dark: bool,
    pub bg: Hsla,
    pub sidebar_bg: Hsla,
    pub bar_bg: Hsla,
    pub surface: Hsla,
    pub surface_elevated: Hsla,
    pub surface_hover: Hsla,
    pub surface_active: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_dim: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub accent_fg: Hsla,
    pub play_btn_bg: Hsla,
    pub play_btn_fg: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub radius: Pixels,
}

impl BiuPalette {
    pub fn from_settings(
        theme_mode: &str,
        primary_hex: &str,
        border_radius: u32,
        system_dark: bool,
    ) -> Self {
        let dark = match theme_mode {
            "light" => false,
            "dark" => true,
            _ => system_dark,
        };
        let accent: Hsla = parse_hex_hsla(primary_hex).unwrap_or_else(|| rgb(0x1ed760).into());
        let radius = px(border_radius.clamp(6, 16) as f32);

        if dark {
            Self {
                dark: true,
                bg: rgb(0x0b0c0e).into(),
                sidebar_bg: rgb(0x141518).into(),
                bar_bg: rgb(0x121316).into(),
                surface: rgb(0x17191d).into(),
                surface_elevated: rgb(0x1f2228).into(),
                surface_hover: rgb(0x252830).into(),
                surface_active: rgb(0x2c303a).into(),
                border: hsla(0.0, 0.0, 1.0, 0.07),
                border_strong: hsla(0.0, 0.0, 1.0, 0.13),
                text: rgb(0xf4f5f7).into(),
                text_muted: rgb(0x9b9fa8).into(),
                text_dim: rgb(0x696e79).into(),
                accent,
                accent_soft: accent.opacity(0.14),
                accent_fg: rgb(0x07120a).into(),
                play_btn_bg: rgb(0xffffff).into(),
                play_btn_fg: rgb(0x0b0c0e).into(),
                danger: rgb(0xf43f5e).into(),
                warning: rgb(0xf59e0b).into(),
                radius,
            }
        } else {
            Self {
                dark: false,
                bg: rgb(0xffffff).into(),
                sidebar_bg: rgb(0xf6f7f9).into(),
                bar_bg: rgb(0xfafbfc).into(),
                surface: rgb(0xf3f4f6).into(),
                surface_elevated: rgb(0xeaecef).into(),
                surface_hover: rgb(0xe4e7ec).into(),
                surface_active: rgb(0xd9dde5).into(),
                border: hsla(0.0, 0.0, 0.0, 0.07),
                border_strong: hsla(0.0, 0.0, 0.0, 0.13),
                text: rgb(0x111318).into(),
                text_muted: rgb(0x565d6d).into(),
                text_dim: rgb(0x8a91a0).into(),
                accent,
                accent_soft: accent.opacity(0.14),
                accent_fg: rgb(0xffffff).into(),
                play_btn_bg: rgb(0x141518).into(),
                play_btn_fg: rgb(0xffffff).into(),
                danger: rgb(0xe11d48).into(),
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

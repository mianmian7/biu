use crate::theme::BiuPalette;
use gpui_kit::*;
use std::sync::Arc;

pub fn format_duration(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "00:00".to_string();
    }
    let total = secs.round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub fn cover_box(
    image: Option<Arc<RenderImage>>,
    w: Pixels,
    h: Pixels,
    radius: Pixels,
    palette: BiuPalette,
) -> Div {
    let base = div()
        .w(w)
        .h(h)
        .flex_shrink_0()
        .rounded(radius)
        .overflow_hidden()
        .bg(palette.surface_active)
        .border_1()
        .border_color(palette.border);

    if let Some(img_arc) = image {
        base.child(
            img(img_arc)
                .w(w)
                .h(h)
                .object_fit(ObjectFit::Cover),
        )
    } else {
        base.flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(palette.text_dim)
            .child("♪")
    }
}

pub fn empty_state(title: impl Into<SharedString>, subtitle: impl Into<SharedString>, palette: BiuPalette) -> Div {
    div()
        .w_full()
        .py_12()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text)
                .child(title.into()),
        )
        .child(
            div()
                .text_sm()
                .text_color(palette.text_muted)
                .child(subtitle.into()),
        )
}

pub fn section_header(title: impl Into<SharedString>, palette: BiuPalette) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .mb_3()
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text)
                .child(title.into()),
        )
}

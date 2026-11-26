use crate::theme::BiuPalette;
use gpui_kit::assets::IconName;
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
        .bg(palette.surface_elevated);

    if let Some(img_arc) = image {
        base.child(img(img_arc).w(w).h(h).object_fit(ObjectFit::Cover))
    } else {
        base.flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(IconName::Music.path())
                    .size(px(18.0))
                    .text_color(palette.text_dim.opacity(0.6)),
            )
    }
}

pub fn fluid_cover(
    image: Option<Arc<RenderImage>>,
    h: Pixels,
    radius: Pixels,
    palette: BiuPalette,
) -> Div {
    let base = div()
        .w_full()
        .h(h)
        .rounded(radius)
        .overflow_hidden()
        .bg(palette.surface_elevated);

    if let Some(img_arc) = image {
        base.child(img(img_arc).size_full().object_fit(ObjectFit::Cover))
    } else {
        base.flex().items_center().justify_center().child(
            svg()
                .path(IconName::Music.path())
                .size(px(26.0))
                .text_color(palette.text_dim.opacity(0.45)),
        )
    }
}

pub fn empty_state(
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    palette: BiuPalette,
) -> Div {
    let subtitle: SharedString = subtitle.into();
    let has_sub = !subtitle.is_empty();
    let base = div()
        .w_full()
        .py_16()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .child(
            div()
                .size(px(48.0))
                .rounded_full()
                .bg(palette.surface)
                .flex()
                .items_center()
                .justify_center()
                .mb_1()
                .child(
                    svg()
                        .path(IconName::Disc3.path())
                        .size(px(22.0))
                        .text_color(palette.text_dim),
                ),
        )
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text)
                .child(title.into()),
        );
    if has_sub {
        base.child(
            div()
                .text_sm()
                .text_color(palette.text_muted)
                .child(subtitle),
        )
    } else {
        base
    }
}

pub fn section_header(title: impl Into<SharedString>, palette: BiuPalette) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .mb_3()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(3.5))
                        .h(px(16.0))
                        .rounded_full()
                        .bg(palette.accent),
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::BOLD)
                        .text_color(palette.text)
                        .child(title.into()),
                ),
        )
}

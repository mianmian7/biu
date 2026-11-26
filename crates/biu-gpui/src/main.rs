mod image_loader;
mod modals;
mod pages;
mod shell;
mod state;
mod theme;
mod widgets;

use crate::shell::MainShell;
use crate::state::{AppState, Services};
use gpui_kit::*;
use std::sync::Arc;

fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let services = Arc::new(Services::new().expect("failed to initialize Biu services"));

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let state = cx.new(|cx| AppState::new(services.clone(), cx));
        state.update(cx, |s, cx| s.refresh_user_and_folders(cx));

        let bounds = Bounds::centered(None, size(px(1160.0), px(760.0)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(880.0), px(600.0))),
            titlebar: Some(TitlebarOptions {
                title: Some("Biu".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(12.0), px(12.0))),
            }),
            ..Default::default()
        };

        let _ = gpui_kit::open_window(opts, cx, |window, cx| {
            let state_clone = state.clone();
            cx.new(move |cx| MainShell::new(state_clone, window, cx))
        });
    });
}

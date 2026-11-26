mod desktop_lyrics;
mod full_screen;
mod image_loader;
mod mini_player;
mod modals;
mod pages;
mod player_bar;
mod shell;
mod state;
mod system;
mod theme;
mod widgets;

use crate::state::{AppState, Services};
use crate::system::GlobalAppState;
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

    let app = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);

    // macOS dock click while the main window is closed to the tray.
    app.on_reopen(|cx| {
        if let Some(state) = system::global_state(cx) {
            system::open_or_focus_main_window(state, cx);
        }
    });

    app.run(move |cx| {
        gpui_kit::init(cx);
        // Closing the main window hides to tray; only an explicit quit ends the app.
        cx.set_quit_mode(QuitMode::Explicit);

        let state = cx.new(|cx| AppState::new(services.clone(), cx));
        cx.set_global(GlobalAppState(state.clone()));
        state.update(cx, |s, cx| s.refresh_user_and_folders(cx));

        system::observe_window_closures(state.clone(), cx);
        system::init_tray_and_hotkeys(state.clone(), cx);
        system::open_or_focus_main_window(state.clone(), cx);

        if state.read(cx).settings.show_desktop_lyrics {
            desktop_lyrics::open_or_toggle_desktop_lyrics(state.clone(), cx);
        }
    });
}

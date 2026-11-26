use crate::state::AppState;
use gpui_kit::*;

/// Owns all active modals (QR Login, Lyrics Search, Fav Select, Fav Folder Edit, Playlist Drawer).
pub struct ModalsHost {
    pub state: Entity<AppState>,
}

impl ModalsHost {
    pub fn new(state: Entity<AppState>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { state }
    }
}

impl Render for ModalsHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

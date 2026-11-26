use crate::state::AppState;
use gpui_kit::*;

/// Owns all content pages, their async fetched data, scroll handles, and text inputs.
pub struct PagesHost {
    pub state: Entity<AppState>,
}

impl PagesHost {
    pub fn new(state: Entity<AppState>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { state }
    }
}

impl Render for PagesHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

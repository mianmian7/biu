use crate::state::AppState;
use gpui_kit::*;

pub struct MainShell {
    pub state: Entity<AppState>,
}

impl MainShell {
    pub fn new(state: Entity<AppState>) -> Self {
        Self { state }
    }
}

impl Render for MainShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.state.read(cx).palette(window);
        div()
            .size_full()
            .bg(palette.bg)
            .text_color(palette.text)
            .flex()
            .items_center()
            .justify_center()
            .child("Biu GPUI Shell")
    }
}

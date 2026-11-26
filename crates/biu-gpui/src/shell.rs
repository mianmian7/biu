use crate::modals::ModalsHost;
use crate::pages::PagesHost;
use crate::state::AppState;
use gpui_kit::*;

pub struct MainShell {
    pub state: Entity<AppState>,
    pub pages: Entity<PagesHost>,
    pub modals: Entity<ModalsHost>,
}

impl MainShell {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pages = cx.new(|cx| PagesHost::new(state.clone(), window, cx));
        let modals = cx.new(|cx| ModalsHost::new(state.clone(), window, cx));
        Self {
            state,
            pages,
            modals,
        }
    }
}

impl Render for MainShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.state.read(cx).palette(window);
        div()
            .size_full()
            .bg(palette.bg)
            .text_color(palette.text)
            .child(self.pages.clone())
            .child(self.modals.clone())
    }
}

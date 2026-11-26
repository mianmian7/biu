use crate::state::AppState;
use gpui_kit::*;

pub fn render_page(_state: &Entity<AppState>, _window: &mut Window, _cx: &mut App) -> AnyElement {
    div().into_any_element()
}

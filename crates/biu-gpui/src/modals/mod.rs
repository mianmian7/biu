use crate::state::AppState;
use gpui_kit::*;

pub fn render_active_modal(_state: &Entity<AppState>, _window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
    None
}

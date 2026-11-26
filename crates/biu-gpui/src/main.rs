use gpui_kit::{component::label::Label, *};

struct BiuBootstrap;

impl Render for BiuBootstrap {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(Label::new("Biu GPUI"))
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        let _ = gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|_| BiuBootstrap)
        });
    });
}

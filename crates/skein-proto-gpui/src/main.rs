// Research stub — see docs/pure-rust-prototype-plan.md (Phase 5b).
//
// This crate is excluded from the workspace because building it
// requires full Xcode + an accepted license, where Floem and Iced
// build with only Command Line Tools. The hello-world below was
// never observed running; the build halted on Xcode's license
// agreement check. Left in place as a research artifact alongside
// the Floem and Iced variants.

use gpui::{
    App, Application, Bounds, Context, Render, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};

struct Hello;

impl Render for Hello {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(rgb(0x1a1a20))
            .text_color(rgb(0xe6e6e6))
            .size_full()
            .child(div().text_xl().child("Hello, Skein"))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x999999))
                    .child("Pure-Rust prototype — GPUI variant — issue #36"),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(720.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Hello),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}

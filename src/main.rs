mod app;
mod config;
mod library;
mod transfer;

use gpui_kit::*;

actions!(piclecor, [Quit]);

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);

        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        app::bind_keys(cx);
        cx.set_menus(vec![Menu {
            name: "Piclecor".into(),
            items: vec![MenuItem::action("Quit Piclecor", Quit)],
            disabled: false,
        }]);
        cx.on_window_closed(|cx, _| cx.quit()).detach();

        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Piclecor".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| app::Piclecor::new(window, cx)))
            .expect("open window");
        cx.activate(true);
    });
}

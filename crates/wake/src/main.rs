mod assets;
mod format;
mod theme;
mod ui;
mod workbench;

use assets::Assets;
use gpui::*;
use gpui_component::Root;
use workbench::{
    PaletteDown, PaletteUp, RefreshSessions, ToggleSearch, Workbench, KEY_CONTEXT, PALETTE_CONTEXT,
};

actions!(wake_app, [Quit, CloseWindow]);

fn main() {
    let app = Application::new().with_assets(Assets);
    app.run(move |cx: &mut App| {
        gpui_component::init(cx);
        gpui_component::set_locale("en");
        theme::sync_appearance(None, cx);

        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &CloseWindow, cx| {
            if let Some(w) = cx.active_window() {
                w.update(cx, |_, window, _| window.remove_window()).ok();
            }
        });
        // secondary = macOS 的 cmd、其他平台的 ctrl(gpui keystroke 内建别名)
        cx.bind_keys([
            KeyBinding::new(ui::SEARCH_KEYSTROKE, ToggleSearch, Some(KEY_CONTEXT)),
            KeyBinding::new("secondary-r", RefreshSessions, Some(KEY_CONTEXT)),
            KeyBinding::new("secondary-q", Quit, None),
            KeyBinding::new("secondary-w", CloseWindow, None),
            // ⌘K 面板:焦点在搜索输入框,↑↓ 冒泡到面板容器挪选中
            KeyBinding::new("up", PaletteUp, Some(PALETTE_CONTEXT)),
            KeyBinding::new("down", PaletteDown, Some(PALETTE_CONTEXT)),
        ]);
        cx.set_menus(vec![
            Menu {
                name: "Wake".into(),
                items: vec![MenuItem::action("Quit Wake", Quit)],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("Refresh Sessions", RefreshSessions),
                    MenuItem::separator(),
                    MenuItem::action("Close Window", CloseWindow),
                ],
            },
        ]);

        let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
        // macOS:隐藏系统标题栏、内容顶到窗顶,traffic light 悬浮在侧栏上;
        // Linux:标准服务端装饰(appears_transparent 仅 macOS/Windows 生效;
        // GNOME Wayland 不给 SSD 时暂无标题栏,已知限制,实机验收后再定 CSD)。
        // cfg! 而非 #[cfg]:两支在任一平台都参与类型检查,别让另一支只有 CI 见得到
        let titlebar = if cfg!(target_os = "macos") {
            TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: Some(point(px(20.), px(11.))),
            }
        } else {
            TitlebarOptions {
                title: Some("Wake".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }
        };
        cx.open_window(
            WindowOptions {
                titlebar: Some(titlebar),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(940.), px(620.))),
                // Linux 桌面按它归组窗口、匹配 .desktop(StartupWMClass=wake)
                app_id: Some("wake".into()),
                // Wayland 显式请求 CSD(2026-08-24 Codex review):默认的 Server
                // 请求在 GNOME/Mutter(无 zxdg-decoration 协议)下会被 gpui 记成
                // Server 而 compositor 实际什么都不画——窗口既无系统标题栏、
                // workbench 又按 Server 不挂 TitleBar,彻底没有关窗/拖拽面。
                // 请求 Client 后:Wayland 全家走 CSD(TitleBar 补位),X11 侧
                // gpui 探测 compositor 不支持 CSD 时仍自动回报 Server(WM 标题
                // 栏照常、TitleBar 不挂),macOS 忽略此字段
                window_decorations: Some(WindowDecorations::Client),
                ..Default::default()
            },
            |window, cx| {
                // 跟随系统深浅色切换
                window
                    .observe_window_appearance(|window, cx| {
                        theme::sync_appearance(Some(window), cx);
                    })
                    .detach();
                theme::sync_appearance(Some(window), cx);

                let workbench = cx.new(|cx| Workbench::new(window, cx));
                window.focus(&workbench.read(cx).focus_handle(cx));
                cx.new(|cx| Root::new(workbench, window, cx))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}

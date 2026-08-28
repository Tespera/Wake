//! macOS 专属的窗口调校。
//!
//! gpui 把窗口设成 `titlebarAppearsTransparent = YES`,但没有动
//! `titlebarSeparatorStyle`。该属性默认 `Automatic`,系统会在标题栏与内容
//! 之间自行画分隔线;深色下是一条亮线。gpui 未暴露此开关,走 objc 运行时关。

/// 关掉所有窗口的标题栏分隔线。每次开窗后调用。
#[cfg(target_os = "macos")]
pub fn suppress_titlebar_separator() {
    use objc::runtime::{Object, BOOL, YES};
    use objc::{class, msg_send, sel, sel_impl};

    /// `NSTitlebarSeparatorStyleNone`
    const SEPARATOR_NONE: i64 = 1;

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let windows: *mut Object = msg_send![app, windows];
        if windows.is_null() {
            return;
        }
        let count: usize = msg_send![windows, count];
        for ix in 0..count {
            let window: *mut Object = msg_send![windows, objectAtIndex: ix];
            if window.is_null() {
                continue;
            }
            // macOS 11+ 的 API
            let responds: BOOL =
                msg_send![window, respondsToSelector: sel!(setTitlebarSeparatorStyle:)];
            if responds == YES {
                let _: () = msg_send![window, setTitlebarSeparatorStyle: SEPARATOR_NONE];
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn suppress_titlebar_separator() {}

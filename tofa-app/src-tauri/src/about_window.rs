//! Singleton "关于 TOFA" window. Shown from the tray menu's "关于 TOFA"
//! item. Hosts the `about.html` page from the bundled frontend.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const WINDOW_LABEL: &str = "about";

pub fn show_or_focus(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.show();
        let _ = win.set_focus();
        let _ = win.unminimize();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::App("about.html".into()))
        .title("关于 TOFA")
        .inner_size(320.0, 400.0)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .always_on_top(false)
        .visible(true)
        .build();
}

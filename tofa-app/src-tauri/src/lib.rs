mod about_window;
mod commands;
mod login_item;
#[cfg(target_os = "macos")]
mod macos_tray;
mod state;

use state::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
#[cfg(not(target_os = "macos"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{
    menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Listener, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Wry,
};

/// If true, the popover does NOT auto-hide on focus loss. JS toggles this
/// around operations that legitimately steal focus (file pickers, camera
/// scan, screen capture) so the user doesn't see the popover vanish
/// mid-action.
static POPOVER_PINNED: AtomicBool = AtomicBool::new(false);

/// Toggle whether the popover stays open during focus-stealing operations.
/// Called from JS via `invoke('set_popover_pinned', { pinned: true|false })`.
#[tauri::command]
fn set_popover_pinned(pinned: bool) {
    POPOVER_PINNED.store(pinned, Ordering::Relaxed);
}

/// Position the popover flush below the tray icon on whichever screen was
/// clicked. Uses NSScreen and NSEvent directly to avoid Tauri's broken
/// multi-display coordinate conversions.
///
/// Strategy:
/// - `NSEvent::mouseLocation()` gives the cursor in Cocoa points (y-up from
///   bottom-left of the primary screen), which is the same coordinate system
///   as NSScreen.frame, so no conversion is needed.
/// - `NSScreen.visibleFrame.maxY` is exactly the bottom edge of the menu bar
///   on each screen — that is where the popover top belongs.
/// - We bypass Tauri's `set_position` (which silently no-ops on secondary
///   screens) and call `setFrameTopLeftPoint:` on the underlying NSWindow.
#[cfg(target_os = "macos")]
fn position_popover_under_tray(window: &WebviewWindow) {
    let _ = window.with_webview(|wv| unsafe {
        use objc2::rc::Retained;
        use objc2_app_kit::{NSEvent, NSScreen, NSWindow};
        use objc2_foundation::NSPoint;

        let ns_window = wv.ns_window() as *mut NSWindow;
        let win = match Retained::retain(ns_window) {
            Some(w) => w,
            None => return,
        };

        // Cursor in Cocoa screen coords — same system as NSScreen.frame.
        let loc = NSEvent::mouseLocation();

        // MainThreadMarker is safe here: with_webview runs on the main thread.
        use objc2::MainThreadMarker;
        let mtm = MainThreadMarker::new_unchecked();

        const POPOVER_W: f64 = 320.0;

        // Compute (target_x, target_y) from the screen that contains the cursor.
        // visibleFrame excludes the menu bar at the top, so its maxY is the
        // Cocoa y of the menu bar's bottom edge — exactly where the popover top sits.
        let compute_pos = |s: &NSScreen| -> (f64, f64) {
            let visible = s.visibleFrame();
            let y = visible.origin.y + visible.size.height;
            let frame = s.frame();
            let x = (loc.x - POPOVER_W / 2.0)
                .max(frame.origin.x)
                .min(frame.origin.x + frame.size.width - POPOVER_W);
            (x, y)
        };

        // Find the screen whose frame contains the cursor.
        let screens = NSScreen::screens(mtm);
        let mut pos: Option<(f64, f64)> = None;
        for i in 0..screens.count() {
            let s = screens.objectAtIndex(i);
            let f = s.frame();
            if loc.x >= f.origin.x
                && loc.x < f.origin.x + f.size.width
                && loc.y >= f.origin.y
                && loc.y < f.origin.y + f.size.height
            {
                pos = Some(compute_pos(&s));
                break;
            }
        }

        // Fall back to the screen that owns the menu bar.
        let (target_x, target_y) = pos
            .or_else(|| NSScreen::mainScreen(mtm).as_deref().map(compute_pos))
            .unwrap_or((0.0, 0.0));

        win.setFrameTopLeftPoint(NSPoint::new(target_x, target_y));
    });
}

#[cfg(not(target_os = "macos"))]
fn position_popover_under_tray(_window: &WebviewWindow) {}

fn show_popover_under_tray(win: &WebviewWindow) {
    position_popover_under_tray(win);
    let _ = win.show();
    let _ = win.set_focus();
    // Re-position once visible — some macOS state transitions can shift
    // the frame on show.
    position_popover_under_tray(win);
}

fn show_mfa_from_tray(app: &tauri::AppHandle) {
    if POPOVER_PINNED.load(Ordering::Relaxed) {
        return;
    }
    if let Some(win) = app.get_webview_window("popover") {
        show_popover_under_tray(&win);
        let _ = win.emit("tray-action", "mfa");
    }
}

fn build_tray_menu(
    app: &tauri::AppHandle,
    item_about: &MenuItem<Wry>,
    item_scan_screen: &MenuItem<Wry>,
    item_scan_camera: &MenuItem<Wry>,
    item_quit: &MenuItem<Wry>,
) -> tauri::Result<Menu<Wry>> {
    let sep1 = PredefinedMenuItem::separator(app)?;
    let settings_item = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;

    let items: &[&dyn IsMenuItem<Wry>] = &[
        item_about,
        &sep1,
        item_scan_screen,
        item_scan_camera,
        &settings_item,
        &sep2,
        item_quit,
    ];
    Menu::with_items(app, items)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(AppState::new()))
        .invoke_handler(tauri::generate_handler![
            set_popover_pinned,
            commands::get_versions,
            commands::vault_exists,
            commands::create_vault,
            commands::unlock,
            commands::get_entries,
            commands::copy_code,
            commands::copy_uri,
            commands::get_masked_uri,
            commands::get_full_uri,
            commands::get_settings,
            commands::save_settings,
            login_item::get_login_item_status,
            login_item::set_login_item_enabled,
            commands::get_secret,
            commands::scan_screen,
            commands::scan_camera,
            commands::scan_image_bytes,
            commands::add_from_uri,
            commands::delete_entry,
            commands::pick_vault_folder,
            commands::pick_and_import_file,
            commands::import_file,
            commands::generate_entry_qr,
            commands::generate_selection_qr,
            commands::generate_otpauth_list,
            commands::save_qr_png,
            commands::save_qr_zip,
            commands::save_uri_list,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            let _ = app
                .handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory);

            let _window =
                WebviewWindowBuilder::new(app, "popover", WebviewUrl::App("index.html".into()))
                    .decorations(false)
                    .always_on_top(true)
                    .resizable(false)
                    .visible(false)
                    .inner_size(320.0, 480.0)
                    .build()?;

            // Make the window follow the active Space instead of switching to the Space it lives in
            #[cfg(target_os = "macos")]
            {
                use objc2::rc::Retained;
                use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
                let _ = _window.with_webview(|wv| unsafe {
                    let ns_window = wv.ns_window() as *mut NSWindow;
                    if let Some(win) = Retained::retain(ns_window) {
                        win.setCollectionBehavior(
                            NSWindowCollectionBehavior::MoveToActiveSpace
                                | NSWindowCollectionBehavior::Transient,
                        );
                    }
                });
            }

            let item_scan_screen =
                MenuItem::with_id(app, "scan-screen", "扫描屏幕", false, None::<&str>)?;
            let item_scan_camera =
                MenuItem::with_id(app, "scan-camera", "摄像头扫描", false, None::<&str>)?;
            let item_quit = MenuItem::with_id(app, "quit", "退出 TOFA", true, Some("CmdOrCtrl+Q"))?;

            let item_about = MenuItem::with_id(app, "about", "关于 TOFA", true, None::<&str>)?;
            let menu = build_tray_menu(
                app.handle(),
                &item_about,
                &item_scan_screen,
                &item_scan_camera,
                &item_quit,
            )?;

            static ICON_LOCKED: &[u8] = include_bytes!("../icons/tray_icon_locked.png");
            static ICON_OPEN: &[u8] = include_bytes!("../icons/tray_icon_open.png");

            let tray_icon = tauri::image::Image::from_bytes(ICON_LOCKED)
                .unwrap_or_else(|_| app.default_window_icon().unwrap().clone());

            let tray = TrayIconBuilder::new()
                .icon(tray_icon)
                .menu(&menu)
                .tooltip(if cfg!(debug_assertions) {
                    "TOFA DEV"
                } else {
                    "TOFA"
                })
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    if POPOVER_PINNED.load(Ordering::Relaxed) {
                        return;
                    }
                    let action = match event.id.as_ref() {
                        "about" => {
                            crate::about_window::show_or_focus(app);
                            return;
                        }
                        "scan-screen" => "scan-screen",
                        "scan-camera" => "scan-camera",
                        "settings" => "settings",
                        "quit" => {
                            app.exit(0);
                            return;
                        }
                        _ => return,
                    };
                    if let Some(win) = app.get_webview_window("popover") {
                        show_popover_under_tray(&win);
                        let _ = win.emit("tray-action", action);
                    }
                })
                .build(app)?;

            let tray_id = tray.id().clone();
            #[cfg(target_os = "macos")]
            macos_tray::install(&tray)?;

            // 扫描入口随保险库会话状态同步。
            let ss = item_scan_screen.clone();
            let sc = item_scan_camera.clone();
            let app_unlock = app.handle().clone();
            let tray_id_unlock = tray_id.clone();
            app.listen("session-unlocked", move |_| {
                let _ = ss.set_enabled(true);
                let _ = sc.set_enabled(true);
                if let Some(tray) = app_unlock.tray_by_id(&tray_id_unlock) {
                    if let Ok(icon) = tauri::image::Image::from_bytes(ICON_OPEN) {
                        let _ = tray.set_icon(Some(icon));
                    }
                }
            });

            let ss2 = item_scan_screen.clone();
            let sc2 = item_scan_camera.clone();
            let app_lock = app.handle().clone();
            let tray_id_lock = tray_id.clone();
            app.listen("session-locked", move |_| {
                let _ = ss2.set_enabled(false);
                let _ = sc2.set_enabled(false);
                if let Some(tray) = app_lock.tray_by_id(&tray_id_lock) {
                    if let Ok(icon) = tauri::image::Image::from_bytes(ICON_LOCKED) {
                        let _ = tray.set_icon(Some(icon));
                    }
                }
            });

            #[cfg(not(target_os = "macos"))]
            tray.on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    show_mfa_from_tray(tray.app_handle());
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(false) = event {
                if window.label() == "popover" && !POPOVER_PINNED.load(Ordering::Relaxed) {
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

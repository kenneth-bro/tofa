use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSEventMask, NSEventModifierFlags, NSEventType, NSMenu, NSStatusBarButton,
};
use objc2_foundation::{NSObject, NSPoint};
use std::cell::RefCell;
use tauri::{tray::TrayIcon, AppHandle, Wry};

struct Context {
    app: AppHandle,
    menu: Retained<NSMenu>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "TofaStatusButtonTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Context]
    struct Target;

    impl Target {
        #[unsafe(method(tofaClick:))]
        fn on_click(&self, sender: &NSStatusBarButton) {
            if super::POPOVER_PINNED.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let ctx = self.ivars();
            let app = NSApplication::sharedApplication(MainThreadMarker::from(self));
            let right = app.currentEvent().is_some_and(|event| {
                event.r#type() == NSEventType::RightMouseUp
                    || (event.r#type() == NSEventType::LeftMouseUp
                        && event.modifierFlags().contains(NSEventModifierFlags::Control))
            });
            if right {
                ctx.menu.popUpMenuPositioningItem_atLocation_inView(
                    None,
                    NSPoint::new(0.0, sender.bounds().origin.y),
                    Some(sender),
                );
            } else {
                super::show_mfa_from_tray(&ctx.app);
            }
        }
    }
);

thread_local! {
    // NSControl 不持有 target；托盘按钮在主线程使用它。
    static TARGET: RefCell<Option<Retained<Target>>> = const { RefCell::new(None) };
}

pub fn install(tray: &TrayIcon<Wry>) -> Result<(), Box<dyn std::error::Error>> {
    let app = tray.app_handle().clone();
    tray.with_inner_tray_icon(move |inner| {
        let mtm = MainThreadMarker::new().ok_or("缺少 macOS 主线程")?;
        let item = inner.ns_status_item().ok_or("未找到 TOFA 状态栏按钮")?;
        let button = item.button(mtm).ok_or("未找到 TOFA 状态栏按钮")?;
        let menu = item.menu(mtm).ok_or("未找到 TOFA 右键菜单")?;
        // 永久挂载 NSMenu 会让系统按钮截获左键；托盘组件的覆盖层也要停用。
        let overlay = button
            .subviews()
            .iter()
            .find(|view| view.class().name().to_bytes() == b"TaoTrayTarget")
            .ok_or("未找到托盘组件的鼠标覆盖层")?;
        overlay.setHidden(true);
        item.setMenu(None);
        let target: Retained<Target> = unsafe {
            msg_send![
                super(Target::alloc(mtm).set_ivars(Context { app, menu })),
                init
            ]
        };
        unsafe {
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(tofaClick:)));
            button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
        }
        TARGET.with(|slot| slot.replace(Some(target)));
        Ok::<_, &'static str>(())
    })?
    .map_err(Into::into)
}

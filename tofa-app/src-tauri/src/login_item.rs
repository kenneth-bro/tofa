#[derive(serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoginItemStatus {
    Disabled,
    Enabled,
    ApprovalRequired,
    Unavailable,
}

#[cfg(target_os = "macos")]
fn service() -> objc2::rc::Retained<objc2_service_management::SMAppService> {
    // 桌面包的最低系统版本为 macOS 13，保证 SMAppService 可用。
    unsafe { objc2_service_management::SMAppService::mainAppService() }
}

#[cfg(target_os = "macos")]
fn status(service: &objc2_service_management::SMAppService) -> LoginItemStatus {
    use objc2_service_management::SMAppServiceStatus as Native;
    match unsafe { service.status() } {
        Native::NotRegistered | Native::NotFound => LoginItemStatus::Disabled,
        Native::Enabled => LoginItemStatus::Enabled,
        Native::RequiresApproval => LoginItemStatus::ApprovalRequired,
        _ => LoginItemStatus::Unavailable,
    }
}

#[tauri::command]
pub fn get_login_item_status() -> LoginItemStatus {
    #[cfg(target_os = "macos")]
    return status(&service());
    #[cfg(not(target_os = "macos"))]
    LoginItemStatus::Unavailable
}

#[tauri::command]
pub fn set_login_item_enabled(enabled: bool) -> Result<LoginItemStatus, String> {
    #[cfg(target_os = "macos")]
    {
        let service = service();
        let current = status(&service);
        if current == LoginItemStatus::Unavailable {
            return Err("无法访问 macOS 登录项。".into());
        }
        if enabled && current == LoginItemStatus::Disabled {
            let result = unsafe { service.registerAndReturnError() };
            // 系统可能成功登记但要求用户到系统设置批准。
            if result.is_err() && status(&service) != LoginItemStatus::ApprovalRequired {
                result.map_err(|err| err.localizedDescription().to_string())?;
            }
        } else if !enabled && current != LoginItemStatus::Disabled {
            unsafe { service.unregisterAndReturnError() }
                .map_err(|err| err.localizedDescription().to_string())?;
        }
        Ok(status(&service))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = enabled;
        Err("此平台不支持 macOS 登录项。".into())
    }
}

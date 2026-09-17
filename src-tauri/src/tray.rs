//! 系统托盘：菜单（显示主窗口 / 立即签到全部 / 今日状态 / 退出）+ 红点两态图标

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Emitter, Manager,
};
use trae_signin_core::settings::Settings;

pub const MENU_SHOW: &str = "m-show";
pub const MENU_SIGNIN_ALL: &str = "m-signin-all";
pub const MENU_STATUS: &str = "m-status";
pub const MENU_QUIT: &str = "m-quit";

/// 两态托盘图标（编译期嵌入，避免打包后相对路径失效）
const TRAY_NORMAL_PNG: &[u8] = include_bytes!("../icons/tray-normal.png");
const TRAY_RED_PNG: &[u8] = include_bytes!("../icons/tray-red.png");

fn tray_image(red_dot: bool) -> Option<tauri::image::Image<'static>> {
    let bytes: &[u8] = if red_dot { TRAY_RED_PNG } else { TRAY_NORMAL_PNG };
    tauri::image::Image::from_bytes(bytes).ok().map(|i| i.to_owned())
}

pub struct TrayHandles {
    pub tray: TrayIcon,
    pub status_item: MenuItem<tauri::Wry>,
}

/// 构建托盘（setup 阶段调用）
pub fn build_tray(app: &AppHandle) -> Result<TrayHandles, tauri::Error> {
    let show = MenuItem::with_id(app, MENU_SHOW, "显示主窗口", true, None::<&str>)?;
    let signin_all = MenuItem::with_id(app, MENU_SIGNIN_ALL, "立即签到全部", true, None::<&str>)?;
    let status = MenuItem::with_id(app, MENU_STATUS, "今日状态：未添加账号", false, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &signin_all, &sep, &status, &sep, &quit])?;

    let tray = TrayIconBuilder::with_id("main-tray")
        .icon(tray_image(false).or_else(|| app.default_window_icon().cloned())
            .expect("tray icon missing"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("TRAE 签到")
        .on_menu_event(|app, event| {
            match event.id().as_ref() {
                MENU_SHOW => show_main_window(app),
                MENU_SIGNIN_ALL => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = crate::commands::run_signin_round(&app, None, false).await {
                            log::error!("托盘签到失败: {e}");
                        }
                    });
                }
                MENU_QUIT => {
                    app.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, button_state: tauri::tray::MouseButtonState::Up, .. } = event {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(TrayHandles { tray, status_item: status })
}

/// 显示/聚焦主窗口
pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 账号状态变化后调用：更新托盘文案与图标（红点）
pub fn update_tray(app: &AppHandle) {
    let state = crate::state::state(app);
    let (signed, total) = state.today_summary();
    let status_text = if total == 0 {
        "今日状态：未添加账号".to_string()
    } else {
        format!("今日状态：已签 {signed}/{total}")
    };
    let red_dot = total > 0 && signed < total;
    if let Some(w) = app.try_state::<TrayHandles>() {
        let _ = w.status_item.set_text(status_text);
        if let Some(img) = tray_image(red_dot) {
            let _ = w.tray.set_icon(Some(img));
        }
        let tooltip = if red_dot { "TRAE 签到（有未签账号）" } else { "TRAE 签到" };
        let _ = w.tray.set_tooltip(Some(tooltip));
    }
    // 同步前端
    let _ = app.emit(
        "tray://status",
        serde_json::json!({ "signed": signed, "total": total, "red_dot": red_dot }),
    );
}

/// 关闭窗口行为（CloseRequested 时调用）：返回 true = 已拦截隐藏到托盘
pub fn handle_close(app: &AppHandle) -> bool {
    let settings: Settings = crate::state::state(app).current_settings();
    if settings.close_to_tray {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.hide();
        }
        true
    } else {
        false
    }
}

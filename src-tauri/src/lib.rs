mod commands;
mod logger;
mod login_service;
mod notify;
mod scheduler_task;
mod state;
mod tray;

use state::{launched_hidden, AppState};
use tauri::{Manager, WindowEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let hidden = launched_hidden();
    tauri::Builder::default()
        // 单实例必须第一个注册（官方约定）：二次启动转发参数后立即退出
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // 二次启动：唤起已有窗口；但带 --hidden 的自启二次触发不弹窗
            if !args.iter().any(|a| a == "--hidden" || a == "-hidden") {
                tray::show_main_window(app);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                Some(vec!["--hidden"]),
            ),
        )
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::choose_data_dir,
            commands::init_data_dir,
            commands::change_data_dir,
            commands::open_data_dir,
            commands::open_external,
            commands::get_settings,
            commands::set_settings,
            commands::check_update,
            commands::app_version,
            commands::list_accounts,
            commands::import_credential,
            commands::delete_account,
            commands::start_login,
            commands::cancel_login,
            commands::signin_all,
            commands::signin_one,
            commands::refresh_all,
            commands::refresh_account,
            commands::get_history,
            commands::test_bark,
        ])
        .setup(move |app| {
            // 托盘
            let handles = tray::build_tray(app.handle())?;
            app.manage(handles);

            // 日志后端最先装：此后所有 log:: 调用才真的落盘（无后端时全静默丢弃）
            // 恢复今日缓存
            {
                let st = app.state::<AppState>();
                logger::init(st.current_data_dir().or_else(state::exe_dir));
                state::restore_today_from_history(st.inner());
            }
            tray::update_tray(app.handle());

            // 自启静默：仅当带 --hidden 参数时不显示窗口（窗口初始 visible:false）
            if !hidden {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }

            // 调度
            scheduler_task::spawn_scheduler(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                if tray::handle_close(app) {
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

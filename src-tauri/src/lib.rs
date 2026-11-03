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
    // panic 钩子最先装：panic=abort 下任何 panic 都会带走整个进程，
    // 唯一取证通道就是这里（见 logger.rs 注释）
    logger::install_panic_hook();

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
            commands::set_device_id,
            commands::detect_client_device_ids,
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
            match event {
                WindowEvent::CloseRequested { api, .. } => {
                    let app = window.app_handle();
                    if tray::handle_close(app) {
                        api.prevent_close();
                    } else {
                        // close_to_tray=false：关窗即退出。显式走 app.exit（RequestExit 路径，
                        // 会在 run 回调里 mark_exiting）而不是放任窗口销毁——「窗口销毁」这条
                        // 路径已被 run 回调的 ExitRequested 守卫接管为「意外销毁→重建」。
                        api.prevent_close();
                        app.exit(0);
                    }
                }
                WindowEvent::Destroyed => {
                    // 诊断锚点：与下方 run 回调的 ExitRequested 守卫配合，
                    // 区分「Tauri 正常退出路径」与「外部强杀」（外部 TerminateProcess 不进事件循环）
                    log::warn!("窗口销毁: {}", window.label());
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            // 坑（2026-09-30 定位）：Tauri/runtime 默认「最后一个窗口 Destroyed → 退出整个
            // 进程」。用户点 X 已被 CloseRequested 拦截，所以能走到这里的销毁只剩一种来源：
            // WebView2 进程死亡（Evergreen 运行时自动更新、系统资源回收等）——进程退出无任何
            // 崩溃痕迹（WER/事件日志/Crashpad 全干净），app.log 里表现为「日志戛然而止、数小时
            // 后新实例启动」。防护：拦下这类退出并重建主窗口，托盘常驻进程不倒。
            tauri::RunEvent::ExitRequested { code, api, .. } => match code {
                // 主动退出（托盘退出 / close_to_tray=false 关窗 / restart）：放行
                Some(_) => state::state(app).mark_exiting(),
                None => {
                    // 退出流程中顺带发生的窗口销毁（app.exit 后 ControlFlow::Exit 的清理阶段）
                    if !state::state(app).is_exiting() {
                        api.prevent_exit();
                        log::error!(
                            "主窗口意外销毁（WebView2 进程退出或运行时更新），已拦截退出并重建窗口"
                        );
                        rebuild_main_window(app);
                    }
                }
            },
            // 事件循环即将退出（主动退出路径终态，或 prevent 未生效）。留痕用于
            // 区分「Tauri 自身退出」与「外部 TerminateProcess 强杀」（后者 app.log 戛然而止、无此行）
            tauri::RunEvent::Exit => {
                log::warn!(
                    "Tauri 事件循环退出（exiting={}）",
                    state::state(app).is_exiting()
                );
            }
            _ => {}
        });
}

/// 重建主窗口（按 tauri.conf.json 的 main 配置，含 visible:false——保持隐藏、托盘常驻，
/// 用户点托盘图标再显示）。WebView2 运行时更新未就绪时 build 会失败，带退避重试。
fn rebuild_main_window(app: &tauri::AppHandle) {
    let Some(conf) = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
    else {
        log::error!("tauri.conf.json 中找不到 main 窗口配置，无法重建窗口");
        return;
    };
    match tauri::WebviewWindowBuilder::from_config(app, &conf).and_then(|b| b.build()) {
        Ok(_) => log::info!("主窗口已重建（保持隐藏，托盘常驻，签到调度不受影响）"),
        Err(e) => {
            log::error!("主窗口重建失败: {e}，稍后自动重试");
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                for delay_secs in [5u64, 15, 45, 120] {
                    tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
                    if app2.get_webview_window("main").is_some() {
                        return; // 别处（如重试竞态）已重建
                    }
                    match tauri::WebviewWindowBuilder::from_config(&app2, &conf)
                        .and_then(|b| b.build())
                    {
                        Ok(_) => {
                            log::info!("主窗口重建成功（退避重试后），保持隐藏、托盘常驻");
                            return;
                        }
                        Err(e) => log::error!("主窗口重建重试失败（{delay_secs}s 后再试）: {e}"),
                    }
                }
                log::error!("主窗口多次重建失败，应用以纯托盘模式运行（签到调度不受影响）");
            });
        }
    }
}

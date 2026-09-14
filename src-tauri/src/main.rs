// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Pi Desktop 桌面应用主入口
//!
//! 本文件是 Tauri 2 应用的启动入口，负责：
//! - 声明并加载各功能模块（bridge / node_manager / piweb / settings / tray）
//! - 初始化全局应用状态（pi-web 端口、子进程句柄、退出意图标记）
//! - 注册 Tauri 插件（窗口状态记忆、单实例锁）
//! - 注册所有前端可调用的命令（invoke_handler）
//! - 设置系统托盘与窗口关闭行为
//! - 构建并运行应用事件循环，退出时清理 pi-web 子进程

// ---- 模块声明 ----
mod bridge;       // pi-web 与壳层之间的注入桥（主题同步、历史窗口、外链跳转）
mod node_manager; // Node.js / Git 环境检测、下载安装、进度上报
mod piweb;        // pi-web 服务的安装、启动、停止、更新、监控重启
mod settings;     // 应用配置读写（关闭行为、镜像源、默认目录等）
mod tray;         // 系统托盘图标与菜单

use std::process::Child;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;

/// 全局应用状态（在 Tauri 中通过 `.manage()` 注册，各 command 可通过 `State<AppState>` 获取）
///
/// 包含 pi-web 服务运行时所需的共享状态：
/// - `piweb_port`：当前 pi-web 监听的端口（未启动时为 None）
/// - `piweb_child`：pi-web 子进程句柄，用于停止/监控重启
/// - `quit_intent`：退出意图原子标记，用于在「询问」关闭模式下区分
///   是用户主动退出应用（应放行）还是点窗口关闭按钮（应弹确认框），
///   避免确认弹窗与 CloseRequested 互相触发造成死循环
pub struct AppState {
    /// pi-web 当前监听的端口号（未启动时为 None）
    pub piweb_port: Mutex<Option<u16>>,
    /// pi-web 子进程句柄（用于停止进程 / 监控异常退出）
    pub piweb_child: Mutex<Option<Child>>,
    /// 退出意图标记：ask 关闭模式下用户点「退出应用」时置位，CloseRequested 据此放行
    pub quit_intent: AtomicBool,
}

/// 应用主入口函数：构建 Tauri 应用并启动事件循环
fn main() {
    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(tauri_plugin_window_state::StateFlags::SIZE)
                .build(),
        )
        // 单实例插件：若已有实例在运行，再次启动时唤起已有窗口而非开新进程
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Emitter;
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
                // 通知前端有二次启动请求（可用于跳转到特定页面等）
                let _ = app.emit("single-instance-requested", ());
                let _ = win.request_user_attention(Some(tauri::UserAttentionType::Informational));
            }
        }))
        .manage(AppState {
            piweb_port: Mutex::new(None),
            piweb_child: Mutex::new(None),
            quit_intent: AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            node_manager::check_node,
            node_manager::install_node,
            node_manager::install_git,
            node_manager::get_node_download_url,
            node_manager::open_url,
            node_manager::pick_directory,
            piweb::check_environment,
            piweb::is_piweb_installed,
            piweb::get_installed_versions,
            piweb::install_piweb,
            piweb::update_component,
            piweb::start_piweb,
            piweb::stop_piweb,
            piweb::restart_piweb,
            piweb::get_piweb_url,
            bridge::open_history_window,
            // 设置相关
            settings::get_settings,
            settings::set_settings,
            settings::update_setting,
            settings::get_settings_path,
            settings::get_config_dir,
            settings::open_config_dir,
            settings::open_dir,
        ])
        .setup(|app| {
            // ---- 系统托盘初始化 ----
            // 失败时（如图标解码错误）留日志到 <exe>\config\tray-init-error.log 便于排查；
            // 托盘缺失不应阻断主窗口使用，所以仅记录而不中止启动
            if let Err(e) = tray::setup_tray(app.handle()) {
                let log_dir = crate::node_manager::get_data_dir().join("config");
                let _ = std::fs::create_dir_all(&log_dir);
                let _ = std::fs::write(
                    log_dir.join("tray-init-error.log"),
                    format!("tray setup failed: {e:?}\n"),
                );
            }

            // ---- 注入 pi-web 桥脚本（仅 Windows，利用 WebView2 的 AddScriptToExecuteOnDocumentCreated） ----
            #[cfg(windows)]
            bridge::install_bridge(app.handle());

            // ---- 注册主窗口关闭事件：根据配置决定隐藏到托盘 / 直接退出 / 询问确认 ----
            let app_handle = app.handle().clone();
            if let Some(win) = app.get_webview_window("main") {
                let ah = app_handle.clone();
                win.on_window_event(move |event| {
                    // 只处理关闭请求事件
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        use tauri::Emitter;
                        let s = settings::AppSettings::load();
                        match s.close_behavior {
                            // 模式一：关闭到托盘——阻止窗口关闭，仅隐藏
                            settings::CloseBehavior::Tray => {
                                api.prevent_close();
                                if let Some(w) = ah.get_webview_window("main") {
                                    let _ = w.hide();
                                }
                            }
                            // 模式二：直接退出——清理 pi-web 子进程后放行关闭
                            settings::CloseBehavior::Quit => {
                                if let Some(state) = ah.try_state::<AppState>() {
                                    piweb::stop_all(&state);
                                }
                                // 不调用 prevent_close，窗口正常关闭
                            }
                            // 模式三：询问确认——通知前端弹确认框
                            // 特殊情况：若 quit_intent 已被置位（用户点了「退出应用」），
                            // 则直接放行，避免确认弹窗与 CloseRequested 互相触发死循环
                            settings::CloseBehavior::Ask => {
                                let quitting = ah
                                    .try_state::<AppState>()
                                    .map(|s| s.quit_intent.swap(false, Ordering::SeqCst))
                                    .unwrap_or(false);
                                if !quitting {
                                    api.prevent_close();
                                    // 通知前端弹出关闭确认对话框
                                    let _ = ah.emit("request-close-confirm", ());
                                }
                            }
                        }
                    }
                });
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // ---- 运行应用事件循环 ----
    app.run(|app_handle, event| {
        // 应用即将退出时，确保 pi-web 子进程被清理（防孤儿进程）
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = app_handle.try_state::<AppState>() {
                piweb::stop_all(&state);
            }
        }
    });
}

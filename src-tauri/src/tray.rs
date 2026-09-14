//! 系统托盘模块
//!
//! 负责创建并管理系统托盘图标与右键菜单，提供以下功能：
//! - 显示/隐藏主窗口
//! - 更新 Pi Web / Pi Coding Agent
//! - 退出应用
//! - 左键点击托盘图标切换窗口显示状态

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;

/// 初始化系统托盘：创建图标、菜单、绑定事件回调
pub fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let hide_item = MenuItem::with_id(app, "hide", "隐藏窗口", true, None::<&str>)?;
    let update_web = MenuItem::with_id(app, "update-web", "更新 Pi Web", true, None::<&str>)?;
    let update_agent =
        MenuItem::with_id(app, "update-agent", "更新 Pi Coding Agent", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &show_item,
            &hide_item,
            &PredefinedMenuItem::separator(app)?,
            &update_web,
            &update_agent,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;

    // 托盘图标：直接内嵌 32x32.png（全出血版，主体放大填充，在托盘 16~32px 槽位下清晰饱满）。
    // 用 PNG 而非 .ico：PNG 只需 image-png feature 即可解码；若改回 .ico 必须同时启用
    // tauri 的 image-ico feature，否则 Image::from_bytes 报 Unsupported，托盘会静默不显示。
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("Pi Desktop")
        .menu(&menu)
        // 左键不弹菜单（左键用于切换窗口显示/隐藏），右键才弹菜单
        .show_menu_on_left_click(false)
        // 菜单项点击事件
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "hide" => hide_main(app),
            "update-web" => crate::piweb::perform_update(app.clone(), "web".into()),
            "update-agent" => crate::piweb::perform_update(app.clone(), "agent".into()),
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        // 托盘图标点击事件：左键抬起切换主窗口显示/隐藏状态
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    if let Ok(visible) = window.is_visible() {
                        if visible {
                            let _ = window.hide();
                        } else {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                }
            }
        })
        .build(app)?;

    Ok(())
}

/// 显示主窗口并置于前台聚焦
fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 隐藏主窗口（最小化到托盘）
fn hide_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

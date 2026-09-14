//! 壳层配置管理：默认目录、关闭行为、代理等
//!
//! 配置文件位置：<安装目录>\config\settings.json
//! （与自托管运行时同目录，用户可查看/修改/备份）

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::node_manager::get_data_dir;

/// 获取配置文件所在目录路径：<应用数据目录>/config
fn config_dir() -> PathBuf {
    get_data_dir().join("config")
}

/// 获取完整的配置文件路径：<应用数据目录>/config/settings.json
fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CloseBehavior {
    Tray,    // 最小化到托盘
    Quit,    // 直接退出
    Ask,     // 每次询问（默认）
}

impl Default for CloseBehavior {
    /// 默认关闭行为：点击关闭按钮时先弹窗询问（最小化到托盘 / 退出 / 取消）
    fn default() -> Self {
        CloseBehavior::Ask
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppSettings {
    /// 默认项目目录（pi-web 启动时自动 ?cwd=）
    pub default_cwd: Option<String>,

    /// 关闭按钮行为
    #[serde(default)]
    pub close_behavior: CloseBehavior,

    /// npm 镜像源（默认国内 npmmirror，可在设置里清空切回官方）
    #[serde(default)]
    pub npm_mirror: Option<String>,

    /// node 下载镜像（默认国内 npmmirror，可在设置里清空切回官方）
    #[serde(default)]
    pub node_mirror: Option<String>,

    /// github 下载代理（默认国内 ghfast.top，用于 git 下载等）
    #[serde(default)]
    pub gh_proxy: Option<String>,
}

impl Default for AppSettings {
    /// 默认配置：关闭行为为「每次询问」，国内镜像源全部启用
    fn default() -> Self {
        Self {
            default_cwd: None,
            close_behavior: CloseBehavior::Ask,
            // 默认全部走国内镜像源（下载速度快）；用户可在设置面板清空以切回官方源
            npm_mirror: Some("https://registry.npmmirror.com".into()),
            node_mirror: Some("https://npmmirror.com/mirrors/node".into()),
            gh_proxy: Some("https://ghfast.top".into()),
        }
    }
}

impl AppSettings {
    /// 从磁盘加载配置；文件不存在或解析失败时返回默认配置
    pub fn load() -> Self {
        let path = settings_file();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(s) = serde_json::from_str::<AppSettings>(&content) {
                    return s;
                }
            }
        }
        Self::default()
    }

    /// 将当前配置以格式化 JSON 写入磁盘；自动创建目录
    pub fn save(&self) -> Result<(), String> {
        let dir = config_dir();
        fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {}", e))?;
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("序列化配置失败: {}", e))?;
        fs::write(settings_file(), content).map_err(|e| format!("写入配置失败: {}", e))?;
        Ok(())
    }
}

/// 获取配置文件路径（供前端"打开配置目录"用）
#[tauri::command]
pub fn get_settings_path() -> String {
    settings_file().to_string_lossy().to_string()
}

/// 获取配置目录路径
#[tauri::command]
pub fn get_config_dir() -> String {
    config_dir().to_string_lossy().to_string()
}

/// 读取全部配置
#[tauri::command]
pub fn get_settings() -> AppSettings {
    AppSettings::load()
}

/// 保存全部配置（返回新的配置）
#[tauri::command]
pub fn set_settings(settings: AppSettings) -> Result<AppSettings, String> {
    settings.save()?;
    Ok(settings)
}

/// 将 JSON 值转换为 Option<String>：空字符串视为未设置（None）
/// 用于前端传来空字符串表示「清除该配置项」的场景
fn opt_string(value: &serde_json::Value) -> Option<String> {
    match value.as_str() {
        Some(v) if !v.is_empty() => Some(v.to_string()),
        _ => None,
    }
}

/// 更新单个字段（便捷方法）
#[tauri::command]
pub fn update_setting(key: String, value: serde_json::Value) -> Result<AppSettings, String> {
    let mut s = AppSettings::load();
    match key.as_str() {
        "default_cwd" => {
            s.default_cwd = opt_string(&value);
        }
        "close_behavior" => {
            s.close_behavior = match value.as_str().unwrap_or("ask") {
                "quit" => CloseBehavior::Quit,
                "ask" => CloseBehavior::Ask,
                _ => CloseBehavior::Tray,
            };
        }
        "npm_mirror" => {
            s.npm_mirror = opt_string(&value);
        }
        "node_mirror" => {
            s.node_mirror = opt_string(&value);
        }
        "gh_proxy" => {
            s.gh_proxy = opt_string(&value);
        }
        _ => return Err(format!("未知配置项: {}", key)),
    }
    s.save()?;
    Ok(s)
}

/// 打开配置目录（用资源管理器）
#[tauri::command]
pub fn open_config_dir() -> bool {
    let dir = config_dir();
    let _ = fs::create_dir_all(&dir);
    open_path_in_explorer(&dir)
}

/// 打开指定目录（用资源管理器）。
/// 路径必须存在且为目录，否则返回 false。
#[tauri::command]
pub fn open_dir(path: String) -> bool {
    let p = PathBuf::from(path);
    if !p.exists() || !p.is_dir() {
        return false;
    }
    open_path_in_explorer(&p)
}

/// 用系统默认文件管理器打开一个目录路径
fn open_path_in_explorer(dir: &std::path::Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("explorer.exe");
        cmd.arg(dir);
        crate::node_manager::apply_no_window(&mut cmd);
        cmd.spawn().is_ok()
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .is_ok()
    }
}

//! Node.js / Git 环境管理模块
//!
//! 负责运行环境的检测、下载和安装：
//! - Node.js 版本检测、下载、安装（自托管便携版）
//! - Git 下载与安装（便携版）
//! - 在 PATH 中查找可执行文件
//! - 带进度上报的文件下载（PowerShell + 进度文件轮询）
//! - 目录选择对话框
//! - 统一的子进程无窗口启动工具

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::Emitter;

/// Windows 下以不弹出控制台窗口的方式启动子进程（CREATE_NO_WINDOW 标志）
pub fn apply_no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NodeCheckResult {
    /// 是否已找到 node 可执行文件
    pub has_node: bool,
    /// node 版本号（获取失败时为 None）
    pub node_version: Option<String>,
    /// node 可执行文件路径
    pub node_path: Option<String>,
    /// 是否满足版本要求（>= 22）
    pub meets_requirement: bool,
    /// 错误信息（检测过程出错时）
    pub error: Option<String>,
}

/// Node.js 来源类型
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NodeSource {
    Env,        // 系统 PATH 环境变量中找到
    Official,   // 官方安装目录（C:\Program Files\nodejs）
    Selfhosted, // 应用内自托管便携版
    Missing,    // 未找到
}

/// Git 来源类型
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "lowercase")]
pub enum GitSource {
    Env,        // 系统 PATH 环境变量中找到
    Official,   // 官方安装目录（C:\Program Files\Git）
    Selfhosted, // 应用内自托管便携版
    Missing,    // 未找到
}

/// 综合环境检测结果（前端展示用）
#[derive(Serialize, Deserialize, Debug)]
pub struct EnvCheckResult {
    /// Node.js 来源
    pub node_source: NodeSource,
    /// Node.js 可执行文件路径
    pub node_path: Option<String>,
    /// Node.js 版本号
    pub node_version: Option<String>,
    /// Node.js 版本是否满足要求（>= 22）
    pub node_meets_req: bool,
    /// Git 是否已安装
    pub git_installed: bool,
    /// Git 来源
    pub git_source: GitSource,
    /// pi-web 是否已安装
    pub pi_web_installed: bool,
    /// pi-coding-agent 是否已安装
    pub pi_coding_agent_installed: bool,
    /// 是否需要执行安装流程
    pub needs_install: bool,
}

/// 下载进度事件结构（通过 Tauri event 推送给前端）
#[derive(Serialize, Clone, Debug)]
pub struct DownloadProgress {
    /// 下载目标："node" / "git" / "pi-web" / "pi-coding-agent"
    pub target: String,
    /// 当前阶段："downloading" / "extracting" / "installing" / "done" / "error"
    pub stage: String,
    /// 当前使用的下载源描述（如「官方源」「国内镜像」等）
    pub source: String,
    /// 下载进度百分比 0-100
    pub percent: f64,
    /// 下载速度（KB/s）
    pub speed_kb_s: f64,
    /// 已下载大小（MB）
    pub downloaded_mb: f64,
    /// 文件总大小（MB），未知时为 0
    pub total_mb: f64,
    /// 附加信息文本
    pub message: String,
}

/// 在 PATH 中查找系统可执行文件
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exe_name = if cfg!(windows) {
        if name.to_lowercase().ends_with(".exe") || name.to_lowercase().ends_with(".cmd") {
            name.to_string()
        } else {
            format!("{}.exe", name)
        }
    } else {
        name.to_string()
    };

    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(&exe_name);
        if candidate.exists() {
            let s = candidate.to_string_lossy().to_lowercase();
            if s.contains("electron") || s.contains("app-") {
                continue;
            }
            return Some(candidate);
        }
    }
    None
}

#[tauri::command]
pub fn check_node() -> NodeCheckResult {
    match find_in_path("node") {
        Some(path) => {
            let mut cmd = Command::new(&path);
            cmd.arg("-v");
            apply_no_window(&mut cmd);
            match cmd.output() {
                Ok(output) => {
                    if output.status.success() {
                        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        let major = version
                            .trim_start_matches('v')
                            .split('.')
                            .next()
                            .and_then(|s| s.parse::<u32>().ok());
                        let meets = major.map(|m| m >= 22).unwrap_or(false);
                        NodeCheckResult {
                            has_node: true,
                            node_version: Some(version),
                            node_path: Some(path.to_string_lossy().to_string()),
                            meets_requirement: meets,
                            error: None,
                        }
                    } else {
                        NodeCheckResult {
                            has_node: true,
                            node_version: None,
                            node_path: Some(path.to_string_lossy().to_string()),
                            meets_requirement: false,
                            error: Some("Failed to get node version".into()),
                        }
                    }
                }
                Err(e) => NodeCheckResult {
                    has_node: true,
                    node_version: None,
                    node_path: Some(path.to_string_lossy().to_string()),
                    meets_requirement: false,
                    error: Some(e.to_string()),
                },
            }
        }
        None => NodeCheckResult {
            has_node: false,
            node_version: None,
            node_path: None,
            meets_requirement: false,
            error: None,
        },
    }
}

/// Node.js 官方下载地址（Windows x64 LTS v22）
#[tauri::command]
pub fn get_node_download_url() -> String {
    "https://nodejs.org/dist/v22.14.0/node-v22.14.0-x64.msi".into()
}

/// 用系统默认方式打开一个 URL
#[tauri::command]
pub fn open_url(url: String) -> bool {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "start", "", &url]);
        apply_no_window(&mut cmd);
        cmd.spawn().is_ok()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new("xdg-open").arg(&url).spawn().is_ok()
    }
}

/// 系统目录选择对话框
#[tauri::command]
pub fn pick_directory() -> Option<String> {
    let dir = get_data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let script = dir.join("pickdir.ps1");
    let content = String::from("\u{FEFF}Add-Type -AssemblyName System.Windows.Forms\r\n")
        + "$f = New-Object System.Windows.Forms.FolderBrowserDialog\r\n"
        + "$f.Description = '选择默认项目目录'\r\n"
        + "$f.ShowNewFolderButton = $true\r\n"
        + "if ($f.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {\r\n"
        + "  [Console]::OutputEncoding = [System.Text.Encoding]::UTF8\r\n"
        + "  Write-Output $f.SelectedPath\r\n"
        + "}\r\n";
    if std::fs::write(&script, content).is_err() {
        return None;
    }
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-Sta", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script);
    apply_no_window(&mut cmd);
    let out = cmd.output().ok();
    let _ = std::fs::remove_file(&script);
    match out {
        Some(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        }
        _ => None,
    }
}

pub fn get_data_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub fn selfhosted_node_dir() -> PathBuf {
    get_data_dir().join("node")
}

pub fn selfhosted_node_exe() -> PathBuf {
    selfhosted_node_dir().join("node.exe")
}

pub fn official_node_exe() -> PathBuf {
    PathBuf::from(r"C:\Program Files\nodejs\node.exe")
}

fn node_version_at(path: &PathBuf) -> Option<String> {
    let mut cmd = Command::new(path);
    cmd.arg("-v");
    apply_no_window(&mut cmd);
    cmd.output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

pub fn node_meets_requirement(version: &str) -> bool {
    version
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .map(|m| m >= 22)
        .unwrap_or(false)
}

pub fn detect_node() -> (NodeSource, Option<String>, Option<String>, bool) {
    if let Some(p) = find_in_path("node") {
        if let Some(v) = node_version_at(&p) {
            return (
                NodeSource::Env,
                Some(p.to_string_lossy().to_string()),
                Some(v.clone()),
                node_meets_requirement(&v),
            );
        }
    }
    let off = official_node_exe();
    if off.exists() {
        if let Some(v) = node_version_at(&off) {
            return (
                NodeSource::Official,
                Some(off.to_string_lossy().to_string()),
                Some(v.clone()),
                node_meets_requirement(&v),
            );
        }
    }
    let sh = selfhosted_node_exe();
    if sh.exists() {
        if let Some(v) = node_version_at(&sh) {
            return (
                NodeSource::Selfhosted,
                Some(sh.to_string_lossy().to_string()),
                Some(v.clone()),
                node_meets_requirement(&v),
            );
        }
    }
    (NodeSource::Missing, None, None, false)
}

pub fn resolve_node() -> Option<PathBuf> {
    if let Some(p) = find_in_path("node") {
        return Some(p);
    }
    let off = official_node_exe();
    if off.exists() {
        return Some(off);
    }
    let sh = selfhosted_node_exe();
    if sh.exists() {
        return Some(sh);
    }
    None
}

pub fn resolve_npm() -> Option<PathBuf> {
    if let Some(p) = find_in_path("npm.cmd").or_else(|| find_in_path("npm")) {
        return Some(p);
    }
    let off = PathBuf::from(r"C:\Program Files\nodejs\npm.cmd");
    if off.exists() {
        return Some(off);
    }
    let sh = selfhosted_node_dir().join("npm.cmd");
    if sh.exists() {
        return Some(sh);
    }
    None
}

// ==================== 带进度的下载（PowerShell + 进度文件） ====================

/// 下载文件，支持进度上报（通过轮询临时进度文件）
/// 返回 (是否成功, 最终使用的源描述)
/// 
/// 原理：PowerShell 中用 System.Net.WebClient 下载，每 200ms 把进度写入临时文件；
/// Rust 侧轮询该文件并 emit 事件。
fn download_with_progress<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    urls: &[(String, String)],  // (url, source_label)
    out_path: &PathBuf,
    target: &str,
) -> (bool, String) {
    let tmp_dir = get_data_dir().join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let progress_file = tmp_dir.join(format!("dl-progress-{}.txt", target));
    let _ = std::fs::remove_file(&progress_file);

    // 按顺序尝试每个源
    for (url, source_label) in urls {
        let _ = std::fs::remove_file(out_path);
        let _ = std::fs::remove_file(&progress_file);

        // 生成 PowerShell 下载脚本（带进度）
        let ps_script = format!(
            r#"
$ErrorActionPreference = 'Stop'
$url = '{}'
$out = '{}'
$progressFile = '{}'

$web = New-Object System.Net.WebClient
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$lastWrite = Get-Date

Register-ObjectEvent -InputObject $web -EventName DownloadProgressChanged -Action {{
    param($sender, $e)
    $now = Get-Date
    if (($now - $lastWrite).TotalMilliseconds -ge 200) {{
        $speed = if ($sw.Elapsed.TotalSeconds -gt 0) {{ $e.BytesReceived / $sw.Elapsed.TotalSeconds }} else {{ 0 }}
        $line = "$($e.ProgressPercentage)|$($e.BytesReceived)|$($e.TotalBytesToReceive)|$speed"
        Set-Content -Path $progressFile -Value $line -Encoding UTF8
        $script:lastWrite = $now
    }}
}} | Out-Null

try {{
    $web.DownloadFile($url, $out)
    $sw.Stop()
    'OK' | Out-File -FilePath $progressFile -Encoding UTF8
}} catch {{
    "ERR|$($_.Exception.Message)" | Out-File -FilePath $progressFile -Encoding UTF8
    exit 1
}}
"#,
            url,
            out_path.to_string_lossy().replace("'", "''"),
            progress_file.to_string_lossy().replace("'", "''"),
        );

        let script_file = tmp_dir.join(format!("dl-{}.ps1", target));
        if std::fs::write(&script_file, ps_script).is_err() {
            continue;
        }

        // 启动下载进程
        let mut cmd = Command::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script_file.to_str().unwrap(),
        ]);
        apply_no_window(&mut cmd);

        let child = cmd.spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(_) => continue,
        };

        // 轮询进度文件并 emit 事件
        let mut last_percent = -1.0;
        loop {
            // 检查进程是否结束
            match child.try_wait() {
                Ok(Some(status)) => {
                    // 进程结束
                    if status.success() && out_path.exists() {
                        // 发送 100% 进度
                        let _ = app.emit(
                            "download-progress",
                            DownloadProgress {
                                target: target.to_string(),
                                stage: "downloading".into(),
                                source: source_label.clone(),
                                percent: 100.0,
                                speed_kb_s: 0.0,
                                downloaded_mb: 0.0,
                                total_mb: 0.0,
                                message: "下载完成".into(),
                            },
                        );
                        let _ = std::fs::remove_file(&script_file);
                        let _ = std::fs::remove_file(&progress_file);
                        return (true, source_label.clone());
                    } else {
                        break; // 失败，试下一个源
                    }
                }
                Ok(None) => {
                    // 还在运行，读进度文件
                    if let Ok(content) = std::fs::read_to_string(&progress_file) {
                        let content = content.trim();
                        if content.starts_with("OK") {
                            continue;
                        }
                        if content.starts_with("ERR") {
                            break;
                        }
                        let parts: Vec<&str> = content.split('|').collect();
                        if parts.len() >= 4 {
                            if let (Ok(percent), Ok(received), Ok(total), Ok(speed)) = (
                                parts[0].parse::<f64>(),
                                parts[1].parse::<f64>(),
                                parts[2].parse::<f64>(),
                                parts[3].parse::<f64>(),
                            ) {
                                if (percent - last_percent).abs() > 0.5 {
                                    last_percent = percent;
                                    let _ = app.emit(
                                        "download-progress",
                                        DownloadProgress {
                                            target: target.to_string(),
                                            stage: "downloading".into(),
                                            source: source_label.clone(),
                                            percent,
                                            speed_kb_s: speed / 1024.0,
                                            downloaded_mb: received / 1024.0 / 1024.0,
                                            total_mb: if total > 0.0 { total / 1024.0 / 1024.0 } else { 0.0 },
                                            message: format!("正在从 {} 下载...", source_label),
                                        },
                                    );
                                }
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
                Err(_) => break,
            }
        }

        // 这个源失败了，继续试下一个
        let _ = std::fs::remove_file(out_path);
        let _ = std::fs::remove_file(&script_file);
        let _ = std::fs::remove_file(&progress_file);
    }

    // 所有源都失败
    let _ = app.emit(
        "download-progress",
        DownloadProgress {
            target: target.to_string(),
            stage: "error".into(),
            source: "所有源均失败".into(),
            percent: 0.0,
            speed_kb_s: 0.0,
            downloaded_mb: 0.0,
            total_mb: 0.0,
            message: "下载失败，请检查网络".into(),
        },
    );
    (false, String::new())
}

/// 平铺解压 zip（剥离顶层目录）
fn extract_zip_flat(zip_path: &PathBuf, dest: &PathBuf) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let top = if archive.len() > 0 {
        archive
            .by_index(0)
            .ok()
            .map(|f| {
                let name = f.name().to_string();
                name.split('/').next().unwrap_or("").to_string()
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let mut name = entry.name().to_string();
        if !top.is_empty() && name.starts_with(&top) {
            name = name[top.len()..].to_string();
        }
        let name = name.trim_start_matches('/').to_string();
        if name.is_empty() { continue; }
        let out_path = dest.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ==================== Node.js 安装（带进度+多源） ====================

#[derive(Serialize, Deserialize, Debug)]
pub struct NodeInstallResult {
    pub success: bool,
    pub message: String,
    pub source_used: Option<String>,
}

/// 获取 node 下载源列表（官方优先，然后镜像）
fn get_node_download_urls(version: &str) -> Vec<(String, String)> {
    use crate::settings::AppSettings;
    let settings = AppSettings::load();
    let mut urls = Vec::new();

    // 用户自定义 node 镜像优先（如果设置了）
    if let Some(mirror) = &settings.node_mirror {
        if !mirror.is_empty() {
            urls.push((
                format!("{}/{}/node-{}-win-x64.zip", mirror.trim_end_matches('/'), version, version),
                "自定义镜像".into(),
            ));
        }
    }

    // 官方源
    urls.push((
        format!("https://nodejs.org/dist/{}/node-{}-win-x64.zip", version, version),
        "官方源 nodejs.org".into(),
    ));

    // 国内镜像兜底
    urls.push((
        format!("https://npmmirror.com/mirrors/node/{}/node-{}-win-x64.zip", version, version),
        "国内镜像 npmmirror.com".into(),
    ));

    urls
}

#[tauri::command]
pub fn install_node(app: tauri::AppHandle) -> NodeInstallResult {
    let node_dir = selfhosted_node_dir();
    if node_dir.join("node.exe").exists() {
        return NodeInstallResult {
            success: true,
            message: "Node.js 已安装".into(),
            source_used: None,
        };
    }
    if let Err(e) = std::fs::create_dir_all(&node_dir) {
        return NodeInstallResult {
            success: false,
            message: format!("创建目录失败: {}", e),
            source_used: None,
        };
    }

    let version = "v22.14.0";
    let zip_path = node_dir.join("node.zip");
    let urls = get_node_download_urls(version);

    let (ok, source) = download_with_progress(&app, &urls, &zip_path, "node");
    if !ok {
        let _ = std::fs::remove_file(&zip_path);
        return NodeInstallResult {
            success: false,
            message: "下载 Node.js 失败（所有源均不可用，请检查网络）".into(),
            source_used: None,
        };
    }

    // 发送解压中状态
    let _ = app.emit("download-progress", DownloadProgress {
        target: "node".into(),
        stage: "extracting".into(),
        source: source.clone(),
        percent: 95.0,
        speed_kb_s: 0.0,
        downloaded_mb: 0.0,
        total_mb: 0.0,
        message: "正在解压...".into(),
    });

    match extract_zip_flat(&zip_path, &node_dir) {
        Ok(_) => {
            let _ = std::fs::remove_file(&zip_path);
            let _ = app.emit("download-progress", DownloadProgress {
                target: "node".into(),
                stage: "done".into(),
                source: source.clone(),
                percent: 100.0,
                speed_kb_s: 0.0,
                downloaded_mb: 0.0,
                total_mb: 0.0,
                message: "Node.js 安装成功".into(),
            });
            NodeInstallResult {
                success: true,
                message: "Node.js 安装成功".into(),
                source_used: Some(source),
            }
        }
        Err(e) => {
            let _ = std::fs::remove_file(&zip_path);
            NodeInstallResult {
                success: false,
                message: format!("解压失败: {}", e),
                source_used: None,
            }
        }
    }
}

// ==================== Git 安装（带进度+多源） ====================

pub fn selfhosted_git_dir() -> PathBuf {
    get_data_dir().join("git")
}

pub fn selfhosted_git_exe() -> PathBuf {
    selfhosted_git_dir().join("cmd").join("git.exe")
}

pub fn official_git_exe() -> PathBuf {
    PathBuf::from(r"C:\Program Files\Git\cmd\git.exe")
}

fn git_root_of(exe: &Path) -> PathBuf {
    let dir = exe.parent().unwrap_or(exe);
    if dir.file_name().map(|s| s.to_string_lossy().to_lowercase()) == Some("cmd".into()) {
        dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| dir.to_path_buf())
    } else {
        dir.to_path_buf()
    }
}

pub fn detect_git() -> (GitSource, Option<String>) {
    if let Some(p) = find_in_path("git") {
        return (GitSource::Env, Some(p.to_string_lossy().to_string()));
    }
    let off = official_git_exe();
    if off.exists() {
        return (GitSource::Official, Some(off.to_string_lossy().to_string()));
    }
    let sh = selfhosted_git_exe();
    if sh.exists() {
        return (GitSource::Selfhosted, Some(sh.to_string_lossy().to_string()));
    }
    (GitSource::Missing, None)
}

pub fn resolve_git_dirs() -> Vec<PathBuf> {
    let exe = if let Some(p) = find_in_path("git") {
        Some(p)
    } else if official_git_exe().exists() {
        Some(official_git_exe())
    } else if selfhosted_git_exe().exists() {
        Some(selfhosted_git_exe())
    } else {
        None
    };
    let Some(exe) = exe else { return Vec::new() };
    let root = git_root_of(&exe);
    let mut dirs = Vec::new();
    let mut push = |d: PathBuf| { if d.is_dir() { dirs.push(d); } };
    push(root.join("cmd"));
    push(root.join("bin"));
    push(root.join("usr").join("bin"));
    dirs
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GitInstallResult {
    pub success: bool,
    pub message: String,
    pub source_used: Option<String>,
}

fn git_portable_download_url() -> Option<String> {
    let script = "$r = Invoke-RestMethod -Uri 'https://api.github.com/repos/git-for-windows/git/releases/latest' -Headers @{ 'User-Agent' = 'pi-desktop' }; $a = $r.assets | Where-Object { $_.name -match '^PortableGit-.*-64-bit\\.7z\\.exe$' } | Select-Object -First 1; if ($a) { Write-Output $a.browser_download_url }";
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script]);
    apply_no_window(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn get_git_download_urls(github_url: &str) -> Vec<(String, String)> {
    use crate::settings::AppSettings;
    let settings = AppSettings::load();
    let mut urls = Vec::new();

    // GitHub 官方直链
    urls.push((github_url.to_string(), "GitHub 官方".into()));

    // 用户自定义 gh_proxy
    if let Some(proxy) = &settings.gh_proxy {
        if !proxy.is_empty() {
            urls.push((
                format!("{}/{}", proxy.trim_end_matches('/'), github_url),
                "自定义 GitHub 代理".into(),
            ));
        }
    }

    // 常用 GitHub 镜像兜底
    urls.push((
        format!("https://ghfast.top/{}", github_url),
        "GitHub 镜像 ghfast.top".into(),
    ));
    urls.push((
        format!("https://gh-proxy.com/{}", github_url),
        "GitHub 镜像 gh-proxy.com".into(),
    ));

    urls
}

#[tauri::command]
pub fn install_git(app: tauri::AppHandle) -> GitInstallResult {
    let git_dir = selfhosted_git_dir();
    if git_dir.join("cmd").join("git.exe").exists() {
        return GitInstallResult { success: true, message: "Git 已安装".into(), source_used: None };
    }
    if let Err(e) = std::fs::create_dir_all(&git_dir) {
        return GitInstallResult { success: false, message: format!("创建目录失败: {}", e), source_used: None };
    }

    let Some(url) = git_portable_download_url() else {
        return GitInstallResult { success: false, message: "获取 Git 下载地址失败（请检查网络）".into(), source_used: None };
    };

    let urls = get_git_download_urls(&url);
    let sfx = git_dir.join("portable-git.exe");
    let (ok, source) = download_with_progress(&app, &urls, &sfx, "git");

    if !ok {
        let _ = std::fs::remove_file(&sfx);
        return GitInstallResult { success: false, message: "下载 Git 失败（所有源均不可用，请检查网络）".into(), source_used: None };
    }

    // 校验大小（避免 404 页面）
    if std::fs::metadata(&sfx).map(|md| md.len() < 1_000_000).unwrap_or(true) {
        let _ = std::fs::remove_file(&sfx);
        return GitInstallResult { success: false, message: "下载文件异常（文件过小）".into(), source_used: None };
    }

    // 解压中
    let _ = app.emit("download-progress", DownloadProgress {
        target: "git".into(),
        stage: "extracting".into(),
        source: source.clone(),
        percent: 95.0,
        speed_kb_s: 0.0,
        downloaded_mb: 0.0,
        total_mb: 0.0,
        message: "正在解压（Git 体积较大，请耐心等待）...".into(),
    });

    let mut cmd = Command::new("powershell.exe");
    let target = git_dir.to_string_lossy();
    let arg = format!("-o\"{}\"", target);
    cmd.args(["-NoProfile", "-Command", &format!("Start-Process -FilePath '{}' -ArgumentList '-y','{}' -Wait -PassThru | Out-Null", sfx.to_string_lossy(), arg)]);
    apply_no_window(&mut cmd);
    let st = cmd.status();
    let _ = std::fs::remove_file(&sfx);

    if git_dir.join("cmd").join("git.exe").exists() {
        let has_bash = git_dir.join("bin").join("bash.exe").exists();
        let _ = app.emit("download-progress", DownloadProgress {
            target: "git".into(),
            stage: "done".into(),
            source: source.clone(),
            percent: 100.0,
            speed_kb_s: 0.0,
            downloaded_mb: 0.0,
            total_mb: 0.0,
            message: "Git 安装成功".into(),
        });
        GitInstallResult {
            success: true,
            message: if has_bash { "Git 安装成功（含 Git Bash）".into() } else { "Git 安装成功".into() },
            source_used: Some(source),
        }
    } else {
        GitInstallResult {
            success: false,
            message: format!("Git 解压失败（{}）", st.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string())),
            source_used: None,
        }
    }
}

//! pi-web 服务管理模块
//!
//! 负责 pi-web 及其依赖（pi-coding-agent）的全生命周期管理：
//! - 环境检测与版本查询
//! - 安装与更新（含备份、失败回滚）
//! - 启动 / 停止 / 重启
//! - 进程异常退出自动监控与重启
//! - 端口探测与服务就绪检测
//! - 代理引导注入（telegram-proxy-init）
//! - 安装/更新进度事件上报

use crate::node_manager::{find_in_path, get_data_dir, resolve_node, resolve_npm, selfhosted_node_dir};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

#[derive(Serialize, Deserialize, Debug)]
pub struct InstallResult {
    /// 是否成功
    pub success: bool,
    /// 结果描述信息（成功/失败原因）
    pub message: String,
}

/// 已安装组件版本信息
#[derive(Serialize, Deserialize, Debug)]
pub struct Versions {
    /// pi-web 当前版本（未安装为 None）
    pub pi_web: Option<String>,
    /// pi-coding-agent 当前版本（未安装为 None）
    pub pi_coding_agent: Option<String>,
}

/// 获取 pi-web 运行时目录：<应用数据目录>/runtime
/// 自托管安装的 npm 包都放在这里
fn piweb_dir() -> PathBuf {
    get_data_dir().join("runtime")
}

/// 查找当前活跃的 Node.js 所在目录
/// 优先级：PATH 环境变量 → 官方安装目录 → 自托管目录
fn active_node_dir() -> Option<PathBuf> {
    if let Some(p) = find_in_path("node") {
        return p.parent().map(|d| d.to_path_buf());
    }
    let off_dir = Path::new(r"C:\Program Files\nodejs");
    if off_dir.join("node.exe").exists() {
        return Some(off_dir.to_path_buf());
    }
    let sh = selfhosted_node_dir();
    if sh.join("node.exe").exists() {
        return Some(sh);
    }
    None
}

/// 构造注入后的 PATH 环境变量值
/// 将 Node.js 和 Git 的目录前置到 PATH 最前面，确保子进程使用正确的版本
fn injected_path() -> String {
    let mut prefix = String::new();
    if let Some(d) = active_node_dir() {
        prefix.push_str(&format!("{};", d.to_string_lossy()));
    }
    for d in crate::node_manager::resolve_git_dirs() {
        prefix.push_str(&format!("{};", d.to_string_lossy()));
    }
    prefix + &std::env::var("PATH").unwrap_or_default()
}

/// 查找 pi-web 可执行脚本的位置
/// 优先查找全局 npm 安装，其次查找应用内 runtime 目录
/// 返回 (pi-web.js 路径, npm 根目录 / runtime 目录)
fn resolve_piweb() -> Option<(PathBuf, PathBuf)> {
    if let Some(g) = global_npm_root() {
        let gb = g.join("@agegr").join("pi-web").join("bin").join("pi-web.js");
        if gb.exists() {
            return Some((gb, g));
        }
    }
    let r = piweb_dir();
    let rb = r.join("node_modules").join("@agegr").join("pi-web").join("bin").join("pi-web.js");
    if rb.exists() {
        return Some((rb, r));
    }
    None
}

/// 获取全局 npm 根目录（通过 `npm root -g` 命令查询）
fn global_npm_root() -> Option<PathBuf> {
    let npm = resolve_npm()?;
    let npm_str = npm.to_string_lossy().to_string();
    log_status(&format!("global_npm_root: npm={}", npm_str));

    // Windows 下 npm 可能是 .cmd 脚本，需要通过 cmd /c 调用
    let mut cmd;
    let is_cmd = npm_str.to_lowercase().ends_with(".cmd");
    if is_cmd {
        cmd = Command::new("cmd.exe");
        cmd.arg("/c").arg(&npm_str).arg("root").arg("-g");
    } else {
        cmd = Command::new(&npm);
        cmd.args(["root", "-g"]);
    }
    cmd.env("PATH", injected_path());
    crate::node_manager::apply_no_window(&mut cmd);

    let out = cmd.output().ok()?;
    log_status(&format!(
        "global_npm_root: status={} stdout={} stderr={}",
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim(),
        String::from_utf8_lossy(&out.stderr).trim()
    ));

    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(PathBuf::from(s)) }
}

/// 检查指定 npm 根目录下是否安装了某个包（通过 package.json 是否存在判断）
fn pkg_exists(root: &PathBuf, scope: &str, name: &str) -> bool {
    root.join(scope).join(name).join("package.json").exists()
}

/// 从 package.json 中读取包的版本号
fn read_pkg_version(pkg_path: &PathBuf) -> Option<String> {
    fs::read_to_string(pkg_path).ok().and_then(|content| {
        serde_json::from_str::<serde_json::Value>(&content)
            .ok()
            .and_then(|json| {
                json.get("version").and_then(|v| v.as_str()).map(|s| s.to_string())
            })
    })
}

/// 获取所有已安装的 npm 包候选根目录（按优先级排序）
/// 优先全局 npm 根，其次应用内 runtime/node_modules
fn installed_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(g) = global_npm_root() {
        roots.push(g);
    }
    roots.push(piweb_dir().join("node_modules"));
    roots
}

/// 读取某个 npm 包当前实际安装的版本号
/// 按优先级在所有候选根目录中查找，返回第一个找到的版本
fn read_current_pkg_version(scope: &str, name: &str) -> Option<String> {
    installed_roots()
        .iter()
        .find_map(|root| read_pkg_version(&root.join(scope).join(name).join("package.json")))
}

/// 检查 pi-web 是否已安装（前端调用）
#[tauri::command]
pub fn is_piweb_installed() -> bool {
    resolve_piweb().is_some()
}

/// 综合环境检测：检查 Node.js、Git、pi-web、pi-coding-agent 的安装状态
/// 返回完整的环境检测结果，供前端展示安装向导
#[tauri::command]
pub fn check_environment() -> crate::node_manager::EnvCheckResult {
    use crate::node_manager::{detect_git, detect_node, GitSource, NodeSource};
    let (src, path, ver, meets) = detect_node();
    let (gsrc, _gexe) = detect_git();

    // 检查 pi-web 和 pi-coding-agent 是否已安装（全局 + runtime 两处都查）
    let runtime_dir = piweb_dir();
    let global_dir = global_npm_root();
    let runtime_nm = runtime_dir.join("node_modules");
    let pi_web = pkg_exists(&runtime_nm, "@agegr", "pi-web")
        || global_dir.as_ref().map(|g| pkg_exists(g, "@agegr", "pi-web")).unwrap_or(false);
    let pi_agent = pkg_exists(&runtime_nm, "@earendil-works", "pi-coding-agent")
        || global_dir.as_ref().map(|g| pkg_exists(g, "@earendil-works", "pi-coding-agent")).unwrap_or(false);
    let git_ok = gsrc != GitSource::Missing;
    // 只要有一项不满足就标记为需要安装
    let needs_install = src == NodeSource::Missing || !meets || !git_ok || !pi_web || !pi_agent;
    crate::node_manager::EnvCheckResult {
        node_source: src,
        node_path: path,
        node_version: ver,
        node_meets_req: meets,
        git_installed: git_ok,
        git_source: gsrc,
        pi_web_installed: pi_web,
        pi_coding_agent_installed: pi_agent,
        needs_install,
    }
}

/// 获取当前已安装的 pi-web 和 pi-coding-agent 版本号（前端调用）
#[tauri::command]
pub fn get_installed_versions() -> Versions {
    let roots = installed_roots();
    Versions {
        pi_web: roots.iter().find_map(|root| {
            read_pkg_version(&root.join("@agegr").join("pi-web").join("package.json"))
        }),
        pi_coding_agent: roots.iter().find_map(|root| {
            read_pkg_version(
                &root.join("@earendil-works").join("pi-coding-agent").join("package.json"),
            )
        }),
    }
}

/// 确保 runtime 目录存在并初始化 package.json
/// 自托管安装模式需要一个有效的 npm 项目目录
fn ensure_runtime_dir() -> std::io::Result<()> {
    let dir = piweb_dir();
    if !dir.exists() { fs::create_dir_all(&dir)?; }
    let pkg_file = dir.join("package.json");
    // 如果 package.json 不存在，创建一个最小化的私有包配置
    if !pkg_file.exists() {
        let pkg = serde_json::json!({
            "name": "pi-desktop-runtime",
            "version": "1.0.0",
            "private": true,
            "description": "Pi Desktop runtime dependencies"
        });
        let mut f = fs::File::create(pkg_file)?;
        f.write_all(serde_json::to_string_pretty(&pkg)?.as_bytes())?;
    }
    Ok(())
}

/// 获取 npm registry 地址（用户配置了镜像则用镜像，未配置走官方）
fn npm_registry() -> Option<String> {
    use crate::settings::AppSettings;
    let s = AppSettings::load();
    s.npm_mirror.filter(|m| !m.is_empty())
}

/// 当前 npm 源描述（用于更新记录/日志）
fn registry_label() -> String {
    match npm_registry() {
        Some(r) => format!("镜像 {}", r),
        None => "官方源 registry.npmjs.org".into(),
    }
}

/// 在 runtime 目录中执行 npm 命令
fn run_npm(args: &[&str]) -> Result<String, String> {
    run_npm_in(&piweb_dir(), args)
}

/// 在指定目录中执行 npm 命令（带镜像配置、PATH 注入、无窗口模式）
/// 成功返回 stdout，失败返回错误信息（取 stderr 最后 5 行）
fn run_npm_in(dir: &Path, args: &[&str]) -> Result<String, String> {
    let npm = resolve_npm().ok_or_else(|| "npm 未找到，请确认 Node.js 安装完整".to_string())?;

    // Windows 下 npm 可能是 .cmd 脚本，需要通过 cmd /c 调用
    let mut cmd;
    let npm_str = npm.to_string_lossy().to_string();
    let is_cmd = npm_str.to_lowercase().ends_with(".cmd");
    if is_cmd {
        cmd = Command::new("cmd.exe");
        cmd.arg("/c").arg(&npm_str);
        for a in args { cmd.arg(a); }
    } else {
        cmd = Command::new(&npm);
        cmd.args(args);
    }

    // 禁用进度条/审计/赞助提示，减少输出噪音；注入自定义 PATH
    cmd.current_dir(dir)
        .env("npm_config_progress", "false")
        .env("npm_config_audit", "false")
        .env("npm_config_fund", "false")
        .env("PATH", injected_path());

    // 如果配置了 npm 镜像，通过 --registry 参数指定（不修改全局 npmrc，不污染用户环境）
    if let Some(reg) = npm_registry() {
        cmd.arg("--registry").arg(&reg);
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::node_manager::apply_no_window(&mut cmd);

    match cmd.output() {
        Ok(output) => {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            } else {
                // 失败时取 stderr 最后 5 行作为错误摘要，避免信息过长
                let err = String::from_utf8_lossy(&output.stderr).to_string();
                let tail: String = err.lines().rev().take(5).collect::<Vec<_>>().join("\n");
                Err(format!("npm {} 失败: {}", args.join(" "), tail))
            }
        }
        Err(e) => Err(format!("无法运行 npm: {}", e)),
    }
}

/// 检查是否存在可用的全局 npm（全局安装模式的前提条件）
fn has_global_npm() -> bool {
    global_npm_root().is_some()
}

/// 备份指定包（用于更新失败回滚）
/// 返回备份目录路径
fn backup_pkg(scope: &str, name: &str) -> Option<PathBuf> {
    // 找到包的实际位置
    let mut pkg_path: Option<PathBuf> = None;
    if let Some(g) = global_npm_root() {
        let p = g.join(scope).join(name);
        if p.exists() { pkg_path = Some(p); }
    }
    if pkg_path.is_none() {
        let p = piweb_dir().join("node_modules").join(scope).join(name);
        if p.exists() { pkg_path = Some(p); }
    }
    let src = pkg_path?;

    // 备份到 tmp/backup/<name>-<timestamp>
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup_dir = get_data_dir().join("tmp").join("backup").join(format!("{}-{}", name, ts));
    let _ = fs::create_dir_all(&backup_dir);

    // 复制整个目录
    if copy_dir_all(&src, &backup_dir.join(name)).is_ok() {
        // 记录原始路径以便回滚
        let _ = fs::write(backup_dir.join("_original_path.txt"), src.to_string_lossy().as_bytes());
        Some(backup_dir)
    } else {
        let _ = fs::remove_dir_all(&backup_dir);
        None
    }
}

/// 从备份恢复
fn restore_from_backup(backup_dir: &PathBuf) -> bool {
    let marker = backup_dir.join("_original_path.txt");
    let orig_path = match fs::read_to_string(&marker) {
        Ok(p) => PathBuf::from(p.trim()),
        Err(_) => return false,
    };
    let src = backup_dir.join(orig_path.file_name().unwrap_or_default());
    if !src.exists() { return false; }

    // 先移除旧的
    if orig_path.exists() {
        let _ = fs::remove_dir_all(&orig_path);
    }
    if let Some(parent) = orig_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    copy_dir_all(&src, &orig_path).is_ok()
}

/// 递归复制整个目录（用于备份和回滚）
/// 目标目录已存在时会先删除再复制
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.exists() {
        let _ = fs::remove_dir_all(dst);
    }
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

/// 安装或更新一个 npm 包
/// 优先全局安装（有全局 npm 时），否则安装到应用内 runtime 目录
/// `--omit=dev` 跳过开发依赖，减少安装体积
fn install_or_update_pkg(pkg: &str) -> Result<String, String> {
    if has_global_npm() {
        run_npm_in(&get_data_dir(), &["install", "-g", pkg, "--omit=dev"])
    } else {
        ensure_runtime_dir().map_err(|e| format!("创建目录失败: {}", e))?;
        run_npm(&["install", pkg, "--omit=dev"])
    }
}

/// 发送安装/更新进度事件。
/// step = 当前第几步（1 起），total_steps = 总步数（用于“1/3”式提示）
fn emit_install_progress<R: Runtime>(
    app: &AppHandle<R>,
    target: &str,
    stage: &str,
    percent: f64,
    message: &str,
    step: u32,
    total_steps: u32,
) {
    let payload = serde_json::json!({
        "target": target,
        "stage": stage,
        "percent": percent,
        "message": message,
        "step": step,
        "total_steps": total_steps,
    });
    let _ = app.emit("install-progress", payload);
}

/// pi-web 安装的内部实现（同步执行，在后台线程中调用）
/// 安装流程：安装 pi-web → 安装 pi-coding-agent → 完成，共 3 步进度
fn install_piweb_inner<R: Runtime>(app: &AppHandle<R>) -> InstallResult {
    // 安装 Pi Web 分 3 步：安装 pi-web → 安装 pi-coding-agent → 完成
    emit_update_log(app, "pi-web", "开始安装 Pi Web / Pi Coding Agent");
    emit_update_log(app, "pi-web", &format!("步骤 1/3：npm install -g @agegr/pi-web@latest（{}）", registry_label()));
    emit_install_progress(app, "pi-web", "installing", 8.0, "正在安装 Pi Web...", 1, 3);

    match install_or_update_pkg("@agegr/pi-web@latest") {
        Ok(_) => match install_or_update_pkg("@earendil-works/pi-coding-agent@latest") {
            Ok(_) => {
                let versions = get_installed_versions();
                emit_update_log(app, "pi-web", &format!(
                    "✅ 安装完成：pi-web v{} / pi-coding-agent v{}",
                    versions.pi_web.clone().unwrap_or_default(),
                    versions.pi_coding_agent.clone().unwrap_or_default()
                ));
                emit_install_progress(app, "pi-web", "done", 100.0, "安装完成", 3, 3);
                log_status(&format!("install_piweb OK: {:?}", versions));
                InstallResult {
                    success: true,
                    message: format!(
                        "Pi Web 安装成功 (v{})",
                        versions.pi_web.unwrap_or_default()
                    ),
                }
            }
            Err(e) => {
                let err: String = e.chars().take(300).collect();
                log_status(&format!("install_piweb ERR (agent): {}", e));
                emit_update_log(app, "pi-web", &format!("安装 Pi Coding Agent 失败：{}", err));
                emit_install_progress(app, "pi-web", "error", 0.0, &format!("安装 Pi Coding Agent 失败: {}", err), 2, 3);
                InstallResult { success: false, message: format!("安装 Pi Coding Agent 失败: {}", e) }
            }
        },
        Err(e) => {
            let err: String = e.chars().take(300).collect();
            log_status(&format!("install_piweb ERR: {}", e));
            emit_update_log(app, "pi-web", &format!("安装失败：{}", err));
            emit_install_progress(app, "pi-web", "error", 0.0, &format!("安装失败: {}", err), 1, 3);
            InstallResult { success: false, message: e }
        }
    }
}

/// 安装 pi-web 和 pi-coding-agent（前端命令入口）
#[tauri::command]
pub fn install_piweb(app: tauri::AppHandle) -> InstallResult {
    install_piweb_inner(&app)
}

// ==================== 重启 pi-web（停止后再启动） ====================

/// 重启 Pi Web 服务：先停止旧进程，等端口释放，再启动新进程。
/// 用于用户手动点击「重启」按钮。
#[tauri::command]
pub fn restart_piweb(state: State<AppState>, app: tauri::AppHandle) -> Result<String, String> {
    log_status("restart_piweb called");
    // 先停止（不管当前是否在运行）
    let was_running = stop_all(&state);
    log_status(&format!("restart: stopped, was_running={}", was_running));
    // 等一下端口释放
    std::thread::sleep(Duration::from_secs(2));
    // 再启动
    let url = start_piweb(state, app)?;
    log_status(&format!("restart: started at {}", url));
    Ok(url)
}

// ==================== pi-web 启动与监控（异常退出自动重启） ====================

/// pi-web 进程监控标志
static PIWEB_MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);

/// 组装 NODE_OPTIONS：把 telegram 代理引导注入进去（仅在未注入过时追加一次）
fn compose_node_options() -> String {
    let prepared = proxy_init_path();
    let mut node_options = std::env::var("NODE_OPTIONS").unwrap_or_default();
    // 父环境若残留旧版注入引用（可能指向已删除路径），整体丢弃，避免子 node 因文件不存在启动失败
    if node_options.contains("telegram-proxy-init") {
        node_options.clear();
    }
    if let Some(init_path) = prepared {
        if !node_options.contains("telegram-proxy-init") {
            if !node_options.is_empty() {
                node_options.push(' ');
            }
            node_options.push_str("--require \"");
            node_options.push_str(&init_path.replace('\\', "/"));
            node_options.push('"');
        }
    }
    node_options
}

/// 组装 pi-web 子进程启动命令：统一注入 PATH / NODE_OPTIONS 并把输出重定向到 logs。
fn build_piweb_cmd(
    node: &Path,
    bin: &Path,
    run_dir: &Path,
    port: u16,
    node_options: &str,
) -> Command {
    let mut cmd = Command::new(node);
    cmd.arg(bin)
        .arg("-p")
        .arg(port.to_string())
        .arg("-H")
        .arg("127.0.0.1")
        .arg("--no-open")
        .current_dir(run_dir)
        .env("PI_WEB_HOSTNAME", "127.0.0.1")
        .env("PI_WEB_NO_OPEN", "1")
        .env("PATH", injected_path())
        .env("NODE_OPTIONS", node_options);
    // 子进程 stdout/stderr → logs\pi-web.out.log / logs\pi-web.err.log
    if let Some(f) = open_child_log_file("pi-web.out.log") {
        cmd.stdout(Stdio::from(f));
    }
    if let Some(f) = open_child_log_file("pi-web.err.log") {
        cmd.stderr(Stdio::from(f));
    }
    crate::node_manager::apply_no_window(&mut cmd);
    cmd
}

/// 启动 pi-web 子进程并等待端口就绪；超时统一 kill+wait 回收，避免遗留孤儿 node 进程。
fn spawn_piweb(
    node: &Path,
    bin: &Path,
    run_dir: &Path,
    port: u16,
    wait_secs: u64,
) -> Result<Child, String> {
    let node_options = compose_node_options();
    log_status(&format!("NODE_OPTIONS: {}", node_options));

    let mut cmd = build_piweb_cmd(node, bin, run_dir, port, &node_options);
    let child = cmd.spawn().map_err(|e| {
        log_status(&format!("ERROR: spawn failed: {}", e));
        format!("启动 Pi Web 失败: {}", e)
    })?;

    if wait_until_ready(port, wait_secs) {
        Ok(child)
    } else {
        // 启动失败：回收刚启动的进程，避免留下占端口的孤儿 node
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
        log_status("ERROR: pi-web startup timeout");
        Err("Pi Web 启动超时".into())
    }
}

/// 在指定端口启动 pi-web 并登记到 AppState（成功后拉起监控线程）。
fn launch_piweb(
    state: &AppState,
    app: &tauri::AppHandle,
    port: u16,
    wait_secs: u64,
) -> Result<(), String> {
    let node = resolve_node().ok_or_else(|| {
        log_status("ERROR: node not found");
        "Node.js 未找到".to_string()
    })?;
    let (bin, run_dir) = resolve_piweb().ok_or_else(|| {
        log_status("ERROR: pi-web not installed");
        "Pi Web 尚未安装，请先安装".to_string()
    })?;
    log_status(&format!(
        "spawning node {:?} with bin {:?} (cwd {:?}) on port {}",
        node, bin, run_dir, port
    ));

    let child = spawn_piweb(&node, &bin, &run_dir, port, wait_secs)?;

    // 登记状态：先子进程句柄、再端口（独立加锁，与 stop_all / monitor 锁序一致）
    *state.piweb_child.lock().unwrap() = Some(child);
    *state.piweb_port.lock().unwrap() = Some(port);
    log_status(&format!("OK: listening on port {}", port));

    // 启动监控线程（仅启动一次）
    start_monitor(app.clone());
    Ok(())
}

#[tauri::command]
pub fn start_piweb(state: State<AppState>, app: tauri::AppHandle) -> Result<String, String> {
    log_status("start called");
    {
        let port_guard = state.piweb_port.lock().unwrap();
        if let Some(p) = *port_guard {
            return Ok(format!("http://127.0.0.1:{}", p));
        }
    }

    let port = find_port_or_random(31141).ok_or_else(|| "找不到空闲端口".to_string())?;
    launch_piweb(&state, &app, port, 90)?;

    Ok(format!("http://127.0.0.1:{}", port))
}

/// 启动 pi-web 进程监控：异常退出时自动重启
fn start_monitor(app: AppHandle) {
    if PIWEB_MONITOR_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return; // 已经在运行
    }

    std::thread::spawn(move || {
        log_status("pi-web monitor started");
        loop {
            std::thread::sleep(Duration::from_secs(3));

            // 获取 AppState
            let state_opt = app.try_state::<AppState>();
            let state = match state_opt {
                Some(s) => s,
                None => continue,
            };

            // 检查子进程是否还活着
            let mut restart_needed = false;
            let mut port_to_use: Option<u16> = None;
            {
                let mut child_guard = state.piweb_child.lock().unwrap();
                let port_guard = state.piweb_port.lock().unwrap();

                if let Some(child) = child_guard.as_mut() {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            // 进程退出了
                            log_status(&format!("pi-web exited unexpectedly: {}", status));
                            port_to_use = *port_guard;
                            restart_needed = true;
                            // 清空状态
                            child_guard.take();
                        }
                        Ok(None) => {
                            // 还在运行
                        }
                        Err(e) => {
                            log_status(&format!("pi-web monitor wait error: {}", e));
                        }
                    }
                } else {
                    // 没有子进程（可能还没启动或已被停止），继续等
                }
            }

            if restart_needed {
                log_status("attempting to restart pi-web...");

                // 重启前复查：已被手动 restart 流程接管（已登记新进程或已被主动停止）则跳过
                {
                    let child_guard = state.piweb_child.lock().unwrap();
                    let port_guard = state.piweb_port.lock().unwrap();
                    if child_guard.is_some() || port_guard.is_none() {
                        log_status("pi-web restart skipped: taken over or stopped");
                        continue;
                    }
                }

                // 端口：优先复用原端口，被占用则自动换空闲端口
                let preferred = port_to_use.unwrap_or(31141);
                let Some(port) = find_port_or_random(preferred) else {
                    log_status("pi-web restart failed (no free port)");
                    *state.piweb_port.lock().unwrap() = None;
                    continue;
                };

                let (Some(node), Some((bin_path, run_dir))) = (resolve_node(), resolve_piweb())
                else {
                    log_status("pi-web restart failed (node / pi-web missing)");
                    *state.piweb_port.lock().unwrap() = None;
                    continue;
                };

                // 等一下端口释放
                std::thread::sleep(Duration::from_secs(2));

                // 再次复查：等待期间可能已被手动启动/停止接管
                {
                    let child_guard = state.piweb_child.lock().unwrap();
                    let port_guard = state.piweb_port.lock().unwrap();
                    if child_guard.is_some() || port_guard.is_none() {
                        log_status("pi-web restart skipped: state changed during wait");
                        continue;
                    }
                }

                match spawn_piweb(&node, &bin_path, &run_dir, port, 60) {
                    Ok(child) => {
                        let mut child_guard = state.piweb_child.lock().unwrap();
                        *child_guard = Some(child);
                        *state.piweb_port.lock().unwrap() = Some(port);
                        log_status("pi-web restarted successfully");
                        // 通知前端刷新（URL 使用实际端口）
                        let _ = app.emit("piweb-restarted", serde_json::json!({
                            "url": format!("http://127.0.0.1:{}", port)
                        }));
                    }
                    Err(e) => {
                        log_status(&format!("pi-web restart failed: {}", e));
                        let mut child_guard = state.piweb_child.lock().unwrap();
                        child_guard.take();
                        *state.piweb_port.lock().unwrap() = None;
                    }
                }
            }
        }
    });
}

/// 写入启动/运行状态日志（logs/start-status.log）
/// 用于排查 pi-web 启动、停止、重启等生命周期问题
fn log_status(msg: &str) {
    use std::fs::OpenOptions;
    use std::io::Write as _;
    let path = log_dir().join("start-status.log");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {}", chrono_now(), msg);
    }
}

/// 统一日志目录：<安装目录>\logs\（运行状态 / 更新记录 / 子进程输出都放这里）
/// 统一日志目录：<安装目录>/logs/（运行状态 / 更新记录 / 子进程输出都放这里）
fn log_dir() -> PathBuf {
    let d = get_data_dir().join("logs");
    let _ = fs::create_dir_all(&d);
    d
}

/// 打开一个日志文件句柄（供 pi-web 子进程 stdout/stderr 重定向，如 logs\pi-web.out.log）
/// 打开一个日志文件句柄（供 pi-web 子进程 stdout/stderr 重定向，如 logs/pi-web.out.log）
fn open_child_log_file(filename: &str) -> Option<fs::File> {
    use std::fs::OpenOptions;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir().join(filename))
        .ok()
}

/// 生成简易时间戳（Unix 秒数格式，用于日志前缀）
fn chrono_now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("[{}s]", d.as_secs())
}

/// 追加一行到更新记录文件 <安装目录>\logs\update.log（pi / pi-web 每次更新明细）
/// 追加一行到更新记录文件 <安装目录>/logs/update.log（pi / pi-web 每次更新明细）
fn append_update_log(line: &str) {
    use std::fs::OpenOptions;
    use std::io::Write as _;
    let path = log_dir().join("update.log");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {}", chrono_now(), line);
    }
}

/// 更新/安装运行记录：写入 update.log + 广播给前端浮层实时打印
/// 更新/安装运行记录：写入 update.log + 广播给前端浮层实时打印
fn emit_update_log<R: Runtime>(app: &AppHandle<R>, target: &str, line: &str) {
    append_update_log(&format!("[{}] {}", target, line));
    let _ = app.emit(
        "update-log",
        serde_json::json!({
            "target": target,
            "line": line,
        }),
    );
}

#[tauri::command]
pub fn stop_piweb(state: State<AppState>) -> bool {
    // 标记退出意图：ask 模式点「退出应用」时放行 CloseRequested（见 main.rs），
    // 避免确认弹窗与窗口关闭事件互相触发导致无法退出
    state.quit_intent.store(true, Ordering::SeqCst);
    stop_all(&state)
}

pub fn stop_all(state: &AppState) -> bool {
    let mut child_guard = state.piweb_child.lock().unwrap();
    let was_running = child_guard.is_some();
    if let Some(mut c) = child_guard.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    *state.piweb_port.lock().unwrap() = None;
    was_running
}

#[tauri::command]
pub fn get_piweb_url(state: State<AppState>) -> Option<String> {
    state.piweb_port.lock().unwrap().map(|p| format!("http://127.0.0.1:{}", p))
}

/// 托盘 / 设置面板共用的后台更新流程（新线程执行）：
/// 顺序：先备份（pi-web 运行中即可读）→ 停止 pi-web（释放运行时更新锁）→ npm 更新（失败回滚）
/// → 立即广播结果 → 自动重启服务 → 重启完成再广播
pub fn perform_update(app: tauri::AppHandle, target: String) {
    std::thread::spawn(move || {
        let (label, scope, name, spec): (&str, &str, &str, &str) = match target.as_str() {
            "pi-web" | "web" => {
                ("Pi Web", "@agegr", "pi-web", "@agegr/pi-web@latest")
            }
            "pi-coding-agent" | "agent" => (
                "Pi Coding Agent",
                "@earendil-works",
                "pi-coding-agent",
                "@earendil-works/pi-coding-agent@latest",
            ),
            _ => {
                let _ = app.emit(
                    "pi-update-result",
                    serde_json::json!({
                        "ok": false,
                        "message": format!("未知更新目标: {}", target),
                        "target": target,
                        "pending_restart": false,
                    }),
                );
                return;
            }
        };
        let ev_target: &'static str = if spec.starts_with("@agegr") { "pi-web" } else { "pi-coding-agent" };
        let state = app.state::<AppState>();

        // 步骤 1/3：先备份（仍在运行，目录可读；若因锁失败则跳过继续）
        let old_ver = read_current_pkg_version(scope, name).unwrap_or_else(|| "未知".into());
        emit_update_log(&app, ev_target, &format!("开始更新 {}（当前 v{}）", label, old_ver));
        emit_install_progress(&app, ev_target, "updating", 5.0, &format!("正在备份 {} 当前版本...", label), 1, 3);
        let backup = backup_pkg(scope, name);
        emit_update_log(&app, ev_target, if backup.is_some() {
            "步骤 1/3：备份完成"
        } else {
            "步骤 1/3：备份失败（跳过，继续尝试更新）"
        });

        // 步骤 2/3：停止 pi-web（释放运行时更新锁）后再安装
        emit_update_log(&app, ev_target, "正在停止 Pi Web 服务（释放运行时更新锁）");
        let was_running = stop_all(&state);
        emit_update_log(&app, ev_target, if was_running {
            "Pi Web 服务已停止"
        } else {
            "Pi Web 服务未在运行"
        });
        emit_update_log(&app, ev_target, "步骤 2/3：安装最新版");
        emit_update_log(&app, ev_target, &format!("npm install -g {} --omit=dev（{}）", spec, registry_label()));
        emit_install_progress(&app, ev_target, "updating", 15.0, &format!("正在更新 {}...", label), 2, 3);

        // 步骤 3/3：安装 / 失败回滚
        let result = match install_or_update_pkg(spec) {
            Ok(_) => {
                let new_ver = read_current_pkg_version(scope, name).unwrap_or_else(|| "未知".into());
                emit_update_log(&app, ev_target, &format!("✅ {} 更新成功：v{} → v{}", label, old_ver, new_ver));
                emit_update_log(&app, ev_target, "步骤 3/3：完成，清理备份");
                if let Some(b) = backup {
                    let _ = fs::remove_dir_all(&b);
                }
                emit_install_progress(&app, ev_target, "done", 100.0, "更新成功", 3, 3);
                InstallResult {
                    success: true,
                    message: format!("{} 更新成功 (v{})", label, new_ver),
                }
            }
            Err(e) => {
                let err: String = e.chars().take(300).collect();
                emit_update_log(&app, ev_target, &format!("步骤 3/3：更新失败（{}）", err));
                emit_install_progress(&app, ev_target, "error", 0.0, "更新失败，正在回滚...", 3, 3);
                if let Some(b) = backup {
                    if restore_from_backup(&b) {
                        let restored =
                            read_current_pkg_version(scope, name).unwrap_or_else(|| "未知".into());
                        emit_update_log(&app, ev_target, &format!("已回滚到之前版本 v{}", restored));
                        // 回滚完成后也清理备份
                        let _ = fs::remove_dir_all(&b);
                        emit_update_log(&app, ev_target, "备份已清理");
                        emit_install_progress(
                            &app, ev_target, "rolled_back", 100.0,
                            "更新失败，已恢复到之前版本", 3, 3,
                        );
                        InstallResult {
                            success: false,
                            message: format!("更新失败，已回滚到之前版本: {}", e),
                        }
                    } else {
                        emit_update_log(&app, ev_target, "回滚失败，请手动重新安装该组件");
                        InstallResult { success: false, message: e }
                    }
                } else {
                    InstallResult { success: false, message: e }
                }
            }
        };

        // 立即广播更新结果（不等重启）——修复“进度条走完很久才弹成功”
        // 更新成功但服务没在跑时也自动拉起（更新完成自动恢复启动）
        let restart = was_running || result.success;
        let _ = app.emit(
            "pi-update-result",
            serde_json::json!({
                "ok": result.success,
                "message": result.message,
                "target": ev_target,
                "pending_restart": restart,
            }),
        );

        if restart {
            // 无论成败都恢复服务（失败已回滚 → 重启后仍是可用旧版本）
            emit_update_log(&app, ev_target, "正在重启 Pi Web 服务…");
            match start_piweb(state, app.clone()) {
                Ok(url) => {
                    emit_update_log(&app, ev_target, &format!("✅ Pi Web 服务已重启：{}", url));
                    let _ = app.emit(
                        "pi-update-restarted",
                        serde_json::json!({ "ok": true, "url": url, "target": ev_target }),
                    );
                }
                Err(e) => {
                    emit_update_log(&app, ev_target, &format!("❌ Pi Web 服务重启失败：{}", e));
                    let _ = app.emit(
                        "pi-update-restarted",
                        serde_json::json!({ "ok": false, "message": e, "target": ev_target }),
                    );
                }
            }
        }
    });
}

/// 设置面板“更新”按钮触发（与托盘走同一条后台流程）
#[tauri::command]
pub fn update_component(app: tauri::AppHandle, target: String) {
    perform_update(app, target);
}

/// 查找系统分配的一个空闲 TCP 端口（绑定 0 端口由系统分配）
fn find_free_port() -> Option<u16> {
    match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => {
            let port = listener.local_addr().ok().map(|a| a.port());
            drop(listener);
            port
        }
        Err(_) => None,
    }
}

/// 优先使用指定端口，若被占用则返回一个随机空闲端口
fn find_port_or_random(preferred: u16) -> Option<u16> {
    if TcpListener::bind(("127.0.0.1", preferred)).is_ok() {
        return Some(preferred);
    }
    find_free_port()
}

/// 代理引导文件路径：应用实际安装目录（exe 所在目录 = `get_data_dir()`）。
/// 该文件在安装/覆盖安装时随安装包放好（见 tauri.conf.json `bundle.resources`；
/// NSIS PREINSTALL 会整目录删除后全新安装，由安装器把文件放回安装根），
/// 此处做路径定位并确认存在，若文件缺失（如 dev 模式未走安装器）时返回 None，调用方跳过注入。
fn proxy_init_path() -> Option<String> {
    let target = get_data_dir().join("telegram-proxy-init.cjs");
    if target.exists() {
        Some(target.to_string_lossy().to_string())
    } else {
        None
    }
}

/// 轮询检测指定端口的 TCP 服务是否就绪
/// 每 500ms 尝试连接一次，最长等待 max_seconds 秒
fn wait_until_ready(port: u16, max_seconds: u64) -> bool {
    for _ in 0..max_seconds * 2 {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", port).parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

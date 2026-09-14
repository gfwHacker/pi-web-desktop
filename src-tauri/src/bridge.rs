//! pi-web 注入桥：让壳层感知 pi-web 内部状态（主题同步 / 「完整历史」弹窗）。
//!
//! 原理：pi-web 跑在主窗口 WebView 的 iframe 里（跨域，JS 无法直接互访）。
//! 通过在 WebView2 上注册 `AddScriptToExecuteOnDocumentCreated`，把一段桥脚本注入到
//! 该 WebView **之后创建的所有文档（含 iframe）**——脚本：
//!   1. 监听 pi-web `<html data-theme="xxx">` 变化 → postMessage 主题给壳层标题栏；
//!      （支持 light / dark / mist / pine / rose 五种主题，与 pi-web 最新外观同步）
//!   2. 拦截「完整历史」按钮的 `window.open('/api/sessions/.../export?inline=1')`
//!      → postMessage 通知壳层用独立窗口打开（避免 WebView2 静默拦截 window.open）。
//! 全程零修改 pi-web 页面/源码。

/// 注入给 pi-web iframe 的桥脚本（只在子 iframe + http(s) 文档里生效）
pub const BRIDGE_JS: &str = r#"/* pi-desktop bridge (auto-injected) */
(function () {
  var tag = '__piDesktopBridgeInjected';
  try { if (window[tag]) return; window[tag] = true; } catch (e) {}
  var isTop = false, isHttp = false;
  try { isTop = window.self === window.top; isHttp = /^https?:/.test(location.protocol); } catch (e) {}
  if (isTop || !isHttp) return; // 只处理壳内 iframe（pi-web）
  function send(t, d) {
    try { window.parent.postMessage({ __piDesktopBridge: true, type: t, data: d }, "*"); } catch (e) {}
  }
  // 1) window.open：导出 /api/sessions/.../export?inline=1 交给壳层开应用内窗口；
  //    其它外部 http(s) 链接交给壳层用系统浏览器打开（WebView2 不弹新窗，直接 window.open 会被静默拦截）。
  try {
    var realOpen = window.open && window.open.bind(window);
    window.open = function (u, f) {
      try {
        var url = new URL(String(u), location.href);
        if (/^https?:/.test(url.protocol)) {
          var isExport = url.pathname.indexOf("/api/sessions/") === 0 && url.searchParams.get("inline") === "1";
          if (isExport) { send("openUrl", url.href); return null; }
          if (url.origin !== location.origin) { send("openExternal", url.href); return null; }
        }
      } catch (e) {}
      return realOpen ? realOpen(u, f) : null;
    };
  } catch (e) {}
  // 1b) 外部链接（如“添加插件”里的 pi.dev/packages 是 target=_blank 的 <a>）：
  //     捕获点击后交给壳层用系统浏览器打开，避免在 WebView2 iframe 里被静默拦截。
  try {
    document.addEventListener('click', function (ev) {
      try {
        var a = ev.target;
        while (a && a !== document && a.tagName !== 'A') a = a.parentNode;
        if (!a || a === document) return;
        var href = a.getAttribute && a.getAttribute('href');
        if (!href) return;
        var tgt = (a.getAttribute('target') || '').toLowerCase();
        if (tgt !== '_blank' && tgt !== '_top' && tgt !== '_parent') return;
        var u = new URL(href, location.href);
        if (/^https?:/.test(u.protocol) && u.origin !== location.origin) {
          if (ev.preventDefault) ev.preventDefault();
          if (ev.stopPropagation) ev.stopPropagation();
          send('openExternal', u.href);
        }
      } catch (e2) {}
    }, true);
  } catch (e) {}
  // 2) 主题：等 documentElement 就绪再监听 data-theme 属性（pi-web 支持 light/dark/mist/pine/rose 多种主题）
  //    优先读 data-theme，回退到 class="dark" 兼容旧版
  function __getPiWebTheme() {
    var el = document.documentElement;
    if (!el) return "light";
    var t = el.getAttribute && el.getAttribute("data-theme");
    if (t && typeof t === "string" && t.length) return t;
    return el.classList && el.classList.contains("dark") ? "dark" : "light";
  }
  function startTheme() {
    var el = document.documentElement;
    if (!el) return false;
    function push() {
      try { send("theme", __getPiWebTheme()); } catch (e) {}
    }
    try {
      new MutationObserver(push).observe(el, {
        attributes: true,
        attributeFilter: ["data-theme", "class"]
      });
    } catch (e) {}
    push();
    return true;
  }
  if (!startTheme()) {
    var iv = setInterval(function () { if (startTheme()) clearInterval(iv); }, 50);
    setTimeout(function () { clearInterval(iv); }, 8000);
  }
  // 3) 路由变化上报：让壳层知道 pi-web 当前在哪个页面（SPA 内部跳转不会触发 iframe onload）
  //    壳层需要在 pi-web 重启/更新后恢复用户当前页面，而不是永远回到首页
  function reportUrl() {
    try { send("route", location.pathname + location.search + location.hash); } catch (e) {}
    // 同步上报当前工作目录（从 URL 参数 cwd 解析）
    try { reportCwd(); } catch (e) {}
  }

  // 3b) 当前工作目录（cwd）上报：壳层「打开工作目录」按钮据此判断是否可用/打开哪个目录。
  //     信息源（只读、零副作用，不修改 pi-web 行为）：
  //       1) URL ?cwd= 参数（进入工作区时的初始源）；
  //       2) 嗅探 pi-web 自身 fetch 请求 URL 里的 cwd 查询参数——pi-web 每次工作区
  //          操作（git/status、worktrees、models、project-trust…）都携带 ?cwd=，
  //          是运行时工作目录最准确的信号（切目录后立即感知）；
  //       3) 启动时读 /api/sessions 取当前活动会话的 cwd 回填（避免空闲时置灰）。
  //     无工作目录时上报空串，壳层会把按钮置灰。
  var __lastCwd = '';
  function setCwd(cwd) {
    if (typeof cwd !== 'string') cwd = '';
    cwd = cwd.trim();
    if (cwd === __lastCwd) return;
    __lastCwd = cwd;
    try { send('cwd', cwd); } catch (e) {}
  }
  function reportCwd() {
    var cwd = '';
    try {
      cwd = new URLSearchParams(location.search).get('cwd') || '';
    } catch (e) {}
    if (!cwd) {
      try {
        var v = localStorage.getItem('pi-web:last-custom-cwd');
        if (v && typeof v === 'string') cwd = v;
      } catch (e) {}
    }
    setCwd(cwd);
  }
  // 嗅探同源 fetch：请求 URL 里出现 ?cwd= 即当前工作区目录
  var __realFetch = null;
  try {
    __realFetch = window.fetch && window.fetch.bind(window);
    if (__realFetch) {
      window.fetch = function (input, init) {
        try {
          var u = typeof input === 'string' ? input : (input && (input.url || input.href)) || '';
          var qi = u.indexOf('cwd=');
          if (qi >= 0 && u.indexOf('/api/') >= 0) {
            var q = u.indexOf('?') >= 0 ? u.slice(u.indexOf('?') + 1) : u;
            var c = new URLSearchParams(q).get('cwd');
            if (c) setCwd(c);
          }
        } catch (e) {}
        return __realFetch(input, init);
      };
    }
  } catch (e) {}
  // 启动回填：当前活动会话（modified 最新）的 cwd / projectRoot
  try {
    if (__realFetch) {
      __realFetch('/api/sessions').then(function (r) {
        if (!r.ok) return null;
        return r.json();
      }).then(function (j) {
        try {
          var list = (j && j.sessions) || [];
          if (!list.length) { setCwd(''); return; }
          list.sort(function (a, b) { return String(a.modified).localeCompare(String(b.modified)); });
          var c = list[list.length - 1];
          setCwd((c && (c.cwd || c.projectRoot)) || '');
        } catch (e) {}
      }).catch(function () {});
    }
  } catch (e) {}
  // 初始上报
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', function () { setTimeout(reportUrl, 500); });
  } else {
    setTimeout(reportUrl, 800);
  }
  // 监听 History API（SPA 路由）
  try {
    var origPush = history.pushState;
    var origReplace = history.replaceState;
    history.pushState = function () {
      var r = origPush.apply(this, arguments);
      setTimeout(reportUrl, 50);
      return r;
    };
    history.replaceState = function () {
      var r = origReplace.apply(this, arguments);
      setTimeout(reportUrl, 50);
      return r;
    };
    window.addEventListener('popstate', function () { setTimeout(reportUrl, 50); });
  } catch (e) {}
  // hashchange 兜底
  try { window.addEventListener('hashchange', function () { setTimeout(reportUrl, 50); }); } catch (e) {}
})();"#;

/// 把桥脚本注册到主窗口 WebView（Windows WebView2 的 AddScriptToExecuteOnDocumentCreated）。
/// 之后 pi-web iframe 每次新建文档都会自动带上桥脚本。
#[cfg(windows)]
pub fn install_bridge(app: &tauri::AppHandle) {
    use tauri::Manager;
    log_bridge("install_bridge called");
    let Some(win) = app.get_webview_window("main") else {
        log_bridge("main window not found");
        return;
    };
    let js = BRIDGE_JS.to_string();
    let res = win.with_webview(move |pw| {
        log_bridge("with_webview executed (main thread)");
        if let Err(e) = inject_into(&pw, &js) {
            log_bridge(&format!("inject failed: {e}"));
        } else {
            log_bridge("inject OK");
        }
    });
    match res {
        Ok(()) => log_bridge("with_webview queued ok"),
        Err(e) => log_bridge(&format!("with_webview dispatch error: {e}")),
    }
}

/// 桥接模块日志输出：写入 logs/bridge.log，用于排查注入问题
#[cfg(windows)]
fn log_bridge(msg: &str) {
    use std::fs::OpenOptions;
    use std::io::Write as _;
    // 统一日志目录 <应用目录>\logs\：与 start-status.log / update.log / pi-web.*.log 放一起
    // 统一使用应用数据目录下的 logs/ 子目录，与其他日志文件放在一起
    let dir = crate::node_manager::get_data_dir().join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("bridge.log"))
    {
        let _ = writeln!(
            f,
            "[{}] {}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            msg
        );
    }
}

/// 调用 WebView2 COM 接口，将 JS 脚本注册到「文档创建前自动执行」列表
///
/// 这样之后该 WebView 中创建的所有文档（包括 iframe）都会自动注入这段脚本，
/// 实现 pi-web iframe 与壳层之间的跨域通信。
#[cfg(windows)]
fn inject_into(pw: &tauri::webview::PlatformWebview, js: &str) -> Result<(), String> {
    use webview2_com::AddScriptToExecuteOnDocumentCreatedCompletedHandler;
    use windows_core::HSTRING;
    let controller = pw.controller();
    let webview = unsafe { controller.CoreWebView2().map_err(|e| e.to_string())? };
    let js_owned = js.to_string();
    AddScriptToExecuteOnDocumentCreatedCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            let script = HSTRING::from(js_owned.as_str());
            webview
                .AddScriptToExecuteOnDocumentCreated(&script, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(|error_code, _id| error_code),
    )
    .map_err(|e| format!("AddScriptToExecuteOnDocumentCreated: {e:?}"))?;
    Ok(())
}

/// 注入到「完整历史」窗口：默认使用 pi-web 原始字号，不强制放大。
/// 提供右下角浮动字号调节控件（A- / A+ / 复位），偏好存入 localStorage，
/// 下次打开自动恢复，避免「先小后大」的抖动观感。
pub const FONT_JS: &str = r#"(function () {
  var tag = '__piDesktopFontCtrl';
  try { if (window[tag]) return; window[tag] = 1; } catch (e) {}

  var BASE = 1.0;          // 默认原始字号
  var MIN = 0.85, MAX = 1.5;
  var KEY = 'pi-desktop:history-font-scale';
  var last = BASE;
  try { var s = parseFloat(localStorage.getItem(KEY)); if (s >= MIN && s <= MAX) last = s; } catch (e) {}

  function apply(scale) {
    var b = document.body;
    if (!b) return false;
    b.style.zoom = String(scale);
    return true;
  }

  function save() { try { localStorage.setItem(KEY, String(last)); } catch (e) {} }

  function mount() {
    if (document.getElementById('__piDesktopFontBar')) return true;
    var b = document.body;
    if (!b) return false;
    var bar = document.createElement('div');
    bar.id = '__piDesktopFontBar';
    bar.style.cssText = 'position:fixed;right:16px;bottom:16px;z-index:999999;' +
      'display:flex;align-items:center;gap:4px;padding:4px 6px;' +
      'background:rgba(24,26,30,0.92);border:1px solid rgba(255,255,255,0.14);' +
      'border-radius:8px;box-shadow:0 4px 16px rgba(0,0,0,0.35);' +
      'font-family:Segoe UI,system-ui,sans-serif;user-select:none;';
    function mkBtn(txt, title, fn) {
      var s = document.createElement('button');
      s.textContent = txt;
      s.title = title;
      s.style.cssText = 'min-width:28px;height:26px;border:none;border-radius:6px;' +
        'background:rgba(255,255,255,0.12);color:#fff;font-size:13px;cursor:pointer;line-height:1;';
      s.onclick = function (e) { e.stopPropagation(); fn(); };
      return s;
    }
    function mkLabel() {
      var l = document.createElement('span');
      l.id = '__piDesktopFontVal';
      l.style.cssText = 'color:rgba(255,255,255,0.75);font-size:11px;padding:0 4px;min-width:30px;text-align:center;';
      return l;
    }
    var lbl = mkLabel();
    var minus = mkBtn('A-', '缩小字号', function () {
      last = Math.max(MIN, Math.round((last - 0.05) * 100) / 100);
      apply(last); save(); lbl.textContent = Math.round(last * 100) + '%';
    });
    var reset = mkBtn('100%', '恢复默认字号', function () {
      last = BASE; apply(last); save(); lbl.textContent = '100%';
    });
    var plus = mkBtn('A+', '放大字号', function () {
      last = Math.min(MAX, Math.round((last + 0.05) * 100) / 100);
      apply(last); save(); lbl.textContent = Math.round(last * 100) + '%';
    });
    bar.appendChild(minus);
    bar.appendChild(reset);
    bar.appendChild(plus);
    bar.appendChild(lbl);
    b.appendChild(bar);
    lbl.textContent = Math.round(last * 100) + '%';
    return true;
  }

  // 先应用字号（避免抖动），再挂载控件
  if (!apply(last)) {
    var iv = setInterval(function () { if (apply(last)) { clearInterval(iv); mount(); } }, 80);
    setTimeout(function () { clearInterval(iv); }, 8000);
  } else {
    var iv2 = setInterval(function () { if (mount()) clearInterval(iv2); }, 100);
    setTimeout(function () { clearInterval(iv2); }, 8000);
  }
})();"#;

/// 页面加载完成后同步执行 FONT_JS 脚本（历史窗口的字号调节控件注入）
///
/// 在窗口 builder 的 `on_page_load` 回调中调用，确保每次导航到新页面都会重新注入。
#[cfg(windows)]
fn inject_page_script(pw: &tauri::webview::PlatformWebview) {
    use webview2_com::ExecuteScriptCompletedHandler;
    use windows_core::HSTRING;
    unsafe {
        if let Ok(webview) = pw.controller().CoreWebView2() {
            let js = HSTRING::from(FONT_JS);
            let handler = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
            match webview.ExecuteScript(&js, &handler) {
                Ok(_) => log_bridge("font: font-control injected"),
                Err(e) => log_bridge(&format!("font: ExecuteScript err {e}")),
            }
        } else {
            log_bridge("font: CoreWebView2() failed");
        }
    }
}

/// 打开「完整历史 / 导出」：应用内新窗口加载该文档。初始与主窗口同尺寸、覆盖同一位置，
/// 之后可自由拖动/缩放（不强制定位跟随）。由壳层前端在收到桥的 openUrl 消息时调用。
///
/// 注意：Windows 上在**同步 command**里创建窗口会卡住（Tauri 已知问题），
/// 因此这里用 async + 后台线程创建；创建时先隐藏、定位到主窗口几何后再显示，避免偏移闪烁。
#[tauri::command]
pub async fn open_history_window(app: tauri::AppHandle, url: String) -> Result<(), String> {
    tauri::async_runtime::spawn(async move {
        use tauri::{Manager, Position, Size, WebviewUrl, WebviewWindowBuilder};
        let parsed: tauri::Url = url.parse().map_err(|e| format!("bad url: {e}"))?;

        // 以主窗口当前几何为初始覆盖基准（物理像素，含 DPI）
        let main = app.get_webview_window("main");
        let m_size = main.as_ref().and_then(|m| m.outer_size().ok());
        let m_pos = main.as_ref().and_then(|m| m.outer_position().ok());
        let m_scale = main.as_ref().and_then(|m| m.scale_factor().ok()).unwrap_or(1.0);

        // 已有窗口：直接 navigate 复用（on_page_load 每次导航都会触发，FONT_JS 会重注入），
        // 避免「先 close 再同 label 重建」的竞态导致窗口打不开
        if let Some(old) = app.get_webview_window("history") {
            let _ = old.navigate(parsed.clone());
            // 仍按主窗口几何覆盖定位（与原「关闭重建」的位置语义一致）
            if let Some(main_sz) = m_size {
                let _ = old.set_size(Size::Physical(fit_size_to_main(&old, main_sz)));
            }
            if let Some(p) = m_pos {
                let _ = old.set_position(Position::Physical(p));
            }
            let _ = old.show();
            let _ = old.unminimize();
            let _ = old.set_focus();
            return Ok(());
        }

        // 新建：先隐藏创建并定位到主窗口，再显示 —— 首次出现即在正确位置，无偏移闪烁
        let (log_w, log_h) = match m_size {
            Some(ps) => (ps.width as f64 / m_scale, ps.height as f64 / m_scale),
            None => (1280.0, 800.0),
        };
        let (log_x, log_y) = match m_pos {
            Some(p) => (p.x as f64 / m_scale, p.y as f64 / m_scale),
            None => (0.0, 0.0),
        };
        let win = WebviewWindowBuilder::new(&app, "history", WebviewUrl::External(parsed))
            .title("Pi Web - 完整会话历史")
            .decorations(true)
            .resizable(true)
            .visible(false) // 先隐藏，定位好再显示
            .inner_size(log_w, log_h)
            .position(log_x, log_y)
            // 页面每次加载完成后注入字体加大脚本（窗口生命周期内每次导航都会触发）
            .on_page_load(|win, _payload| {
                let _ = win.with_webview(|pw| {
                    #[cfg(windows)]
                    inject_page_script(&pw);
                });
            })
            .build()
            .map_err(|e| e.to_string())?;
        // 精确校准（仍隐藏中）：原生标题栏/边框占掉 frame 尺寸，set_size 是内尺寸，
        // 因此按“主窗口外框 = 目标外框”折算内尺寸，使 history 外框贴合主窗口
        if let Some(main_sz) = m_size {
            let _ = win.set_size(Size::Physical(fit_size_to_main(&win, main_sz)));
        }
        if let Some(p) = m_pos {
            let _ = win.set_position(Position::Physical(p));
        }
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 按主窗口外框尺寸 + 目标窗口自身原生边框，折算一个让 history 外框贴合主窗口的内尺寸
fn fit_size_to_main(
    win: &tauri::WebviewWindow,
    main_size: tauri::PhysicalSize<u32>,
) -> tauri::PhysicalSize<u32> {
    let inner = win.inner_size().ok();
    let outer = win.outer_size().ok();
    let frame_w = outer
        .as_ref()
        .map(|o| o.width as i64 - inner.as_ref().map(|i| i.width as i64).unwrap_or(0))
        .unwrap_or(0);
    let frame_h = outer
        .as_ref()
        .map(|o| o.height as i64 - inner.as_ref().map(|i| i.height as i64).unwrap_or(0))
        .unwrap_or(0);
    let tw = (main_size.width as i64 - frame_w).max(400) as u32;
    let th = (main_size.height as i64 - frame_h).max(300) as u32;
    tauri::PhysicalSize::new(tw, th)
}

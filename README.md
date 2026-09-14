# Pi Desktop

> pi-web 的 Windows 桌面壳（Tauri 2）

<div align="center">

[![License](https://img.shields.io/github/license/gfwHacker/pi-web-desktop)](LICENSE)
[![Release](https://img.shields.io/github/v/release/gfwHacker/pi-web-desktop)](https://github.com/gfwHacker/pi-web-desktop/releases)
[![Build Status](https://img.shields.io/github/actions/workflow/status/gfwHacker/pi-web-desktop/build.yml?branch=main)](https://github.com/gfwHacker/pi-web-desktop/actions)
[![Downloads](https://img.shields.io/github/downloads/gfwHacker/pi-web-desktop/total)](https://github.com/gfwHacker/pi-web-desktop/releases)

</div>

基于 **Tauri 2** 为 **pi-web** 封装的原生 Windows 桌面壳：无边框窗口、系统托盘、单实例、首次运行自动检测/引导安装运行时、壳层增强（主题联动 / 打开工作目录 / Telegram 代理注入）。**pi-web 页面本身不做任何改动**，pi-coding-agent 由 pi-web 内置提供。

- 当前版本：**1.0.0**（安装包约 5.8 MB）
- 安装模式：**currentUser**（可自选目录，默认 `%LOCALAPPDATA%\Pi Desktop`，对当前用户可写）
- 运行时：系统 WebView2（Windows 10/11 自带）
- 代理/插件：与 pi CLI 共享 `~/.pi/agent` 生态（插件、配置、会话、telegram.json）

## 功能一览

| 能力 | 说明 |
|---|---|
| 单实例启动 | 重复启动自动聚焦已有窗口，不会新开 |
| 环境检测+引导 | 综合检测（环境变量 → node 官网路径 → 自托管），缺项显示「去安装」引导 |
| 复用已有安装 | 已全局安装 pi-web / pi-coding-agent 直接复用，不重复引导 |
| 数据放安装目录 | node / runtime / 日志 / 代理 init 全随应用存放（可自选、可写目录） |
| 无边框窗口 | 自绘标题栏（40px）+ 最小化/最大化/关闭；**配色跟随系统深浅主题** |
| 打开工作目录 | 标题栏 📂 一键打开 **pi-web 当前工作目录**（桥实时同步 + 会话目录回填）；未选择目录时按钮置灰 |
| 重启 Pi 服务 | 标题栏 🔄 一键重启 pi-web，重启后按桥记录的路由恢复当前页面（不附加 `?cwd=`） |
| 设置面板 | 标题栏 ⚙️：关闭按钮行为 · 下载源（npm / Node / GitHub 代理，默认国内镜像可切官方/自定义）· 组件版本与一键更新 · 打开配置文件目录 |
| 关闭行为三模式 | 默认「每次询问」弹确认框（托盘/退出/取消）；可在设置面板改为最小化到托盘或直接退出（存 `config/settings.json`） |
| Telegram 代理 | 进程级 `NODE_OPTIONS` 注入，仅 `*.telegram.org` 走代理；**插件升级不影响** |
| 系统托盘 | 常驻、显示/隐藏（**左键单击切换**）· 托盘内分别更新 Pi Web / Pi Coding Agent |
| 固定端口 | `31141`（被占自动回退），保证 pi-web 的 localStorage 持久 |
| 退出清理 | 退出自动终止 pi-web 子进程，不残留端口占用 |

## 架构

```
┌─────────────────────────────────────────────┐
│ Pi Desktop（Tauri 2 · Rust，~5MB 外壳）     │
│  单实例 · 标题栏(主题跟随) · 托盘 · Commands│
└────────────────────┬────────────────────────┘
                     │ WebView2 (tauri.localhost)
┌────────────────────▼────────────────────────┐
│ src/web/index.html  加载页 + 引导流程        │
│  check_environment 检测 → 缺则「去安装」→   │
│  启动 → iframe 加载 pi-web 首页           │
└────────────────────┬────────────────────────┘
                     │ iframe (http://127.0.0.1:31141)
┌────────────────────▼────────────────────────┐
│ Pi Web（Node.js 子进程）                    │
│  优先复用全局安装，其次自托管 runtime       │
│  NODE_OPTIONS 注入 telegram 代理引导        │
│  agent 目录 = ~/.pi/agent（共享生态）       │
└─────────────────────────────────────────────┘
```

## 目录结构（源码 + 编译脚本）

```
pi-desktop/
├── BuildPiDesktop.bat        # 🖱️ 一键重新构建（双击 → scripts/build.ps1）
├── README.md                # 本文件
├── package.json             # npm 脚本：tauri / dev / build / build:installer
├── package-lock.json
├── .gitignore
├── .cargo/config.toml       # 项目级 cargo：target=gnu + linker=gcc（必留）
├── scripts/                 # 编译辅助脚本
│   └── build.ps1            #   可重复构建主脚本（tauri build + NSIS 打包）
├── src/
│   └── web/                 # 前端资源（Tauri frontendDist）
│       ├── index.html       #   加载页（标题栏/环境引导/设置面板/更新浮层/桥消息）
│       └── icon.png         #   标题栏/加载页图标（= icons/256x256.png 副本，换图时手动覆盖）
└── src-tauri/               # Rust 后端（Tauri 2）
    ├── tauri.conf.json      # 窗口(无边框)/图标/打包(currentUser+NSIS)/资源(bundle.resources)
    ├── capabilities/default.json
    ├── Cargo.toml / Cargo.lock
    ├── rust-toolchain.toml  # 固定 GNU 工具链 stable-x86_64-pc-windows-gnu
    ├── build.rs             # 调 tauri_build::build()
    ├── hooks.nsh            # NSIS 钩子：覆盖安装/卸载前 taskkill 进程树 + 整目录重建/删除
    ├── icons/               # 应用图标：icon.ico（多尺寸）+ 512 母图 + 256x256.png + 32x32.png（托盘内嵌）
    ├── resources/           # 随安装包分发的运行时文件（bundle.resources）
    │   ├── telegram-proxy-init.cjs  # Telegram 域代理注入引导（随包放安装根，运行期只读）
    │   └── WebView2Loader.dll       # GNU 打包需手动声明打包的 WebView2 引导 dll（映射到安装根、exe 旁）
    └── src/
        ├── main.rs          # 入口、插件(单实例/窗口状态)、命令注册、Exit 清理子进程
        ├── bridge.rs        # 跨域桥（主题/路由/工作目录感知 + 完整历史窗口 async 弹窗）
        ├── node_manager.rs  # 数据目录/环境基元：detect|resolve node+git、自托管安装、选目录/开链接
        ├── piweb.rs         # check_environment 综合检测、全局复用、启停 pi-web、代理注入/PATH
        ├── settings.rs      # 壳层设置 config/settings.json（关闭行为/镜像源），读写 + 打开目录命令
        └── tray.rs          # 系统托盘（显示/隐藏/退出、托盘内更新）
```
## 构建环境要求

- Node.js ≥ 22
- Rust 工具链 `stable-x86_64-pc-windows-gnu`（`rust-toolchain.toml` 已固定，构建时自动安装）
- MinGW-w64（含 `windres`，可通过 WinGet / 官网安装）
- （可选）Cargo 国内镜像：建议配置 `~/.cargo/config.toml` 以加速依赖下载

> 依赖/产物（node_modules、target、gen）由构建脚本自动生成，项目内不保留。

## 一键重新构建

双击项目根目录的 **`BuildPiDesktop.bat`** 即可（或 PowerShell 运行 `scripts\build.ps1`），自动完成：环境检查 → `npm install` → `cargo build --release` → `npx tauri build --bundles nsis` → 把安装包复制一份到项目根 → 打开产物目录。首次构建需联网下载依赖与 NSIS 工具（GitHub 走 ghfast.top 镜像）。

产物：`src-tauri/target/x86_64-pc-windows-gnu/release/bundle/nsis/Pi Desktop_1.0.0_x64-setup.exe`（项目根目录留一份同名副本）

> 脚本参数：`-SkipNpm`（跳过 npm install）· `-NoOpen`（构建后不自动打开目录）· `-Clean`（全量重编译，换图标/资源后必须加，见下）

> 换图标注意：母图为 `src-tauri/icons/512x512.png`（不参与打包，仅作换图源）。仓库实际图标文件：`icon.ico`（打包/窗口/安装包用，多尺寸 16~256）、`256x256.png`、`32x32.png`（托盘编译期内嵌，`include_bytes!`）——打包所需规格均已在列。前端 logo 用 `src/web/icon.png`（=`icons/256x256.png` 副本）。换图时需从 512 母图手工产出各规格覆盖，并将 256x256.png 复制到 `src/web/icon.png`。最后构建带 `-Clean`（tauri-build 不会因图标文件变化自动重编译资源；托盘为 `include_bytes!` 内嵌，必须重编译）。

## 数据存放（安装目录）

- 安装器用 **currentUser** 模式，默认装到 `%LOCALAPPDATA%\Pi Desktop`（可写、可自选）
- 首次运行 `check_environment` 检测：Node.js（环境变量 → 官网路径 → 自托管）→ pi-web / pi-coding-agent（自托管 runtime 或**用户全局已装**任一处即就绪）
- 缺项 → 前端「去安装」引导：自托管 Node（下载官方 zip 解压到安装目录）+ pi-web
- 启动时 `resolve_piweb()` 优先自托管 runtime，回退全局；node/npm 目录**应用内注入 PATH**（不写系统环境变量）
- 壳层设置落在 `<安装目录>\config\settings.json`：关闭按钮行为 + 下载源。默认全走**国内镜像**（npm 经 `--registry` 参数注入、Node 下载/GitHub 代理走 npmmirror/ghfast.top），可在 ⚙️ 设置面板改官方或自定义

## 设计要点与注意事项

- 标题栏与 pi-web 主题统一：壳层 CSS 用 `prefers-color-scheme` 跟随系统，与 pi-web 默认「跟随系统」一致
- 📂「打开工作目录」按钮：打开的是 **pi-web 当前工作目录**（注入桥嗅探 pi-web 请求的 `?cwd=` + `/api/sessions` 回填，实时同步），**不是壳层配置目录**；未选择目录时按钮置灰（配置目录改在设置面板「配置文件目录 → 打开目录」打开）
- `?cwd=` 唤起参数：pi-web 原生支持 `?cwd=<forward-slash path>` 直达指定目录（Windows 路径须转正斜杠）；壳层启动**不自动附加**该参数（避免无会话目录时异常跳转，由 pi-web 自行恢复会话/默认目录）
- Telegram 代理持久化：`NODE_OPTIONS=--require <init>` 进程级注入；**反斜杠会被 NODE_OPTIONS 转义，必须用正斜杠路径**
- WebView2Loader.dll：GNU 工具链下 tauri 打包不自动带它；经 `bundle.resources` **直接映射到安装根（exe 旁）**——系统加载 dll 只从 exe 目录搜；hooks.nsh 的 POSTINSTALL 为空宏（安装即就位，无搬运动作）
- 关闭按钮默认「每次询问」：`CloseRequested` 按 `close_behavior` 分流（tray/quit/ask）；点「退出应用」先置 `quit_intent` 再关窗，避免确认弹窗与关闭事件互相触发死循环
- 托盘初始化失败不阻断启动：仅写 `<安装目录>\config\tray-init-error.log` 便于排查；托盘图标内嵌 `icons/32x32.png`（全出血版，PNG 解码只需 `image-png` feature；若改用 .ico 需同时开启 `image-ico`）
- agent 目录默认共享 `~/.pi/agent`（不设置 `PI_CODING_AGENT_DIR`）
- Rust 执行 `.cmd`/`.bat` 需 `cmd /c` 包装（npm 全局检测/安装）
- `npm root -g` 返回的已是 node_modules，拼路径不要再加 node_modules

## 相关链接

- 仓库：[github.com/gfwHacker/pi-web-desktop](https://github.com/gfwHacker/pi-web-desktop)
- 上游：[pi-web](https://github.com/agegr/pi-web) · [pi](https://github.com/earendil-works/pi) · [Tauri 2](https://v2.tauri.app)

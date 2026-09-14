#requires -Version 5.1
# ============================================================
#  Pi Desktop · Windows 安装包构建脚本（可重复执行）
#
#  用法：
#    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build.ps1
#    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build.ps1 -Clean
#
#  参数：
#    -Clean    全量重编译（cargo clean）。更换图标 / 资源后必须加，
#              否则 tauri-build 不会因图标文件变化自动重编译资源。
#    -SkipNpm  跳过 npm install（依赖已就绪时加速）
#    -NoOpen   构建成功后不自动打开产物目录
#
#  产物：
#    src-tauri\target\x86_64-pc-windows-gnu\release\bundle\nsis\Pi Desktop_<ver>_x64-setup.exe
#    并自动复制一份到项目根目录。
# ============================================================
[CmdletBinding()]
param(
    [switch]$Clean,
    [switch]$SkipNpm,
    [switch]$NoOpen
)

$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

# ---------- 输出辅助 ----------
function Write-Step([string]$m) { Write-Host "`n========== $m ==========" -ForegroundColor Cyan }
function Write-Ok([string]$m)   { Write-Host "  [OK]   $m" -ForegroundColor Green }
function Write-Note([string]$m) { Write-Host "  [..]   $m" -ForegroundColor DarkGray }
function Write-Bad([string]$m)  { Write-Host "  [FAIL] $m" -ForegroundColor Red }
function Assert-Native([string]$what) {
    if ($LASTEXITCODE -ne 0) { throw "$what 失败 (exit=$LASTEXITCODE)，详见上方输出。" }
}

# ---------- 路径常量 ----------
$root    = Split-Path -Parent $PSScriptRoot          # pi-desktop 项目根
$srcTauri = Join-Path $root 'src-tauri'
$archTarget = 'x86_64-pc-windows-gnu'                # 由 .cargo/config.toml 固定
$toolchain  = 'stable-x86_64-pc-windows-gnu'         # 由 rust-toolchain.toml 固定
$start  = Get-Date

try {
    Write-Step "Pi Desktop 构建开始"
    Write-Note "项目目录 : $root"
    Write-Note "PowerShell: $($PSVersionTable.PSVersion)"

    # ================= 1. 环境检测 =================
    Write-Step "1/5 环境检测"

    # --- Node.js / npm ---
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) { throw '未找到 node，请安装 Node.js >= 22 并加入 PATH。' }
    if (-not (Get-Command npm  -ErrorAction SilentlyContinue)) { throw '未找到 npm，请确认 Node.js 安装完整。' }
    Write-Ok "node $((& node -v)) / npm $((& npm -v))"

    # --- cargo / rustup（依赖系统环境变量：CARGO_HOME / RUSTUP_HOME / PATH）---
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw '未找到 cargo，请配置环境变量 CARGO_HOME=D:\bigdata\.sdk\.cargo 并将 .cargo\bin 加入 PATH。' }
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { throw '未找到 rustup，请配置环境变量 RUSTUP_HOME=D:\bigdata\.sdk\.rustup 并将 .cargo\bin 加入 PATH。' }
    Write-Ok "cargo $((& cargo -V))"

    # --- MinGW-w64 (GNU 链接器 gcc + 资源编译器 windres) ---
    if (-not (Get-Command gcc -ErrorAction SilentlyContinue)) { throw '未找到 gcc，请安装 MinGW-w64 并加入 PATH。' }
    Write-Ok "gcc $(((& gcc --version) | Select-Object -First 1))"
    Write-Ok "windres $(((& windres --version) | Select-Object -First 1))"
    
    # --- Rust GNU 工具链（rust-toolchain.toml 固定）---
    $list = (& rustup toolchain list 2>$null) -join "`n"
    if ($list -match [regex]::Escape($toolchain)) {
        Write-Ok "Rust GNU 工具链已装 : $toolchain"
    } else {
        Write-Note "安装 Rust GNU 工具链 $toolchain（首次，需几分钟）……"
        & rustup toolchain install $toolchain --profile minimal
        Assert-Native 'rustup toolchain install'
        Write-Ok "已安装 $toolchain"
    }

    # ================= 2. npm install =================
    if (-not $SkipNpm) {
        Write-Step "2/5 npm install（tauri CLI 依赖）"
        Push-Location $root
        try {
            & npm install
            Assert-Native 'npm install'
        } finally { Pop-Location }
    } else {
        Write-Note "跳过 npm install (-SkipNpm)"
    }

    # ================= 3. 可选全量清理 =================
    if ($Clean) {
        Write-Step "3/5 cargo clean（全量重编译，等待较久）"
        Push-Location $srcTauri
        try {
            & cargo clean
            Assert-Native 'cargo clean'
        } finally { Pop-Location }
        Write-Ok "已清理编译缓存"
    }

    # ================= 4. tauri build =================
    Write-Step "$(if ($Clean) { '4/5' } else { '3/5' }) Rust release 编译 + NSIS 打包"
    Write-Note "命令行 : npx tauri build --bundles nsis"
    # 首次打包需联网下载 NSIS 工具；给 GitHub 下载走国内镜像（仅当未自行设置）
    if (-not $env:TAURI_BUNDLER_TOOLS_GITHUB_MIRROR) {
        $env:TAURI_BUNDLER_TOOLS_GITHUB_MIRROR = 'https://ghfast.top/https://github.com'
        Write-Note "设置 TAURI_BUNDLER_TOOLS_GITHUB_MIRROR=ghfast.top（首次下载 NSIS 用）"
    }
    Push-Location $root
    try {
        & npx tauri build --bundles nsis
        Assert-Native 'tauri build'
    } finally { Pop-Location }

    # ================= 5. 收集产物 =================
    # tauri 打包日志中的标准输出目录
    $nsisDir = Join-Path $srcTauri ("target\$archTarget\release\bundle\nsis")
    if (-not (Test-Path $nsisDir)) {
        # 兜底：递归查找含 *-setup.exe 的目录，取最像 bundle\nsis 的那个
        $d = Get-ChildItem (Join-Path $srcTauri 'target') -Recurse -Directory -ErrorAction SilentlyContinue |
             Where-Object { Get-ChildItem $_.FullName -Filter '*-setup.exe' -File -ErrorAction SilentlyContinue } |
             Sort-Object FullName | Select-Object -First 1
        if ($d) { $nsisDir = $d.FullName }
    }
    if (-not (Test-Path $nsisDir)) { throw "未找到 NSIS 产物目录：$nsisDir" }
    $installer = Get-ChildItem $nsisDir -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
                 Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $installer) { throw "未在 $nsisDir 中找到 *-setup.exe 安装包" }

    # 复制一份到项目根目录（与旧约定一致）
    $dest = Join-Path $root $installer.Name
    Copy-Item $installer.FullName $dest -Force

    $secs = [int]((Get-Date) - $start).TotalSeconds
    Write-Step "构建成功（耗时 $secs 秒）"
    Write-Ok "安装包 : $($installer.FullName)"
    Write-Ok "根副本 : $dest"
    if (-not $NoOpen) {
        Write-Note "打开产物目录……"
        Start-Process explorer.exe -ArgumentList ('/select,"' + $installer.FullName + '"')
    }
    exit 0
}
catch {
    Write-Bad $_.Exception.Message
    Write-Bad '构建失败，请根据上方日志排查。'
    exit 1
}

// Pi Desktop proxy bootstrap (loaded via NODE_OPTIONS --require)
//
// 职责：启动时检查 pi-telegram 的 lib/telegram-api.ts 是否带有文档所述的
// 代理补丁（setGlobalDispatcher 方案）。
//   - 已有补丁 -> 跳过，什么都不做
//   - 缺补丁   -> 自动重新打上（插件升级覆盖补丁后自愈）
// 不做任何运行时 fetch 包装 / dispatcher 劫持。
"use strict";

const os = require("node:os");
const path = require("node:path");
const fs = require("node:fs");

function debug(msg) {
  if (process.env.PI_DESKTOP_DEBUG) {
    try { console.log("[pi-desktop-proxy] " + msg); } catch (e) {}
  }
}

// ---------- 目标文件定位 ----------
function resolveAgentDir() {
  const envDir = process.env.PI_CODING_AGENT_DIR || process.env.PI_AGENT_DIR;
  if (envDir && fs.existsSync(envDir)) return envDir;
  const def = path.join(os.homedir(), ".pi", "agent");
  return fs.existsSync(def) ? def : envDir || def;
}

function findTarget() {
  // 测试专用覆盖（正常启动不会设置）
  const explicit = process.env.PI_TELEGRAM_PATCH_TARGET;
  if (explicit && fs.existsSync(explicit)) return explicit;
  const agentDir = resolveAgentDir();
  const candidates = [
    path.join(agentDir, "npm", "node_modules", "@llblab", "pi-telegram", "lib", "telegram-api.ts"),
    path.join(agentDir, "node_modules", "@llblab", "pi-telegram", "lib", "telegram-api.ts"),
  ];
  for (const c of candidates) {
    if (fs.existsSync(c)) return c;
  }
  return null;
}

// undici 是否可被补丁 import 解析到（补丁前置条件，缺了就跳过以免打坏文件）
function undiciResolvableFrom(target) {
  let dir = path.dirname(target);
  for (let depth = 0; depth < 10; depth++) {
    if (path.basename(dir) === "node_modules") {
      return fs.existsSync(path.join(dir, "undici", "package.json"));
    }
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return false;
}

// ---------- 补丁内容 ----------
const PATCH_MARKER = /setGlobalDispatcher\s*\(\s*new\s+ProxyAgent\s*\(/;

const ANCHOR = "export const TELEGRAM_API_BASE = ";

const PATCH_BLOCK = `// --- Proxy support: use HTTPS_PROXY / HTTP_PROXY / proxy from telegram.json ---
// We patch the global fetch (undici-based) via setGlobalDispatcher so that
// every Telegram API call respects the proxy configuration.
import { setGlobalDispatcher, ProxyAgent } from "undici";

let proxyUrl =
  process.env.HTTPS_PROXY ||
  process.env.https_proxy ||
  process.env.HTTP_PROXY ||
  process.env.http_proxy;

try {
  // Also try to read proxy from telegram.json at well-known location
  const { readFileSync } = await import("node:fs");
  const { resolveTelegramConfigPath } = await import("./paths.ts");
  const configPath = resolveTelegramConfigPath();
  try {
    const raw = readFileSync(configPath, "utf-8");
    const cfg = JSON.parse(raw);
    if (cfg.proxy?.url && !proxyUrl) {
      proxyUrl = cfg.proxy.url;
    }
  } catch { /* no config or no proxy field, ignore */ }
} catch { /* paths module not ready yet, ignore */ }

if (proxyUrl) {
  try {
    setGlobalDispatcher(new ProxyAgent(proxyUrl));
    console.debug(\`[telegram-api] using proxy: \${proxyUrl}\`);
  } catch (err) {
    console.warn(\`[telegram-api] failed to setup proxy \${proxyUrl}:\`, (err as Error).message);
  }
}

`;

function detectEol(text) {
  return text.includes("\r\n") ? "\r\n" : "\n";
}

// 幂等 + 防并发：用独占锁文件保证同一时刻只有一个进程在打补丁
function applyPatch(target) {
  const lockPath = target + ".proxy-patch.lock";
  let lock = null;
  try {
    lock = fs.openSync(lockPath, "wx");
  } catch {
    debug("skip: another process is patching (lock busy)");
    return false;
  }
  try {
    const raw = fs.readFileSync(target, "utf8");
    if (PATCH_MARKER.test(raw)) {
      debug("skip: patch already present: " + target);
      return false;
    }
    const idx = raw.indexOf(ANCHOR);
    if (idx < 0) {
      debug("skip: anchor not found (plugin layout changed?): " + target);
      return false;
    }
    if (!undiciResolvableFrom(target)) {
      debug("skip: undici not resolvable from plugin, leaving file untouched");
      return false;
    }
    const eol = detectEol(raw);
    const block = PATCH_BLOCK.replace(/\n/g, eol);
    const patched = raw.slice(0, idx) + block + raw.slice(idx);
    const tmp = target + ".proxy-patch.tmp";
    fs.writeFileSync(tmp, patched, "utf8");
    fs.renameSync(tmp, target); // 原子替换，避免并发读看到半截文件
    debug("applied proxy patch: " + target);
    return true;
  } catch (err) {
    debug("patch failed: " + (err && err.message));
    try { fs.unlinkSync(target + ".proxy-patch.tmp"); } catch (e) {}
    return false;
  } finally {
    try { fs.closeSync(lock); } catch (e) {}
    try { fs.unlinkSync(lockPath); } catch (e) {}
  }
}

try {
  const target = findTarget();
  if (!target) {
    debug("skip: telegram-api.ts not found");
  } else {
    applyPatch(target);
  }
} catch (err) {
  debug("init failed: " + (err && err.message));
}

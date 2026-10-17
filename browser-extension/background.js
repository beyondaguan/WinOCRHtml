// WinOCR 划词助手 - Background Service Worker
// 职责：
// 1. 管理与桌面端 winocr-host.exe 的 Native Messaging 连接
// 2. 连接检测（ping）并缓存状态，供 popup / content script 查询
// 3. 上下文菜单：划词后“翻译/采集”
// 4. 转发 content script 的请求，广播状态变化

const HOST_NAME = "com.winocr.native_host";

let port = null;
let connected = false;
let reconnectTimer = null;
let pingTimer = null;
let requestSeq = 0;
const pending = new Map(); // id -> {resolve, reject}

// ---------- 状态管理 ----------
function setConnected(v) {
  const changed = connected !== v;
  connected = v;
  if (changed) {
    chrome.runtime.sendMessage({ type: "conn-status", connected: v }).catch(() => {});
    // 通知所有 content script（它们需要更新气泡状态）
    chrome.tabs.query({}, (tabs) => {
      for (const tab of tabs) {
        chrome.tabs.sendMessage(tab.id, { type: "conn-status", connected: v }).catch(() => {});
      }
    });
  }
}

function getStatus() {
  return { connected, host: HOST_NAME };
}

// ---------- Native Messaging 连接管理 ----------
function connect() {
  if (port) return;
  try {
    port = chrome.runtime.connectNative(HOST_NAME);
  } catch (e) {
    console.warn("[winocr] connectNative failed:", e);
    setConnected(false);
    scheduleReconnect();
    return;
  }

  port.onMessage.addListener((msg) => {
    if (msg && msg.id && pending.has(msg.id)) {
      const p = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.ok) p.resolve(msg);
      else p.reject(new Error(msg.error || "host error"));
    }
  });

  port.onDisconnect.addListener(() => {
    const err = chrome.runtime.lastError;
    if (err) console.warn("[winocr] host disconnected:", err.message);
    port = null;
    setConnected(false);
    // 拒绝所有挂起请求
    for (const [, p] of pending) p.reject(new Error("host disconnected"));
    pending.clear();
    scheduleReconnect();
  });

  setConnected(true);
  // 立即 ping 确认桌面端存活
  ping();
  // host 连接已建立 → 补传缓存的生词
  flushPendingCollects();
}

function scheduleReconnect() {
  if (reconnectTimer) return;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    connect();
  }, 3000);
}

function disconnect() {
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  if (pingTimer) {
    clearInterval(pingTimer);
    pingTimer = null;
  }
  if (port) {
    try { port.disconnect(); } catch (_) {}
    port = null;
  }
  setConnected(false);
}

// ---------- 请求 / 响应 ----------
function sendRequest(action, payload) {
  return new Promise((resolve, reject) => {
    if (!port) {
      connect();
      // 连接是异步的，等待 host 进程冷启动握手（最多 3 秒）后发送
      setTimeout(() => {
        if (!port) return reject(new Error("host not connected"));
        doSend(action, payload, resolve, reject);
      }, 3000);
    } else {
      doSend(action, payload, resolve, reject);
    }
  });
}

function doSend(action, payload, resolve, reject) {
  const id = "req-" + (++requestSeq);
  const msg = Object.assign({ id, action }, payload);
  pending.set(id, { resolve, reject });
  try {
    port.postMessage(msg);
  } catch (e) {
    pending.delete(id);
    reject(e);
  }
}

// ---------- 动作封装 ----------

// 在线翻译引擎（不依赖桌面端 host，浏览器扩展可独立工作）
// 引擎：host（桌面端 Native Messaging）/ sf（SiliconFlow）/ mymemory / ollama / browser（内置 Translator API）

const MM_ENDPOINT = "https://api.mymemory.translated.net/get";
const MM_LANG_MAP = {
  zh: "zh-CN", en: "en-GB", ja: "ja-JP", ko: "ko-KR", fr: "fr-FR",
  de: "de-DE", es: "es-ES", ru: "ru-RU", it: "it-IT", pt: "pt-PT",
  ar: "ar-SA", th: "th-TH", vi: "vi-VN"
};

function mmLang(code) {
  const c = String(code || "").toLowerCase();
  if (!c || c === "auto") return "en-GB";
  return MM_LANG_MAP[c] || code;
}

function decodeEntities(s) {
  return String(s == null ? "" : s)
    .replace(/&#x([0-9a-fA-F]+);/g, (m, h) => { try { return String.fromCodePoint(parseInt(h, 16)); } catch (_) { return m; } })
    .replace(/&#(\d+);/g, (m, d) => { try { return String.fromCodePoint(parseInt(d, 10)); } catch (_) { return m; } })
    .replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">").replace(/&nbsp;/g, " ").replace(/&amp;/g, "&");
}

function mmChunks(text, max) {
  const out = []; let buf = "";
  String(text == null ? "" : text).split(/(\n+)/).forEach((seg) => {
    if ((buf + seg).length <= max) { buf += seg; return; }
    if (buf) { out.push(buf); buf = ""; }
    if (seg.length <= max) { buf = seg; return; }
    let s = seg;
    while (s.length > max) {
      let cut = Math.max(s.lastIndexOf(". ", max), s.lastIndexOf("。", max), s.lastIndexOf("！", max), s.lastIndexOf(" ", max));
      if (cut >= max || cut < max * 0.5) cut = max - 1;
      out.push(s.slice(0, cut + 1)); s = s.slice(cut + 1);
    }
    buf = s;
  });
  if (buf) out.push(buf);
  return out.filter((x) => x.trim());
}

async function translateMyMemory(text, srcLang, tgtLang, email) {
  const src = mmLang(srcLang), tgt = mmLang(tgtLang);
  const parts = [];
  for (const c of mmChunks(text, 450)) {
    let url = MM_ENDPOINT + "?q=" + encodeURIComponent(c) +
      "&langpair=" + encodeURIComponent(src + "|" + tgt);
    if (email) url += "&de=" + encodeURIComponent(email);
    const r = await fetch(url, { method: "GET" });
    const j = await r.json().catch(() => null);
    const det = String((j && j.responseDetails) || "");
    if (/MYMEMORY WARNING|ALL AVAILABLE FREE TRANSLATIONS/i.test(det) || (j && j.quotaFinished)) {
      throw new Error("MyMemory 今日免费额度已用尽");
    }
    if (!j || (j.responseStatus && Number(j.responseStatus) !== 200)) {
      throw new Error("MyMemory 返回 " + ((j && j.responseStatus) || r.status));
    }
    const out = decodeEntities(j.responseData && j.responseData.translatedText);
    if (!out) throw new Error("MyMemory 返回空内容");
    parts.push(out);
  }
  return parts.join("");
}

async function translateOllama(text, srcLang, tgtLang, url, model) {
  if (!model) throw new Error("未配置 Ollama 模型");
  const base = (url || "http://localhost:11434").replace(/\/+$/, "");
  const sys = "You are a professional translator. Translate the user text into " + (tgtLang || "zh") +
    ". Keep medical/technical terms accurate. Output ONLY the translation, no commentary, no quotes.";
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), 120000);
  try {
    const r = await fetch(base + "/api/chat", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        model,
        messages: [{ role: "system", content: sys }, { role: "user", content: text }],
        stream: false,
        options: { temperature: 0.3 }
      }),
      signal: ctl.signal
    });
    if (!r.ok) throw new Error("Ollama 返回 HTTP " + r.status);
    const j = await r.json();
    const out = (j && j.message && j.message.content) || "";
    if (!out.trim()) throw new Error("Ollama 模型返回空内容");
    return out.trim();
  } catch (e) {
    if (e && e.name === "AbortError") throw new Error("Ollama 翻译超时（120 秒）");
    if (e instanceof TypeError) throw new Error("无法连接 Ollama 服务（" + base + "）");
    throw e;
  } finally {
    clearTimeout(timer);
  }
}

async function translateSF(text, srcLang, tgtLang, settings) {
  const key = settings.sfKey || "";
  if (!key) throw new Error("未填写 SiliconFlow Key");
  const url = (settings.sfUrl || "https://api.siliconflow.cn/v1").replace(/\/+$/, "") + "/chat/completions";
  const model = settings.sfModel || "Qwen/Qwen3-8B";
  const sys = "You are a professional translator. Translate the user text into " + (tgtLang || "zh") +
    ". Keep medical/technical terms accurate. Output ONLY the translation, no commentary, no quotes.";
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), 60000);
  try {
    const r = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json", "Authorization": "Bearer " + key },
      body: JSON.stringify({
        model,
        messages: [{ role: "system", content: sys }, { role: "user", content: text }],
        temperature: 0.3,
        max_tokens: 4096,
        enable_thinking: false
      }),
      signal: ctl.signal
    });
    let j = null; try { j = await r.json(); } catch (_) {}
    const emsg = j && (j.message || (j.error && j.error.message));
    const code = (j && typeof j.code === "number") ? j.code : null;
    const bizErr = !!(j && j.error) || (code !== null && code !== 0 && code !== 200);
    if (!r.ok || bizErr) {
      const m = String(emsg || "");
      if (code === 20012 || /does not exist/i.test(m)) throw new Error("模型名不存在：" + m);
      if (r.status === 401 || r.status === 403) throw new Error("SiliconFlow Key 无效");
      if (r.status === 429) throw new Error("SiliconFlow 限流（429），请稍后重试");
      throw new Error(emsg || ("HTTP " + r.status));
    }
    const ch = j && j.choices && j.choices[0];
    const out = ((ch && ch.message && ch.message.content) || "").trim();
    if (!out) throw new Error("模型返回空内容");
    return out;
  } catch (e) {
    if (e && e.name === "AbortError") throw new Error("SiliconFlow 请求超时（60 秒）");
    throw e;
  } finally {
    clearTimeout(timer);
  }
}

async function translateBrowser(text, srcLang, tgtLang) {
  if (typeof globalThis.Translator === "undefined" || typeof globalThis.Translator.availability !== "function") {
    throw new Error("当前浏览器不支持内置 Translator API（需 Chrome/Edge 138+）");
  }
  const src = (srcLang && srcLang !== "auto") ? srcLang : "en";
  const tgt = tgtLang || "zh";
  const avail = await globalThis.Translator.availability({ sourceLanguage: src, targetLanguage: tgt });
  if (avail === "no") {
    throw new Error("浏览器内置翻译不支持该语言对（" + src + " → " + tgt + "）");
  }
  if (avail !== "available" && avail !== "downloadable") {
    throw new Error("浏览器内置翻译语言包不可用（availability=" + avail + "）");
  }
  const tr = await globalThis.Translator.create({ sourceLanguage: src, targetLanguage: tgt });
  // availability 为 "downloadable" 时，语言包尚未下载完成，等待就绪后再翻译
  if (avail === "downloadable" || (tr && typeof tr.availability === "string" && tr.availability !== "available")) {
    await waitForTranslatorReady(tr, 120000);
  }
  return await tr.translate(text);
}

// 等待内置 Translator 语言包下载完成（兼容不同实现的 API 形态）
function waitForTranslatorReady(tr, timeoutMs) {
  return new Promise((resolve, reject) => {
    // 某些实现提供 ready Promise / ready() 方法
    if (tr && typeof tr.ready === "object" && typeof tr.ready.then === "function") {
      tr.ready.then(resolve).catch(reject);
      return;
    }
    if (tr && typeof tr.ready === "function") {
      Promise.resolve(tr.ready()).then(resolve).catch(reject);
      return;
    }
    // 通用方案：轮询 availability + 监听 availabilitychange 事件
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      clearInterval(timer);
      if (tr && typeof tr.removeEventListener === "function") {
        tr.removeEventListener("availabilitychange", onChange);
      }
      resolve();
    };
    const onChange = () => {
      if (tr && tr.availability === "available") finish();
    };
    const timer = setInterval(() => {
      if (tr && tr.availability === "available") finish();
    }, 500);
    if (tr && typeof tr.addEventListener === "function") {
      tr.addEventListener("availabilitychange", onChange);
    }
    if (tr && tr.availability === "available") finish();
    // 超时保护（语言包较大时下载可能较慢）
    setTimeout(() => {
      if (done) return;
      done = true;
      clearInterval(timer);
      if (tr && typeof tr.removeEventListener === "function") {
        tr.removeEventListener("availabilitychange", onChange);
      }
      reject(new Error("浏览器内置翻译语言包下载超时（120 秒）"));
    }, timeoutMs);
  });
}

// 默认 browser 引擎（浏览器内置，独立工作，无需桌面端）
async function translateWithSettings(text, srcLang, tgtLang, settings) {
  const engine = settings.engine || "browser";
  if (engine !== "host") {
    return await translateOnline(engine, text, srcLang, tgtLang, settings);
  }
  // host 引擎：桌面端已连接则走 host，否则回退
  if (port) {
    try {
      const r = await Promise.race([
        sendRequest("translate", { src: text, srcLang, tgtLang }),
        new Promise((_, rej) => setTimeout(() => rej(new Error("timeout")), 15000))
      ]);
      if (r && r.ok && r.tgt) {
        return { tgt: r.tgt || "", engine: "host", label: ENGINE_LABELS.host };
      }
    } catch (_) {
      // host 失败/超时，回退
    }
  }
  // 回退：sf → mymemory → ollama → browser
  const fallbackChain = ["sf", "mymemory", "ollama", "browser"];
  let lastErr = null;
  for (const eng of fallbackChain) {
    try {
      return await translateOnline(eng, text, srcLang, tgtLang, settings);
    } catch (e) {
      lastErr = e;
    }
  }
  throw new Error("所有翻译引擎均不可用：" + ((lastErr && lastErr.message) || lastErr));
}

const ENGINE_LABELS = {
  host: "桌面端 WinOCR",
  sf: "SiliconFlow",
  mymemory: "MyMemory",
  ollama: "Ollama",
  browser: "浏览器内置",
  "local-ocr": "本地 OCR（PP-OCRv6）"
};

async function translateOnline(engine, text, srcLang, tgtLang, settings) {
  let tgt;
  switch (engine) {
    case "sf": tgt = await translateSF(text, srcLang, tgtLang, settings); break;
    case "mymemory": tgt = await translateMyMemory(text, srcLang, tgtLang, settings.mymemoryEmail); break;
    case "ollama": tgt = await translateOllama(text, srcLang, tgtLang, settings.ollamaUrl, settings.ollamaModel); break;
    case "browser": tgt = await translateBrowser(text, srcLang, tgtLang); break;
    default: throw new Error("未知引擎：" + engine);
  }
  return { tgt, engine, label: ENGINE_LABELS[engine] || engine };
}

async function translate(text, srcLang, tgtLang) {
  return await new Promise((resolve, reject) => {
    loadSettings((settings) => {
      translateWithSettings(text, srcLang, tgtLang, settings)
        .then(resolve)
        .catch(reject);
    });
  });
}

// 兼容旧调用：仅取翻译文本
async function translateTextOnly(text, srcLang, tgtLang) {
  const r = await translate(text, srcLang, tgtLang);
  return r && r.tgt ? r.tgt : "";
}

// ---------- 采集缓存同步（方案 A：主程序未运行时可独立采集） ----------
// 设计：
// - host 已连接：直接写入桌面端生词本（SQLite），并清除同 word 的本地缓存
// - host 未连接：生词先存入 chrome.storage.local（键 CACHE_KEY），返回 { ok:true, cached:true }
//   （ok=true 表示「已接受」，气泡显示「已缓存，待同步」而非失败）
// - host 连接建立后（connect() 成功 / 每次 ping 成功）自动调用 flushPendingCollects() 补传
const COLLECT_CACHE_KEY = "winocr_pending_collects";

function loadCachedCollects(cb) {
  chrome.storage.local.get({ [COLLECT_CACHE_KEY]: [] }, (items) => {
    if (chrome.runtime.lastError) { cb([]); return; }
    cb(items[COLLECT_CACHE_KEY] || []);
  });
}

function saveCachedCollects(list, cb) {
  chrome.storage.local.set({ [COLLECT_CACHE_KEY]: list }, () => {
    if (cb) cb();
  });
}

// 追加一条待补传记录（按 word 去重，重复则更新译文/时间）
function cacheCollect(entry, cb) {
  loadCachedCollects((list) => {
    const idx = list.findIndex((e) => e && e.word === entry.word);
    if (idx >= 0) list[idx] = entry;
    else list.push(entry);
    // 上限保护：最多保留 500 条，避免本地缓存无限增长
    if (list.length > 500) list = list.slice(-500);
    saveCachedCollects(list, cb);
  });
}

// 补传所有待同步生词到桌面端；成功的从缓存移除，失败的保留待下次
function flushPendingCollects() {
  // 连接未建立时跳过，等下次连接成功再补传
  if (!connected) return;
  loadCachedCollects((list) => {
    if (!list.length) return;
    const remaining = [];
    let done = 0;
    const total = list.length;
    list.forEach((entry) => {
      if (!entry || !entry.word) { done++; return; }
      // 注意：sendRequest 内部如果 port 断开会 reject，这里捕获后保留
      sendRequest("collect", {
        src: entry.word, tgt: entry.tgt || "", tags: entry.tags || [],
        srcLang: entry.srcLang || "auto", tgtLang: entry.tgtLang || "zh"
      }).then((r) => {
        if (r && r.ok) {
          console.log("[winocr] 补传生词成功:", entry.word);
        } else {
          console.warn("[winocr] 补传失败，保留缓存:", entry.word, r && r.error);
          remaining.push(entry);
        }
      }).catch((e) => {
        console.warn("[winocr] 补传异常，保留缓存:", entry.word, e && e.message);
        remaining.push(entry);
      }).finally(() => {
        done++;
        if (done >= total) {
          saveCachedCollects(remaining);
          console.log("[winocr] 补传完成:", total - remaining.length, "/", total);
        }
      });
    });
  });
}

async function collect(word, tgt, tags, srcLang, tgtLang) {
  // host 未连接时直接走本地缓存（避免 sendRequest 的 3 秒等待）
  if (!connected) {
    console.warn("[winocr] host 未连接，采集缓存到本地:", word);
    const entry = {
      word, tgt: tgt || "", tags: tags || [],
      srcLang: srcLang || "auto", tgtLang: tgtLang || "zh",
      ts: Date.now()
    };
    await new Promise((resolve) => cacheCollect(entry, resolve));
    return { ok: true, cached: true, data: { count: 1, items: [{ word, project: "待同步", level: "" }] } };
  }

  // host 已连接：尝试直接写桌面端生词本
  try {
    const r = await sendRequest("collect", {
      src: word, tgt: tgt || "", tags: tags || [], srcLang, tgtLang
    });
    // 成功：清除本地同 word 缓存（如果有）
    if (r && r.ok) {
      loadCachedCollects((list) => {
        const next = list.filter((e) => e && e.word !== word);
        if (next.length !== list.length) saveCachedCollects(next);
      });
    }
    return r;
  } catch (e) {
    // host 发送失败（连接刚好断开等）→ 缓存到本地，返回 cached 标记
    console.warn("[winocr] host 采集发送失败，缓存到本地:", word, e && e.message);
    const entry = {
      word, tgt: tgt || "", tags: tags || [],
      srcLang: srcLang || "auto", tgtLang: tgtLang || "zh",
      ts: Date.now()
    };
    await new Promise((resolve) => cacheCollect(entry, resolve));
    return { ok: true, cached: true, data: { count: 1, items: [{ word, project: "待同步", level: "" }] } };
  }
}

async function ocrTranslate(imgBase64) {
  // 优先使用本地 OCR（Offscreen Document + PP-OCRv6，完全独立于主程序）
  try {
    // 确保 offscreen 已初始化（首次调用会创建并预加载模型）
    await ensureOcrOffscreen();
    if (ocrOffscreenReady) {
      const r = await sendToOcrOffscreen({ type: 'ocr-translate', img: imgBase64 });
      if (r && r.ok) {
        return { ok: true, src_text: r.text || '', tgt: r.text || '', engine: 'local-ocr', label: ENGINE_LABELS['local-ocr'] };
      }
      throw new Error((r && r.error) || '本地 OCR 返回空');
    }
    throw new Error('本地 OCR 未就绪');
  } catch (e) {
    // 本地 OCR 失败（模型加载失败 / offscreen 不可用）→ 回退 host
    if (port) {
      const r = await sendRequest("ocr_translate", { img: imgBase64 });
      return Object.assign({}, r, { label: ENGINE_LABELS.host });
    }
    throw e;
  }
}

// ---------- Offscreen Document 管理（本地 OCR） ----------
let ocrOffscreenReady = false;
let ocrOffscreenCreating = null;

async function ensureOcrOffscreen() {
  if (ocrOffscreenReady) return true;
  if (ocrOffscreenCreating) return ocrOffscreenCreating;
  ocrOffscreenCreating = (async () => {
    try {
      // 检查是否已存在 offscreen document（getContexts 需 Chrome 116+；低版本直接尝试创建）
      let exists = false;
      if (typeof chrome.runtime.getContexts === 'function') {
        const contexts = await chrome.runtime.getContexts({
          contextTypes: ['OFFSCREEN_DOCUMENT'],
          documentUrls: [chrome.runtime.getURL('offscreen.html')]
        });
        exists = contexts.length > 0;
      } else if (typeof chrome.offscreen.hasDocument === 'function') {
        exists = await chrome.offscreen.hasDocument();
      }
      if (!exists) {
        await chrome.offscreen.createDocument({
          url: 'offscreen.html',
          reasons: ['WORKERS'],
          justification: 'Run local ONNX OCR (PP-OCRv6) for WinOCR extension'
        });
      }
      // 预加载模型
      await sendToOcrOffscreen({ type: 'ocr-init' });
      ocrOffscreenReady = true;
      return true;
    } catch (e) {
      console.warn('[winocr] 本地 OCR offscreen 初始化失败：', e);
      ocrOffscreenReady = false;
      return false;
    } finally {
      ocrOffscreenCreating = null;
    }
  })();
  return ocrOffscreenCreating;
}

function sendToOcrOffscreen(msg) {
  return new Promise((resolve) => {
    try {
      chrome.runtime.sendMessage(msg, (r) => {
        if (chrome.runtime.lastError) {
          resolve({ ok: false, error: chrome.runtime.lastError.message });
        } else {
          resolve(r || { ok: false, error: '本地 OCR 无响应' });
        }
      });
    } catch (e) {
      resolve({ ok: false, error: String(e && e.message || e) });
    }
  });
}

// 启动时尝试初始化本地 OCR（失败不阻塞，host 仍可用）
if (typeof chrome.offscreen !== 'undefined') {
  setTimeout(() => { ensureOcrOffscreen().catch(() => {}); }, 1000);
}

// ---------- 连接检测 ----------
function ping() {
  if (!port) return;
  const id = "ping-" + (++requestSeq);
  pending.set(id, {
    resolve: () => {
      setConnected(true);
      // 连接恢复：补传缓存的生词
      flushPendingCollects();
    },
    reject: () => setConnected(false)
  });
  try {
    port.postMessage({ id, action: "ping" });
  } catch (_) {
    pending.delete(id);
    setConnected(false);
  }
}

// 每 10 秒 ping 一次，检测桌面端退出
function startPing() {
  if (pingTimer) clearInterval(pingTimer);
  pingTimer = setInterval(ping, 10000);
}

// ---------- 消息路由 ----------
chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (!msg || typeof msg !== "object") return;

  switch (msg.type) {
    case "get-status":
      sendResponse(getStatus());
      return;

    case "translate": {
      const finish = (r) => {
        // r = { tgt, engine, label }
        if (r && r.tgt) sendResponse({ ok: true, tgt: r.tgt, engine: r.engine || "", label: r.label || "" });
        else sendResponse({ ok: false, error: "翻译引擎返回空" });
      };
      const fail = (e) => sendResponse({ ok: false, error: String(e && e.message || e) });
      if (msg.settings) {
        // 设置页「测试引擎」：用当前表单值直接测，不落盘
        translateWithSettings(msg.src, msg.srcLang, msg.tgtLang, msg.settings)
          .then(finish)
          .catch(fail);
      } else {
        translate(msg.src, msg.srcLang, msg.tgtLang)
          .then(finish)
          .catch(fail);
      }
      return true; // async
    }

    case "collect": {
      collect(msg.src, msg.tgt, msg.tags, msg.srcLang, msg.tgtLang)
        .then((r) => {
          const resp = {
            ok: !!(r && r.ok),
            cached: !!(r && r.cached),
            data: r && r.data,
            error: r && r.error
          };
          // 记录最近一次采集结果到 storage，供 popup 右上角徽章显示。
          // 所有采集入口（划词气泡 / OCR 气泡 / 右键菜单 / popup）都走本消息处理器，
          // 在这里集中写一处即可全覆盖；popup 通过 storage.onChanged 实时刷新。
          if (resp.ok && resp.data && Array.isArray(resp.data.items) && resp.data.items.length) {
            try {
              chrome.storage.local.set({
                winocr_last_collect: {
                  items: resp.data.items.slice(0, 200),
                  count: resp.data.count,
                  cached: resp.cached,
                  ts: Date.now()
                }
              });
            } catch (_) {}
          }
          sendResponse(resp);
        })
        .catch((e) => sendResponse({ ok: false, error: String(e && e.message || e) }));
      return true;
    }

    case "list-collect": {
      // 只读拉取桌面端生词本（B 方案：桌面端唯一真源，扩展侧镜像）
      sendRequest("list_collect", { limit: msg.limit || 0 })
        .then((r) => sendResponse({ ok: !!(r && r.ok), data: r && r.data, error: r && r.error }))
        .catch((e) => sendResponse({ ok: false, error: String(e && e.message || e) }));
      return true;
    }

    case "ocr_translate": {
      ocrTranslate(msg.img)
        .then((r) => sendResponse(r))
        .catch((e) => sendResponse({ ok: false, error: String(e && e.message || e) }));
      return true;
    }

    case "ping":
      sendResponse({ ok: connected });
      return;

    case "settings-updated": {
      const s = msg.settings || DEFAULT_SETTINGS;
      applyContextMenus(s);
      sendResponse({ ok: true });
      return;
    }
  }
});

// ---------- 上下文菜单 ----------
const DEFAULT_SETTINGS = {
  srcLang: "auto",
  tgtLang: "zh",
  engine: "browser",         // host | sf | mymemory | ollama | browser（默认浏览器内置，独立工作）
  autoBubble: true,
  bubbleTimeout: 0,          // 0 = 常显示（默认），>0 为自动关闭秒数
  ctxTranslate: true,
  ctxCollect: true,
  ttsRate: "1.0",
  ttsPitch: "1.0",
  sfKey: "",
  sfUrl: "https://api.siliconflow.cn/v1",
  sfModel: "Qwen/Qwen3-8B",
  mymemoryEmail: "",
  ollamaUrl: "http://localhost:11434",
  ollamaModel: ""
};

function loadSettings(cb) {
  chrome.storage.sync.get(DEFAULT_SETTINGS, (items) => {
    if (chrome.runtime.lastError) items = DEFAULT_SETTINGS;
    cb(items);
  });
}

function applyContextMenus(settings) {
  chrome.contextMenus.removeAll(() => {
    void chrome.runtime.lastError;
    if (settings.ctxTranslate) {
      chrome.contextMenus.create({
        id: "winocr-translate",
        title: "WinOCR 翻译「%s」",
        contexts: ["selection"]
      });
    }
    if (settings.ctxCollect) {
      chrome.contextMenus.create({
        id: "winocr-collect",
        title: "WinOCR 采集生词「%s」",
        contexts: ["selection"]
      });
    }
  });
}

chrome.runtime.onInstalled.addListener(() => {
  connect();
  startPing();
  loadSettings(applyContextMenus);
});

// 通过 scripting.executeScript 读取当前页选区（PDF 等 content script 不可注入的场景）
function getSelectionViaScripting(tab) {
  return new Promise((resolve) => {
    if (!tab || !tab.id || !tab.url) { resolve(""); return; }
    if (!tab.url.startsWith("http") && !tab.url.startsWith("https") && !tab.url.startsWith("file")) {
      resolve(""); return;
    }
    chrome.scripting.executeScript({
      target: { tabId: tab.id, allFrames: true },
      func: () => { try { return window.getSelection()?.toString() || ""; } catch (_) { return ""; } }
    }, (results) => {
      if (chrome.runtime.lastError) { resolve(""); return; }
      const text = (results && results[0] && results[0].result) || "";
      resolve(text.trim());
    });
  });
}

chrome.contextMenus.onClicked.addListener((info, tab) => {
  // PDF / 无选区：通过 scripting 读取
  if (!info.selectionText) {
    getSelectionViaScripting(tab).then((selection) => {
      if (!selection) return;
      loadSettings((settings) => {
        const srcLang = settings.srcLang;
        const tgtLang = settings.tgtLang;
        handleContextAction(info.menuItemId, selection, srcLang, tgtLang, tab);
      });
    });
    return;
  }
  const selection = info.selectionText.trim();
  if (!selection) return;
  loadSettings((settings) => {
    handleContextAction(info.menuItemId, selection, settings.srcLang, settings.tgtLang, tab);
  });
});

function handleContextAction(menuItemId, selection, srcLang, tgtLang, tab) {
  if (menuItemId === "winocr-translate") {
    translate(selection, srcLang, tgtLang).then((r) => {
      const tgt = r && r.tgt ? r.tgt : "";
      injectBubble(tab, selection, tgt || "", "winocr");
    }).catch(() => {
      injectBubble(tab, selection, "（翻译失败：翻译引擎不可用）", "winocr");
    });
  } else if (menuItemId === "winocr-collect") {
    collect(selection, "", [], srcLang, tgtLang).then((r) => {
      const text = r && r.ok
        ? (r.cached ? "已缓存到本地（待同步）" : "已采集到生词本")
        : "采集失败";
      injectBubble(tab, selection, text, "winocr");
    }).catch(() => {
      injectBubble(tab, selection, "（采集失败：桌面端未连接？）", "winocr");
    });
  }
}

// 向指定 tab 注入气泡（通过 scripting.executeScript 注入临时内容脚本）
function injectBubble(tab, src, tgt, engine) {
  if (!tab || !tab.id) return;
  chrome.scripting.executeScript({
    target: { tabId: tab.id },
    func: (srcText, tgtText, eng) => {
      // 清理旧气泡
      const old = document.getElementById('winocr-pdf-bubble');
      if (old) old.remove();

      const b = document.createElement('div');
      b.id = 'winocr-pdf-bubble';
      b.style.cssText = 'position:fixed;top:50%;left:50%;transform:translate(-50%,-50%);z-index:2147483647;max-width:420px;background:#fff;border:1px solid #e2e8f0;border-radius:14px;box-shadow:0 12px 36px rgba(30,41,55,.22);font-family:system-ui,sans-serif;font-size:14px;color:#1e293b;overflow:hidden';
      b.innerHTML = '<div style="padding:10px 14px;background:#f8fafc;border-bottom:1px solid #e2e8f0;display:flex;justify-content:space-between;font-size:12px;color:#64748b"><span>WinOCR 划词助手</span><span style="cursor:pointer;color:#94a3b8" onclick="document.getElementById(\'winocr-pdf-bubble\').remove()">✕</span></div><div style="padding:12px 14px"><div style="font-weight:600;margin-bottom:8px;word-break:break-word"></div><div style="color:#475569;word-break:break-word"></div></div><div style="padding:8px 14px;border-top:1px solid #f1f5f9;display:flex;gap:8px"><button id="wb-pdf-copy" style="border:1px solid #e2e8f0;background:#fff;border-radius:6px;padding:5px 14px;font-size:12px;cursor:pointer">复制</button><button id="wb-pdf-collect" style="border:1px solid #e2e8f0;background:#fff;border-radius:6px;padding:5px 14px;font-size:12px;cursor:pointer">采集生词</button></div>';
      b.querySelectorAll('.wb-src').forEach(() => {});
      const body = b.querySelector('div:nth-child(2)');
      const srcDiv = body.querySelector('div:first-child');
      const tgtDiv = body.querySelector('div:last-child');
      srcDiv.textContent = srcText;
      tgtDiv.textContent = tgtText || '（点击翻译获取译文）';
      document.body.appendChild(b);

      document.getElementById('wb-pdf-copy').addEventListener('click', () => {
        navigator.clipboard.writeText(tgtText || srcText).then(() => {
          const btn = document.getElementById('wb-pdf-copy');
          if (btn) btn.textContent = '已复制 ✓';
        });
      });
    },
    args: [src, tgt, engine]
  }, () => {
    if (chrome.runtime.lastError) {
      // PDF plugin 区域 scripting 也不可用，尝试 sendMessage 到已有 content script
      chrome.tabs.sendMessage(tab.id, { type: "show-bubble", src, tgt, engine }).catch(() => {});
    }
  });
}

// ---------- 快捷键命令 ----------
// MV3 service worker 要求顶级同步注册；Edge 154 偶发 chrome.commands 未就绪，
// 用 try-catch + 延迟重试兜底。
function handleCommand(command) {
  console.log('[winocr] hotkey triggered:', command);
  if (command === "capture-translate") {
    // 对标主程序 Alt+Q：按下后进入十字线框选，松开框选区域才做 OCR+翻译。
    // 流程：快捷键 → captureVisibleTab 截全视口 → 注入框选 UI → 用户框选 →
    //       canvas 按坐标裁剪 → ocr_translate → 结果气泡显示在选框旁。
    chrome.tabs.query({ active: true, currentWindow: true }, (tabs) => {
      const tab = tabs[0];
      if (!tab || !tab.id) return;
      chrome.tabs.captureVisibleTab(tab.windowId, { format: 'png' }, (dataUrl) => {
        if (chrome.runtime.lastError || !dataUrl) {
          console.warn('[winocr] captureVisibleTab failed:', (chrome.runtime.lastError && chrome.runtime.lastError.message) || 'no data');
          return;
        }
        // 主路径：交给 content script 启动框选并复用划词气泡显示结果（界面/功能一致）。
        // content script 不可注入时（如 PDF viewer）回退到 scripting 注入的框选 UI。
        chrome.tabs.sendMessage(tab.id, { type: 'start-region-select', img: dataUrl }, (resp) => {
          if (chrome.runtime.lastError) {
            console.log('[winocr] content script 不可用，回退 scripting 注入框选');
            injectRegionSelect(tab, dataUrl);
          } else {
            console.log('[winocr] 已转交 content script 框选');
          }
        });
      });
    });
  } else if (command === "cycle-engine") {
    cycleEngine();
  }
}

// 循环切换翻译引擎（Alt+Shift+R）
function cycleEngine() {
  const ORDER = ["host", "sf", "mymemory", "ollama", "browser"];
  const LABELS = { host: "桌面端 WinOCR", sf: "SiliconFlow", mymemory: "MyMemory", ollama: "Ollama", browser: "浏览器内置" };
  loadSettings((settings) => {
    const cur = settings.engine || "host";
    let idx = ORDER.indexOf(cur);
    if (idx < 0) idx = 0;
    const next = ORDER[(idx + 1) % ORDER.length];
    chrome.storage.sync.set({ engine: next }, () => {
      if (chrome.runtime.lastError) return;
      console.log('[winocr] engine cycled:', cur, '->', next);
      chrome.tabs.query({ active: true, currentWindow: true }, (tabs) => {
        const tab = tabs[0];
        if (!tab || !tab.id) return;
        chrome.scripting.executeScript({
          target: { tabId: tab.id },
          func: (eng, label) => {
            const old = document.getElementById('winocr-engine-toast');
            if (old) old.remove();
            const t = document.createElement('div');
            t.id = 'winocr-engine-toast';
            t.style.cssText = 'position:fixed;top:18px;left:50%;transform:translateX(-50%);z-index:2147483647;background:#1e293b;color:#fff;padding:10px 18px;border-radius:10px;font:15px/1.4 system-ui,"Microsoft YaHei",sans-serif;box-shadow:0 8px 24px rgba(0,0,0,.25);white-space:nowrap';
            t.textContent = '翻译引擎已切换：' + label + '（' + eng + '）';
            document.body.appendChild(t);
            setTimeout(() => {
              t.style.transition = 'opacity .4s';
              t.style.opacity = '0';
              setTimeout(() => t.remove(), 420);
            }, 2200);
          },
          args: [next, LABELS[next] || next]
        });
      });
    });
  });
}

// 向页面注入十字线框选 UI（自包含函数，经 scripting.executeScript 序列化注入，
// 不可引用外部闭包变量）。dataUrl 为按下快捷键瞬间截下的全视口 PNG。
function injectRegionSelect(tab, dataUrl) {
  if (!tab || !tab.id) return;
  chrome.scripting.executeScript({
    target: { tabId: tab.id },
    func: regionSelectFunc,
    args: [dataUrl]
  }, () => {
    if (chrome.runtime.lastError) {
      console.warn('[winocr] region select inject failed:', chrome.runtime.lastError.message);
    }
  });
}

function regionSelectFunc(dataUrl) {
  if (document.getElementById('winocr-region-overlay')) return; // 防重复注入

  // 扩展上下文失效时静默失败，避免 "Extension context invalidated" 崩溃
  function ctxAlive() {
    try { return !!(typeof chrome !== 'undefined' && chrome.runtime && chrome.runtime.id); }
    catch (e) { return false; }
  }
  function safeSend(msg, cb) {
    if (!ctxAlive()) { if (typeof cb === 'function') { try { cb(null); } catch (_) {} } return; }
    try {
      chrome.runtime.sendMessage(msg, function (r) {
        if (chrome.runtime.lastError) { if (typeof cb === 'function') { try { cb(null); } catch (_) {} } return; }
        if (typeof cb === 'function') { try { cb(r); } catch (_) {} }
      });
    } catch (e) {
      if (typeof cb === 'function') { try { cb(null); } catch (_) {} }
    }
  }

  const dpr = window.devicePixelRatio || 1;

  // PDF / 受限页面 fallback：结果气泡与划词 showBubble 完全一致（复用 .winocr-bubble 样式与全套按钮）
  let bx = 0, by = 0, bh = 0;                                  // 选区几何，用于气泡定位
  let bubbleEl = null;                                         // 当前结果气泡
  let lastBubblePos = null;                                    // 用户拖动后的气泡位置（会话内记忆）
  let curSrc = '', curTgt = '', curSrcLang = 'auto', curTgtLang = 'zh';
  let currentUtterance = null;

  // ---- 遮罩 + 选框 + 十字线 + 尺寸标签 ----
  const ov = document.createElement('div');
  ov.id = 'winocr-region-overlay';
  ov.style.cssText = 'position:fixed;left:0;top:0;width:100vw;height:100vh;z-index:2147483647;cursor:crosshair;user-select:none';

  const box = document.createElement('div');
  box.style.cssText = 'position:absolute;display:none;border:1px solid #1677ff;box-shadow:0 0 0 100000px rgba(0,0,0,.25);background:transparent;pointer-events:none';

  const hline = document.createElement('div');
  hline.style.cssText = 'position:absolute;left:0;width:100%;height:1px;background:rgba(22,119,255,.65);pointer-events:none';

  const vline = document.createElement('div');
  vline.style.cssText = 'position:absolute;top:0;width:1px;height:100%;background:rgba(22,119,255,.65);pointer-events:none';

  const tag = document.createElement('div');
  tag.style.cssText = 'position:absolute;display:none;background:#1677ff;color:#fff;font:11px/1.7 system-ui,sans-serif;padding:0 6px;border-radius:3px;pointer-events:none;white-space:nowrap';

  ov.appendChild(box); ov.appendChild(hline); ov.appendChild(vline); ov.appendChild(tag);
  document.documentElement.appendChild(ov);

  let sx = 0, sy = 0, dragging = false, done = false;

  function cleanup() {
    done = true;
    window.removeEventListener('keydown', onKey, true);
    ov.remove();
  }
  function onKey(e) {
    if (e.key === 'Escape') cleanup();
  }
  window.addEventListener('keydown', onKey, true);

  ov.addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    dragging = true;
    sx = e.clientX; sy = e.clientY;
    box.style.display = 'block';
    tag.style.display = 'block';
    e.preventDefault();
    e.stopPropagation();
  });

  ov.addEventListener('mousemove', (e) => {
    if (done) return;
    hline.style.top = e.clientY + 'px';
    vline.style.left = e.clientX + 'px';
    if (!dragging) return;
    const x = Math.min(sx, e.clientX), y = Math.min(sy, e.clientY);
    const w = Math.abs(e.clientX - sx), h = Math.abs(e.clientY - sy);
    box.style.left = x + 'px'; box.style.top = y + 'px';
    box.style.width = w + 'px'; box.style.height = h + 'px';
    tag.textContent = w + ' \u00d7 ' + h;
    tag.style.left = Math.max(0, x + w - 64) + 'px';
    tag.style.top = Math.max(0, y - 22) + 'px';
  });

  ov.addEventListener('mouseup', (e) => {
    if (!dragging || done) return;
    const x = Math.min(sx, e.clientX), y = Math.min(sy, e.clientY);
    const w = Math.abs(e.clientX - sx), h = Math.abs(e.clientY - sy);
    cleanup();
    if (w < 5 || h < 5) return; // 点击/拖动过小视为取消

    // 按设备像素比从全视口截图中裁剪选区
    const img = new Image();
    img.onload = () => {
      const c = document.createElement('canvas');
      c.width = Math.max(1, Math.round(w * dpr));
      c.height = Math.max(1, Math.round(h * dpr));
      const ctx = c.getContext('2d');
      ctx.drawImage(img,
        Math.round(x * dpr), Math.round(y * dpr), c.width, c.height,
        0, 0, c.width, c.height);
      const b64 = c.toDataURL('image/jpeg', 0.85).split(',')[1];
      bx = x; by = y; bh = h;
      renderBubble(null);
      safeSend({ type: 'ocr_translate', img: b64 }, (r) => {
        renderBubble(r);
      });
    };
    img.src = dataUrl;
  });

  // ---------- 结果气泡（与划词 showBubble 完全一致） ----------
  function injectWinocrStyles() {
    if (document.getElementById('winocr-styles')) return;
    const style = document.createElement('style');
    style.id = 'winocr-styles';
    style.textContent = WINOCR_BUBBLE_CSS;
    (document.head || document.documentElement).appendChild(style);
  }

  function clearSpeakingState() {
    if (!bubbleEl) return;
    const tts = bubbleEl.querySelectorAll('.wb-actions button.tts');
    if (tts) tts.forEach(function (b) { b.classList.remove('speaking'); });
  }
  function pickVoiceForLang(lang) {
    if (!('speechSynthesis' in window)) return null;
    const voices = speechSynthesis.getVoices();
    const prefix = (lang || 'en').split('-')[0].toLowerCase();
    return voices.find(function (v) { return v.lang && v.lang.toLowerCase().startsWith(prefix); }) || null;
  }
  function speakText(text, lang, btn) {
    if (!text || !text.trim()) return;
    if (!('speechSynthesis' in window)) { alert('当前浏览器不支持朗读（Web Speech API）'); return; }
    window.speechSynthesis.cancel();
    const u = new SpeechSynthesisUtterance(text);
    u.lang = lang; u.rate = 0.9;
    const voice = pickVoiceForLang(lang); if (voice) u.voice = voice;
    if (btn) btn.classList.add('speaking');
    u.onend = function () { currentUtterance = null; clearSpeakingState(); };
    u.onerror = function () { currentUtterance = null; clearSpeakingState(); };
    currentUtterance = u; window.speechSynthesis.speak(u);
  }
  function stopSpeaking() { if ('speechSynthesis' in window) window.speechSynthesis.cancel(); currentUtterance = null; clearSpeakingState(); }

  const RESULT_CSS = '.winocr-bubble .wb-collect-result{margin-top:10px;border:1px solid #a7f3d0;background:#ecfdf5;border-radius:8px;overflow:hidden}.winocr-bubble .wb-collect-result.err{border-color:#fca5a5;background:#fef2f2}.winocr-bubble .wb-collect-result .wb-cr-head{padding:8px 12px;background:#10b981;color:#fff;font-size:14px;font-weight:600}.winocr-bubble .wb-collect-result.err .wb-cr-head{background:#ef4444}.winocr-bubble .wb-collect-result .wb-cr-row{display:flex;gap:8px;padding:6px 12px;font-size:14px;line-height:1.5}.winocr-bubble .wb-collect-result .wb-cr-k{flex:0 0 44px;color:#059669;font-weight:600}.winocr-bubble .wb-collect-result .wb-cr-v{flex:1;color:#064e3b;word-break:break-word}.winocr-bubble .wb-collect-result.err .wb-cr-k{color:#b91c1c}.winocr-bubble .wb-collect-result.err .wb-cr-v{color:#7f1d1d}.winocr-bubble .wb-collect-result .wb-cr-tip{padding:6px 12px 10px;font-size:11px;color:#047857}.winocr-bubble .wb-collect-result .wb-cr-words{display:flex;flex-wrap:wrap;gap:4px 6px}.winocr-bubble .wb-collect-result .wb-cr-word{display:inline-block;background:#a7f3d0;color:#065f46;border-radius:4px;padding:1px 6px;font-size:12px;font-weight:600}';

  function injectResultStyles() {
    if (document.getElementById('winocr-result-styles')) return;
    const style = document.createElement('style');
    style.id = 'winocr-result-styles';
    style.textContent = RESULT_CSS;
    (document.head || document.documentElement).appendChild(style);
  }

  function escapeHtml(s) {
    return String(s == null ? '' : s)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
  }

  function showCollectResult(word, tgt, tag, ok, error, resp, cached) {
    if (!bubbleEl) return;
    injectResultStyles();
    const body = bubbleEl.querySelector('.wb-body'); if (!body) return;
    const existing = bubbleEl.querySelector('.wb-collect-result'); if (existing) existing.remove();
    const div = document.createElement('div');
    div.className = 'wb-collect-result' + (ok ? '' : ' err');
    const head = ok
      ? (cached ? '✓ 已缓存到本地（待同步）' : '✓ 已采集到生词本')
      : '✗ 采集失败';
    let rows;
    if (ok) {
      const tgtHtml = tgt ? escapeHtml(tgt) : '<span style="color:#94a3b8">（无译文）</span>';
      const tagHtml = (tag && tag.trim()) ? escapeHtml(tag) : '未分组';
      rows = '<div class="wb-cr-row"><span class="wb-cr-k">原文</span><span class="wb-cr-v">' + escapeHtml(word) + '</span></div>' +
             '<div class="wb-cr-row"><span class="wb-cr-k">译文</span><span class="wb-cr-v">' + tgtHtml + '</span></div>' +
             '<div class="wb-cr-row"><span class="wb-cr-k">分组</span><span class="wb-cr-v">' + tagHtml + '</span></div>';
      // 过滤出的生词明细（host 端 ingest_text 拆分/过滤后实际入库的词；缓存时仅原始词）
      const items = (resp && resp.data && resp.data.items) || [];
      const count = (resp && resp.data && typeof resp.data.count === 'number') ? resp.data.count : items.length;
      if (items && items.length > 0) {
        const lis = items.map(function (it) {
          return '<span class="wb-cr-word">' + escapeHtml(it.word) + '</span>';
        }).join('');
        rows += '<div class="wb-cr-row"><span class="wb-cr-k">过滤生词</span><span class="wb-cr-v wb-cr-words">' + lis + '</span></div>';
      } else {
        rows += '<div class="wb-cr-row"><span class="wb-cr-k">过滤生词</span><span class="wb-cr-v">' + (count > 0 ? count + ' 个词' : '（无新词）') + '</span></div>';
      }
      if (cached) {
        rows += '<div class="wb-cr-tip" style="background:#fffbeb;border-top:1px solid #fde68a;color:#92400e">桌面端 WinOCR 未运行，生词已暂存在浏览器本地；下次桌面端启动并连接后自动补传。</div>';
      } else {
        rows += '<div class="wb-cr-tip">共采集 ' + count + ' 个词 · 在桌面端 WinOCR「采录」页查看与复习</div>';
      }
    } else {
      rows = '<div class="wb-cr-row"><span class="wb-cr-k">原因</span><span class="wb-cr-v">' + escapeHtml(error || '未知错误') + '</span></div>';
    }
    div.innerHTML = '<div class="wb-cr-head">' + head + '</div>' + rows;
    body.appendChild(div);
  }

  function onBubbleClick(e) {
    const btn = e.target.closest('button'); if (!btn) return;
    const act = btn.dataset.act;
    if (act === 'translate') {
      btn.disabled = true; btn.textContent = '翻译中…';
      safeSend({ type: 'translate', src: curSrc, srcLang: curSrcLang, tgtLang: curTgtLang }, function (rr) {
        btn.disabled = false; btn.textContent = '翻译';
        if (!bubbleEl) return;
        const t = bubbleEl.querySelector('.wb-tgt');
        if (t) t.textContent = (rr && rr.ok && rr.tgt) ? rr.tgt : '（翻译失败：' + ((rr && rr.error) || '未知错误') + '）';
      });
    } else if (act === 'collect') {
      btn.disabled = true;
      safeSend({ type: 'collect', src: curSrc, tgt: curTgt, tags: [], srcLang: curSrcLang, tgtLang: curTgtLang }, function (rr) {
        btn.disabled = false;
        if (rr && rr.ok) {
          btn.textContent = rr.cached ? '已缓存 ✓' : '已采集 ✓';
          showCollectResult(curSrc, curTgt, '', true, '', rr, rr.cached);
        } else {
          btn.textContent = '采集失败';
          showCollectResult(curSrc, curTgt, '', false, (rr && rr.error) || '未知错误', rr);
        }
      });
    } else if (act === 'copy') {
      navigator.clipboard.writeText(curTgt || curSrc).then(function () { btn.textContent = '已复制 ✓'; }).catch(function () {});
    } else if (act === 'tts-en' || act === 'tts-zh') {
      const textToSpeak = act === 'tts-en' ? curSrc : (curTgt || curSrc);
      const langToSpeak = act === 'tts-en' ? 'en-US' : 'zh-CN';
      speakText(textToSpeak, langToSpeak, btn);
    } else if (act === 'tts-stop') {
      stopSpeaking();
    } else if (act === 'close') {
      if (bubbleEl && bubbleEl.parentNode) bubbleEl.remove();
      bubbleEl = null;
    }
  }

  // 渲染/更新结果气泡。r===null 表示识别中；r 存在则填结果（沿用划词气泡结构）
  function renderBubble(r) {
    injectWinocrStyles();
    if (bubbleEl && bubbleEl.parentNode) bubbleEl.remove();
    bubbleEl = document.createElement('div');
    bubbleEl.className = 'winocr-bubble';
    const bw = Math.min(560, window.innerWidth - 16);
    // 与划词气泡一致：用户拖动过则新气泡出现在上次位置（clamp 视口内）
    let top, left;
    if (typeof lastBubblePos === 'object' && lastBubblePos) {
      left = Math.max(8, Math.min(lastBubblePos.left, window.innerWidth - bw - 8));
      top = Math.max(8, Math.min(lastBubblePos.top, window.innerHeight - 120));
    } else {
      top = by + bh + 8;
      if (top + 200 > window.innerHeight - 8) top = Math.max(8, by - 200 - 8);
      left = Math.min(Math.max(8, bx), window.innerWidth - bw - 8);
    }
    bubbleEl.style.left = left + 'px';
    bubbleEl.style.top = top + 'px';
    bubbleEl.style.maxWidth = bw + 'px';

    const connected = !r || (r && r.ok);
    bubbleEl.innerHTML = '<div class="wb-head"><span>WinOCR 划词助手</span><span><span class="wb-dot' + (connected ? '' : ' off') + '"></span>' + (connected ? '已连接' : '未连接') + '</span></div><div class="wb-engine" style="display:none"></div><div class="wb-body"><div class="wb-src"></div><div class="wb-tgt"></div></div><div class="wb-actions"><button data-act="translate" class="primary">翻译</button><button data-act="collect">采集生词</button><button data-act="copy">复制</button><button data-act="tts-en" class="tts" title="朗读原文（英文）">英</button><button data-act="tts-zh" class="tts" title="朗读译文（中文）">中</button><button data-act="tts-stop" class="tts-stop" title="停止朗读">■</button><button data-act="close">关闭</button></div>';

    const srcEl = bubbleEl.querySelector('.wb-src');
    const tgtEl = bubbleEl.querySelector('.wb-tgt');
    if (r === null || r === undefined) {
      if (srcEl) srcEl.textContent = '';
      if (tgtEl) tgtEl.textContent = '（OCR 识别中…）';
    } else if (r && r.ok) {
      curSrc = r.src_text || ''; curTgt = r.tgt || '';
      if (srcEl) srcEl.textContent = r.src_text || '';
      if (tgtEl) tgtEl.textContent = r.tgt || '（无译文）';
    } else {
      if (srcEl) srcEl.textContent = '';
      if (tgtEl) tgtEl.textContent = '（OCR 失败：' + ((r && r.error) || (chrome.runtime.lastError && chrome.runtime.lastError.message) || '未知错误') + '）';
    }
    // 显示翻译引擎状态
    const engEl = bubbleEl.querySelector('.wb-engine');
    if (engEl) {
      if (r && r.ok && (r.label || r.engine)) {
        const label = r.label || r.engine;
        const isHost = r.engine === 'host';
        const isLocalOcr = r.engine === 'local-ocr';
        engEl.textContent = '⚡ 引擎：' + label + ' · ' + (isHost ? '桌面端已连接' : (isLocalOcr ? '本地模型' : '在线引擎'));
        engEl.style.display = 'block';
      } else {
        engEl.style.display = 'none';
      }
    }
    bubbleEl.addEventListener('click', onBubbleClick);
    // 标题栏拖动（与划词气泡一致）：位置记忆到 lastBubblePos，OCR 气泡跨次出现位置一致
    const head = bubbleEl.querySelector('.wb-head');
    head.addEventListener('mousedown', (e) => {
      e.preventDefault();
      e.stopPropagation();
      let sx = e.clientX, sy = e.clientY;
      const rect = bubbleEl.getBoundingClientRect();
      const ox = sx - rect.left, oy = sy - rect.top;
      const onMove = (ev) => {
        if (!bubbleEl) return;
        const left2 = Math.max(0, Math.min(ev.clientX - ox, window.innerWidth - bubbleEl.offsetWidth));
        const top2 = Math.max(0, Math.min(ev.clientY - oy, window.innerHeight - 32));
        bubbleEl.style.left = left2 + 'px';
        bubbleEl.style.top = top2 + 'px';
        lastBubblePos = { left: left2, top: top2 };
      };
      const onUp = () => {
        document.removeEventListener('mousemove', onMove, true);
        document.removeEventListener('mouseup', onUp, true);
      };
      document.addEventListener('mousemove', onMove, true);
      document.addEventListener('mouseup', onUp, true);
    });
    document.documentElement.appendChild(bubbleEl);
  }

  const WINOCR_BUBBLE_CSS = '.winocr-bubble{position:fixed;z-index:2147483647;max-width:560px;background:#fff;border:1px solid #e2e8f0;border-radius:12px;box-shadow:0 8px 24px rgba(30,41,59,.18);font-family:"Segoe UI","Microsoft YaHei",system-ui,sans-serif;font-size:16px;line-height:1.6;color:#1e293b;overflow:hidden}.winocr-bubble .wb-head{display:flex;align-items:center;justify-content:space-between;padding:10px 16px;background:#f8fafc;border-bottom:1px solid #e2e8f0;font-size:14px;color:#64748b;cursor:move;user-select:none;-webkit-user-select:none}.winocr-bubble .wb-dot{display:inline-block;width:10px;height:10px;border-radius:50%;background:#10b981}.winocr-bubble .wb-dot.off{background:#ef4444}.winocr-bubble .wb-body{padding:14px 18px;max-height:56vh;overflow-y:auto;overscroll-behavior:contain;scrollbar-width:thin;scrollbar-color:#cbd5e1 transparent}.winocr-bubble .wb-body::-webkit-scrollbar{width:6px}.winocr-bubble .wb-body::-webkit-scrollbar-thumb{background:#cbd5e1;border-radius:3px}.winocr-bubble .wb-body::-webkit-scrollbar-thumb:hover{background:#94a3b8}.winocr-bubble .wb-body::-webkit-scrollbar-track{background:transparent}.winocr-bubble .wb-src{font-weight:600;color:#1e293b;font-size:17px;line-height:1.6;margin-bottom:8px;word-break:break-word}.winocr-bubble .wb-tgt{color:#475569;font-size:16px;line-height:1.7;word-break:break-word;min-height:24px}.winocr-bubble .wb-actions{display:flex;gap:8px;padding:10px 16px;border-top:1px solid #f1f5f9;flex-wrap:wrap}.winocr-bubble .wb-actions button{border:1px solid #e2e8f0;background:#fff;color:#475569;border-radius:8px;padding:8px 14px;font-size:15px;cursor:pointer}.winocr-bubble .wb-actions button:hover{background:#f1f5f9}.winocr-bubble .wb-actions button.primary{background:#6366f1;border-color:#6366f1;color:#fff}.winocr-bubble .wb-actions button.primary:hover{background:#4f46e5}.winocr-bubble .wb-actions button.copied{background:#10b981;border-color:#10b981;color:#fff}.winocr-bubble .wb-actions button:disabled{opacity:.6;cursor:wait}.winocr-bubble .wb-actions button.tts{width:38px;padding:7px 0;font-weight:700;text-align:center}.winocr-bubble .wb-actions button.tts.speaking{background:#6366f1;border-color:#6366f1;color:#fff}.winocr-bubble .wb-actions button.tts-stop{width:32px;padding:6px 0;text-align:center;font-size:13px}.winocr-bubble .wb-engine{padding:6px 16px;background:#eef2ff;color:#4338ca;border-bottom:1px solid #e0e7ff;font-size:12px;line-height:1.5;word-break:break-word}.winocr-bubble .wb-collect-status{margin-top:8px;padding:8px 12px;background:#d1fae5;color:#065f46;border-radius:6px;font-size:14px;font-weight:600;transition:opacity .4s}';
}

// 顶级同步注册（MV3 service worker 规范）
function registerHotkeys() {
  try {
    if (chrome.commands && chrome.commands.onCommand) {
      chrome.commands.onCommand.addListener(handleCommand);
      console.log('[winocr] hotkey listener registered');
      chrome.commands.getAll((cmds) => {
        console.log('[winocr] active commands:', JSON.stringify(cmds));
      });
    } else {
      console.warn('[winocr] chrome.commands not ready, retrying in 1s');
      setTimeout(registerHotkeys, 1000);
    }
  } catch (e) {
    console.warn('[winocr] registerHotkeys error:', e.message);
    setTimeout(registerHotkeys, 1000);
  }
}

// 翻译文本（优先走 host，否则在线引擎）
async function translateText(text, srcLang, tgtLang) {
  const r = await translate(text, srcLang, tgtLang);
  return r && r.tgt ? r.tgt : "";
}

// ---------- 启动 ----------
chrome.runtime.onStartup.addListener(() => {
  connect();
  startPing();
});

chrome.runtime.onInstalled.addListener(() => {
  connect();
  startPing();
});

// 首次安装时也尝试连接（service worker 可能随时被唤醒）
connect();
startPing();

// 注册快捷键监听（顶级同步注册，MV3 service worker 规范）
registerHotkeys();
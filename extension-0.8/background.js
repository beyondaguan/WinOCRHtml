// background.js — Service Worker：侧栏行为 + 快捷键 + 右键菜单 + Native Messaging 桥
// 翻译触发统一走 translateAndPush()：翻译 → 记录 → 推给侧栏显示。
try { importScripts('js/common.js'); } catch (e) {}

// --------------------- Native Messaging（可选，默认关闭） ---------------------
// 原生宿主 com.winocr_host 仅用于「外部截图 / 记录回传」。未注册时 connectNative 会报
// "Specified native messaging host not found"。因此：
//   1) 默认不连接（设置里 nativeHost=false），纯浏览器使用完全不触发该报错；
//   2) 失败时读取 chrome.runtime.lastError（抑制 "Unchecked runtime.lastError" 日志）；
//   3) 不再无限重试（最多 3 次、5s 间隔），避免刷屏。
// 注意：必须与安装器注册的键名 com.winocr_host（下划线）一致
const NATIVE_HOST = 'com.winocr_host';
let port = null;
let nativeEnabled = false;
let nativeTried = 0;
let nativeLastError = '';

// ---------------- 常量定义 ----------------
const NATIVE_MAX_RETRIES = 3;
const NATIVE_RETRY_INTERVAL_MS = 5000;
const NATIVE_TIMEOUT_MS = 8000;
const PANEL_OPEN_DELAY_MS = 500;
const SETTINGS_SYNC_DELAY_MS = 500;
let nativeGen = 0;          // 代次：忽略"被主动替换/断开"的陈旧 disconnect
let hostInfo = null;        // 原生宿主自报的状态（收到 hello 后填充：模式 / 生效热键 / pid）

// --------------------- 本地 OCR 往返（借宿主的 PP-OCRv6 引擎） ---------------------
// 云端视觉模型实测 103~121s（PaddleOCR-VL）/输出不稳（DeepSeek-OCR）；
// 宿主侧本地 PP-OCRv6 只要 1.2~2.2s。所以浏览器里的图片 OCR 也走宿主。
const ocrWaiters = new Map();   // id -> {resolve, reject, timer}
let ocrSeq = 0;

// --------------------- 宿主 → 扩展的「配置回读」 ---------------------
// 独立模式与扩展是两份互不同步的配置：winocr_config.json（宿主用）
// 与 chrome.storage（扩展用）。用户常只填了其中一边，于是出现
// 「扩展里连上了、截图却说没 key」。选项页的「从本地配置导入」走这条往返。
const settingsWaiters = new Map();   // id -> {resolve, reject, timer}
let settingsSeq = 0;

function hostSettings(timeoutMs) {
  return new Promise((resolve, reject) => {
    if (!port) {
      return reject(new Error('原生宿主未连接：先在选项页勾选「连接原生宿主」并保存，再重新加载扩展'));
    }
    const id = 'c' + (++settingsSeq);
    const timer = setTimeout(() => {
      settingsWaiters.delete(id);
      reject(new Error('宿主读取配置超时'));
    }, timeoutMs || NATIVE_TIMEOUT_MS);
    settingsWaiters.set(id, { resolve: resolve, reject: reject, timer: timer });
    try {
      port.postMessage({ type: 'settings.get', id: id });
    } catch (e) {
      clearTimeout(timer); settingsWaiters.delete(id); reject(e);
    }
  });
}

function failSettingsWaiters(reason) {
  settingsWaiters.forEach((w) => { clearTimeout(w.timer); w.reject(new Error(reason)); });
  settingsWaiters.clear();
}

function ocrViaHost(dataUrl, timeoutMs) {
  return new Promise((resolve, reject) => {
    if (!port) return reject(new Error('原生宿主未连接：请在扩展选项勾选「连接原生宿主」并重新加载扩展'));
    const id = 'o' + (++ocrSeq);
    const timer = setTimeout(() => {
      ocrWaiters.delete(id);
      reject(new Error('宿主 OCR 超时（本地模型可能在加载）'));
    }, timeoutMs || 60000);
    ocrWaiters.set(id, { resolve: resolve, reject: reject, timer: timer });
    try {
      port.postMessage({ type: 'ocr', id: id, dataUrl: dataUrl });
    } catch (e) {
      clearTimeout(timer); ocrWaiters.delete(id); reject(e);
    }
  });
}

function failOcrWaiters(reason) {
  ocrWaiters.forEach((w) => { clearTimeout(w.timer); w.reject(new Error(reason)); });
  ocrWaiters.clear();
}

function readLastError() {  // 读取即视为已处理，抑制 Unchecked 日志
  const e = chrome.runtime.lastError;
  return e ? (e.message || String(e)) : '';
}

function disconnectNative() {
  nativeGen++;
  if (port) { try { port.disconnect(); } catch (e) {} port = null; }
}

function connectNative(force) {
  if (!nativeEnabled && !force) return;
  disconnectNative();
  const gen = ++nativeGen;
  let p = null;
  try {
    p = chrome.runtime.connectNative(NATIVE_HOST);
  } catch (e) {
    nativeLastError = String((e && e.message) || e);
    return;
  }
  const err0 = readLastError();          // 兜底：个别实现同步置错
  if (err0) nativeLastError = err0;
  port = p;
  nativeLastError = '';
  p.onMessage.addListener((msg) => {
    if (!msg) return;
    if (msg.type === 'record') WINOCR.addRecord(msg.record);
    else if (msg.type === 'hello') hostInfo = msg;   // 宿主自报：已桥接 + 生效热键 + OCR 引擎
    else if (msg.type === 'settings.current') {
      const w = settingsWaiters.get(msg.id);
      if (!w) return;
      clearTimeout(w.timer); settingsWaiters.delete(msg.id);
      w.resolve(msg.settings || {});
    }
    else if (msg.type === 'ocr.result') {
      const w = ocrWaiters.get(msg.id);
      if (!w) return;
      clearTimeout(w.timer); ocrWaiters.delete(msg.id);
      if (msg.error) w.reject(new Error(msg.error)); else w.resolve(msg.text || '');
    }
    // 其他类型（如 toast）由原生宿主自行弹窗处理
  });
  p.onDisconnect.addListener(() => {
    const err = readLastError();         // 必须读，否则控制台报 Unchecked runtime.lastError
    if (gen !== nativeGen) return;       // 已主动断开/替换，忽略
    if (err) nativeLastError = err;
    port = null;
    failOcrWaiters('原生宿主已断开：' + (err || '连接关闭'));
    failSettingsWaiters('原生宿主已断开：' + (err || '连接关闭'));
    nativeTried++;
    if (nativeEnabled && err && nativeTried <= NATIVE_MAX_RETRIES) setTimeout(() => connectNative(true), NATIVE_RETRY_INTERVAL_MS);
  });
  WINOCR.getSettings().then((s) => {
    if (port && gen === nativeGen) { try { port.postMessage({ type: 'settings', settings: s }); } catch (e) {} }
  });
}

function nativeStatus() {
  return { enabled: nativeEnabled, connected: !!port, error: nativeLastError, tried: nativeTried, host: hostInfo };
}

// 依据设置开启/关闭原生宿主连接（设置变化时也走这里）
async function syncNativeSetting() {
  let want = false;
  try { want = !!(await WINOCR.getSettings()).nativeHost; } catch (e) {}
  if (want === nativeEnabled) return;
  nativeEnabled = want;
  nativeTried = 0;
  nativeLastError = '';
  if (want) connectNative(true); else disconnectNative();
}
syncNativeSetting();

try {
  chrome.storage.onChanged.addListener((ch, area) => {
    if (area === 'local' && ch && ch.winocr_settings_v1) syncNativeSetting();
  });
} catch (e) {}

// --------------------- 侧栏 ---------------------
// 点扩展图标即开侧栏（每次 SW 启动都重设，防止休眠后行为丢失）
function setPanelBehavior() {
  if (!chrome.sidePanel) return;
  try {
    const r = chrome.sidePanel.setPanelBehavior({ openPanelOnActionClick: true });
    if (r && r.catch) r.catch(() => {});
  } catch (e) {}
}
setPanelBehavior();

// 打开侧栏（必须在用户手势的同步链里调用，不能放到 await 之后）
function openPanel(tabId) {
  if (!chrome.sidePanel || !tabId) return;
  try {
    const r = chrome.sidePanel.open({ tabId: tabId });
    if (r && r.catch) r.catch(() => {});
  } catch (e) {}
}

// --------------------- 工具 ---------------------
function qTabs(sel) {
  return new Promise((resolve) => {
    try { chrome.tabs.query(sel, (t) => resolve(t || [])); } catch (e) { resolve([]); }
  });
}

function sendToTab(tabId, msg) {
  return new Promise((resolve, reject) => {
    try {
      chrome.tabs.sendMessage(tabId, msg, (res) => {
        const le = chrome.runtime.lastError;
        if (le) return reject(new Error(le.message));
        resolve(res);
      });
    } catch (e) { reject(e); }
  });
}

// 读取页面选中：优先问 content script，失败再用 scripting 兜底
async function getSelectionFromTab(tabId) {
  try {
    const r = await sendToTab(tabId, { type: 'winocr.getSelection' });
    if (r && typeof r.text === 'string') return r.text.trim();
  } catch (e) {}
  try {
    const res = await chrome.scripting.executeScript({
      target: { tabId: tabId },
      func: () => String((window.getSelection && window.getSelection().toString) ? window.getSelection().toString() : '')
    });
    return ((res && res[0] && res[0].result) || '').trim();
  } catch (e) { return ''; }
}

// 向侧栏推送（侧栏未开时静默忽略）
function pushPanel(msg) {
  try { chrome.runtime.sendMessage(msg, () => { void chrome.runtime.lastError; }); } catch (e) {}
}

// 无原生侧栏时（旧内核 / 弹窗版），把结果就地弹成页面气泡，保证「看得见」
function showPageBubble(tabId, original, translation, error) {
  if (!tabId || chrome.sidePanel) return;
  sendToTab(tabId, { type: 'winocr.showBubble', original: original || '', translation: translation || '', error: error || '' }).catch(() => {});
}

// 翻译一段文本 → 记录 → 推给侧栏（无侧栏时就地弹气泡）
async function translateAndPush(text, sourceUrl, tabId) {
  const t = (text || '').trim();
  if (!t) {
    const em = '没有可翻译的文本（请先选中文字）';
    pushPanel({ type: 'panel-result', original: '', translation: '', error: em });
    showPageBubble(tabId, '', '', em);
    return;
  }
  const s = await WINOCR.getSettings();
  pushPanel({ type: 'panel-input', text: t });
  try {
    const out = await WINOCR.translate(t, {
      engine: s.engine, sfKey: s.sfKey, sfUrl: s.sfUrl, sfModel: s.sfModel,
      mymemoryEmail: s.mymemoryEmail,
      ollamaUrl: s.ollamaUrl, ollamaModel: s.ollamaModel,
      srcLang: s.srcLang, tgtLang: s.tgtLang
    });
    await WINOCR.addRecord({ type: 'text', source: sourceUrl || '', original: t, translation: out });
    pushPanel({ type: 'panel-result', original: t, translation: out });
    showPageBubble(tabId, t, out, '');
  } catch (e) {
    const em = (e && e.message) || String(e);
    pushPanel({ type: 'panel-result', original: t, translation: '', error: em });
    showPageBubble(tabId, t, '', em);
  }
}

// --------------------- 快捷键 ---------------------
chrome.commands.onCommand.addListener((cmd) => {
  if (cmd === 'open-panel') {
    qTabs({ active: true, currentWindow: true }).then((tabs) => { if (tabs[0]) openPanel(tabs[0].id); });
  } else if (cmd === 'translate-selection') {
    (async () => {
      const tabs = await qTabs({ active: true, currentWindow: true });
      const tab = tabs[0];
      if (!tab) return;
      openPanel(tab.id);                                  // 先开侧栏（同步手势；弹窗版为 no-op）
      const text = await getSelectionFromTab(tab.id);
      await translateAndPush(text, tab.url || '', tab.id);
    })();
  }
});

// --------------------- 右键菜单 ---------------------
function ensureMenu() {
  try {
    chrome.contextMenus.removeAll(() => {
      chrome.contextMenus.create(
        { id: 'winocr-translate', title: 'WinOCR：翻译选中', contexts: ['selection'] },
        () => void chrome.runtime.lastError
      );
    });
  } catch (e) {}
}
ensureMenu();   // SW 每次启动确保菜单存在（幂等）

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId !== 'winocr-translate' || !info.selectionText) return;
  if (tab && tab.id) openPanel(tab.id);   // 先同步开面板以保住用户手势
  translateAndPush(info.selectionText, (tab && tab.url) || '', (tab && tab.id) || 0);
});

chrome.runtime.onInstalled.addListener(() => {
  setPanelBehavior();
  ensureMenu();
  syncNativeSetting();   // 仅当设置里开启了原生宿主才连接
});

// --------------------- 消息：外部请求转发 / 设置同步 / 截图 ---------------------
// 后台转发外部请求：SW 持有 host_permissions，可绕开页面 CSP/CORS。
// 服务两类上游：SiliconFlow（POST，OpenAI 兼容）与 MyMemory（GET）。
chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (!msg) return;
  if (msg.type === 'sf.chat') {
    (async () => {
      try {
        const isGet = String(msg.method || 'POST').toUpperCase() === 'GET';
        const init = { method: isGet ? 'GET' : 'POST', headers: {} };
        if (!isGet) {
          init.headers = { 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + (msg.key || '') };
          init.body = JSON.stringify(msg.body);
        }
        const r = await fetch(msg.url, init);
        let j = null; try { j = await r.json(); } catch (e) {}
        // ① OpenAI 兼容（SiliconFlow）
        if (j && j.choices) {
          const ch = j.choices[0];
          sendResponse({ content: ((ch && ch.message && ch.message.content) || '').trim() });
          return;
        }
        // ② MyMemory（GET，取 responseData.translatedText）
        if (j && j.responseData) {
          const det = String(j.responseDetails || '');
          if (/MYMEMORY WARNING|ALL AVAILABLE FREE TRANSLATIONS/i.test(det) || j.quotaFinished) {
            sendResponse({ error: 'MyMemory 今日免费额度已用尽' });
            return;
          }
          if (j.responseStatus && Number(j.responseStatus) !== 200) {
            sendResponse({ error: 'MyMemory 返回 ' + j.responseStatus + (det ? '：' + det : '') });
            return;
          }
          sendResponse({ content: String(j.responseData.translatedText || '') });
          return;
        }
        // ③ 其它错误（含 SF 的 {code:20012,message:"Model does not exist"} 外壳，
        //    它 HTTP 状态可能是 200，必须看 code，否则会被当成"成功但空内容"）
        const emsg = j && (j.message || (j.error && j.error.message));
        const code = (j && typeof j.code === 'number') ? j.code : null;
        const bizErr = !!(j && j.error) || (code !== null && code !== 0 && code !== 200);
        if (!r.ok || bizErr) {
          let msg = emsg || ('HTTP ' + r.status);
          if (code === 20012 || /does not exist/i.test(String(msg))) {
            msg = '模型名不存在（' + msg + '）。硅基流动上要写全称：' +
              'PaddlePaddle/PaddleOCR-VL-1.5 / Qwen/Qwen3-8B';
          }
          sendResponse({ error: msg });
          return;
        }
        sendResponse({ content: '' });
      } catch (e) {
        sendResponse({ error: String((e && e.message) || e) });
      }
    })();
    return true; // 异步响应
  }
  if (msg.type === 'syncSettings') {
    syncNativeSetting();                                   // 开关变化即时生效
    const push = () => { if (port) { try { port.postMessage({ type: 'settings', settings: msg.settings }); } catch (e) { console.warn('[WinOCR] 设置同步到宿主失败:', e); } } };
    push();
    setTimeout(push, SETTINGS_SYNC_DELAY_MS);   // 若刚触发连接，端口就绪后再补发一次（保证热键等设置真送达）
  }
  if (msg.type === 'capture' && port) { try { port.postMessage({ type: 'capture' }); } catch (e) {} }
  // 浏览器内截图：捕获当前标签页可见区域（无需原生宿主）
  if (msg.type === 'capture.visibleTab') {
    (async () => {
      try {
        const tabId = msg.tabId || (sender.tab && sender.tab.id);
        if (!tabId) { sendResponse({ error: '无 tabId' }); return; }
        const dataUrl = await new Promise((res, rej) => {
          chrome.tabs.captureVisibleTab(
            chrome.windows ? chrome.windows.WINDOW_ID_CURRENT : undefined,
            { format: 'png' },
            (d) => { const e = chrome.runtime.lastError; if (e) rej(new Error(e.message)); else res(d); }
          );
        });
        sendResponse({ dataUrl: dataUrl });
      } catch (e) {
        sendResponse({ error: String((e && e.message) || e) });
      }
    })();
    return true;
  }
  // 把可见区域截图交给 content script 弹选区遮罩
  if (msg.type === 'capture.visibleTab.start') {
    (async () => {
      try {
        const tabs = await qTabs({ active: true, currentWindow: true });
        const tab = tabs[0];
        if (!tab) { sendResponse({ error: '无活动标签页' }); return; }
        const dataUrl = await new Promise((res, rej) => {
          chrome.tabs.captureVisibleTab(undefined, { format: 'png' }, (d) => {
            const e = chrome.runtime.lastError; if (e) rej(new Error(e.message)); else res(d);
          });
        });
        // 把截图发给 content script 让用户框选区域
        await sendToTab(tab.id, { type: 'winocr.regionSelect', dataUrl: dataUrl });
        sendResponse({ ok: true });
      } catch (e) {
        sendResponse({ error: String((e && e.message) || e) });
      }
    })();
    return true;
  }
  if (msg.type === 'ocr.viaHost') {
    // 浏览器侧的图片 OCR 交给宿主的本地 PP-OCRv6 引擎（快、离线、零 key）
    ocrViaHost(msg.dataUrl, msg.timeoutMs).then(
      (text) => sendResponse({ text: text }),
      (e) => sendResponse({ error: (e && e.message) || String(e) })
    );
    return true;   // 异步响应
  }
  if (msg.type === 'winocr.openConfigDir') {
    // 选项页「打开本地配置文件」：让宿主在资源管理器里选中 winocr_config.json
    if (!port) { sendResponse({ error: '原生宿主未连接：先勾选「连接原生宿主」并保存，再重新加载扩展' }); return; }
    try { port.postMessage({ type: 'openConfigDir' }); sendResponse({ ok: true }); }
    catch (e) { sendResponse({ error: String((e && e.message) || e) }); }
    return;
  }
  if (msg.type === 'native.status') { sendResponse(nativeStatus()); return; }
  if (msg.type === 'winocr.hostSettings') {
    // 选项页「从本地配置导入」：把宿主正在生效的配置读回来
    if (!port) { sendResponse({ error: '原生宿主未连接：先勾选「连接原生宿主」并保存，再重新加载扩展' }); return; }
    hostSettings(msg.timeoutMs).then(
      (settings) => sendResponse({ settings: settings, host: hostInfo }),
      (e) => sendResponse({ error: (e && e.message) || String(e) })
    );
    return true;   // 异步响应
  }
  if (msg.type === 'native.reconnect') {
    // enable 由调用方传入（设置页勾选框），不在此永久改写设置
    nativeEnabled = !!msg.enable;
    nativeTried = 0;
    nativeLastError = '';
    if (!nativeEnabled) { disconnectNative(); sendResponse(nativeStatus()); return; }
    connectNative(true);
    setTimeout(() => sendResponse(nativeStatus()), PANEL_OPEN_DELAY_MS);   // 稍等以捕获连接结果
    return true;
  }
});

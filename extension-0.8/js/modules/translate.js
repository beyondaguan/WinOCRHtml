// js/modules/translate.js — 翻译引擎（SF/MyMemory/Ollama/Browser）
// 所有翻译逻辑集中在此模块。

import { extAlive } from './storage.js';

// ---------------- 常量定义 ----------------
const SF_DEFAULT_URL = 'https://api.siliconflow.cn/v1';
const MM_ENDPOINT = 'https://api.mymemory.translated.net/get';
const OLLAMA_DEFAULT_URL = 'http://localhost:11434';
const MM_LANG_MAP = {
  zh: 'zh-CN', en: 'en-GB', ja: 'ja-JP', ko: 'ko-KR', fr: 'fr-FR',
  de: 'de-DE', es: 'es-ES', ru: 'ru-RU', it: 'it-IT', pt: 'pt-PT',
  ar: 'ar-SA', th: 'th-TH', vi: 'vi-VN'
};
const MM_CHUNK_SIZE = 450;
const SMART_ROUTE_SHORT_LEN = 200;
const TRANSLATE_CACHE_MAX = 500;

// ---------------- 免费模型清单 ----------------
export const FREE_TEXT_MODELS = [
  { id: 'Qwen/Qwen3-8B', note: '快（实测 2.5s）· 翻译推荐' },
  { id: 'deepseek-ai/DeepSeek-R1-0528-Qwen3-8B', note: '推理模型，慢 4 倍（9.5–12.3s）' }
];

export const FREE_OCR_MODELS = [
  { id: 'PaddlePaddle/PaddleOCR-VL-1.5', note: '文字对但极慢（103–121s）' },
  { id: 'deepseek-ai/DeepSeek-OCR', note: '输出不稳定（会退化成乱码）' }
];

// ---------------- 翻译缓存（内存 LRU） ----------------
const TRANSLATE_CACHE = new Map();

function cacheKey(text, opts) {
  return (opts.srcLang || 'en') + '→' + (opts.tgtLang || 'zh') + ':' + String(text || '').trim();
}

function cacheGet(text, opts) {
  const k = cacheKey(text, opts);
  const v = TRANSLATE_CACHE.get(k);
  if (v) {
    TRANSLATE_CACHE.delete(k);
    TRANSLATE_CACHE.set(k, v);
  }
  return v;
}

function cacheSet(text, opts, value) {
  const k = cacheKey(text, opts);
  if (TRANSLATE_CACHE.size >= TRANSLATE_CACHE_MAX) {
    const first = TRANSLATE_CACHE.keys().next().value;
    if (first) TRANSLATE_CACHE.delete(first);
  }
  TRANSLATE_CACHE.set(k, value);
}

// ---------------- 后台消息通道检测 ----------------
const CAN_MSG = (typeof window !== 'undefined') && (typeof chrome !== 'undefined') &&
  !!(chrome.runtime && chrome.runtime.id && typeof chrome.runtime.sendMessage === 'function');

// ---------------- SiliconFlow 翻译 ----------------
function sfEndpoint(opts) {
  return (opts.sfUrl || SF_DEFAULT_URL).replace(/\/+$/, '') + '/chat/completions';
}

function friendlySfError(status, msg, code) {
  const m = String(msg || '');
  if (code === 20012 || /does not exist/i.test(m)) {
    return '模型名不存在（' + m + '）。硅基流动上模型 id 必须写全称：' +
      'OCR 用 PaddlePaddle/PaddleOCR-VL-1.5，翻译用 Qwen/Qwen3-8B（扩展「选项」里可改）';
  }
  if (status === 401 || status === 403 || /invalid_token|Token is invalid/i.test(m)) {
    return 'SiliconFlow Key 无效或未填写，请在扩展「选项」里填写正确的 Key';
  }
  if (status === 429) return 'SiliconFlow 限流（429），请稍后重试';
  if (status === 400) return '请求被拒（400）：' + (m || '参数错误');
  return m || ('HTTP ' + status);
}

function sfChatViaBackground(url, key, body) {
  return new Promise((resolve, reject) => {
    if (!CAN_MSG || !extAlive()) return reject(new Error('no-runtime'));
    try {
      chrome.runtime.sendMessage({ type: 'sf.chat', url: url, key: key, body: body }, (res) => {
        const le = chrome.runtime.lastError;
        if (le) return reject(new Error(le.message));
        if (!res) return reject(new Error('后台无响应'));
        if (res.error) return reject(new Error(res.error));
        resolve(res.content || '');
      });
    } catch (e) { reject(e); }
  });
}

export async function sfChat(opts, body) {
  const key = (opts && opts.sfKey) || '';
  if (!key) throw new Error('未填写 SiliconFlow Key（请在扩展「选项」中填写）');
  const url = sfEndpoint(opts || {});

  const callViaFetch = async (b) => {
    const ctl = (typeof AbortController !== 'undefined') ? new AbortController() : null;
    const timer = ctl ? setTimeout(() => ctl.abort(), 60000) : null;
    let r, j = null;
    try {
      r = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + key },
        body: JSON.stringify(b),
        signal: ctl ? ctl.signal : undefined
      });
      try { j = await r.json(); } catch (e) { /* 非 JSON */ }
    } finally { if (timer) clearTimeout(timer); }
    const emsg = j && (j.message || (j.error && j.error.message));
    const code = (j && typeof j.code === 'number') ? j.code : null;
    const bizErr = !!(j && j.error) || (code !== null && code !== 0 && code !== 200);
    if (!r.ok || bizErr) {
      const err = new Error(friendlySfError(r.status, emsg, code));
      err.status = r.status; err.code = code;
      throw err;
    }
    const ch = j && j.choices && j.choices[0];
    return ((ch && ch.message && ch.message.content) || '').trim();
  };

  const call = async (b) => {
    try {
      return await callViaFetch(b);
    } catch (e) {
      if (e && e.name === 'AbortError') throw new Error('请求超时（60 秒），请检查网络');
      if (e instanceof TypeError && CAN_MSG) return await sfChatViaBackground(url, key, b);
      throw e;
    }
  };

  try {
    return await call(body);
  } catch (e) {
    if (e && e.status === 400 && body && Object.prototype.hasOwnProperty.call(body, 'enable_thinking')) {
      const b2 = Object.assign({}, body); delete b2.enable_thinking;
      return await call(b2);
    }
    throw e;
  }
}

// ---------------- MyMemory 翻译 ----------------
function mmLang(code) {
  const c = String(code || '').toLowerCase();
  if (!c || c === 'auto') return 'en-GB';
  return MM_LANG_MAP[c] || code;
}

function decodeEntities(s) {
  return String(s == null ? '' : s)
    .replace(/&#x([0-9a-fA-F]+);/g, (m, h) => { try { return String.fromCodePoint(parseInt(h, 16)); } catch (e) { return m; } })
    .replace(/&#(\d+);/g, (m, d) => { try { return String.fromCodePoint(parseInt(d, 10)); } catch (e) { return m; } })
    .replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>').replace(/&nbsp;/g, ' ').replace(/&amp;/g, '&');
}

function mmChunks(text, max) {
  const out = []; let buf = '';
  String(text == null ? '' : text).split(/(\n+)/).forEach((seg) => {
    if ((buf + seg).length <= max) { buf += seg; return; }
    if (buf) { out.push(buf); buf = ''; }
    if (seg.length <= max) { buf = seg; return; }
    let s = seg;
    while (s.length > max) {
      let cut = Math.max(s.lastIndexOf('. ', max), s.lastIndexOf('。', max), s.lastIndexOf('！', max), s.lastIndexOf(' ', max));
      if (cut >= max || cut < max * 0.5) cut = max - 1;
      out.push(s.slice(0, cut + 1)); s = s.slice(cut + 1);
    }
    buf = s;
  });
  if (buf) out.push(buf);
  return out.filter((x) => x.trim());
}

function mmViaBackground(url) {
  return new Promise((resolve, reject) => {
    try {
      chrome.runtime.sendMessage({ type: 'sf.chat', url: url, method: 'GET', key: '', body: null }, (res) => {
        const le = chrome.runtime.lastError;
        if (le) return reject(new Error(le.message));
        if (!res) return reject(new Error('后台无响应'));
        if (res.error) return reject(new Error(res.error));
        resolve(decodeEntities(res.content || ''));
      });
    } catch (e) { reject(e); }
  });
}

async function mmOnce(text, src, tgt, email) {
  let url = MM_ENDPOINT + '?q=' + encodeURIComponent(text) +
    '&langpair=' + encodeURIComponent(src + '|' + tgt);
  if (email) url += '&de=' + encodeURIComponent(email);
  let r, j = null;
  try {
    r = await fetch(url, { method: 'GET' });
    try { j = await r.json(); } catch (e) { /* 非 JSON */ }
  } catch (e) {
    if (e instanceof TypeError && CAN_MSG && extAlive()) {
      return await mmViaBackground(url);
    }
    throw new Error('MyMemory 网络不可达：' + ((e && e.message) || e));
  }
  const det = String((j && j.responseDetails) || '');
  if (/MYMEMORY WARNING|ALL AVAILABLE FREE TRANSLATIONS/i.test(det) || (j && j.quotaFinished)) {
    throw new Error('MyMemory 今日免费额度已用尽（匿名约 5000 字符/天）；在扩展「选项」里填一个邮箱可提升到约 50000 字符/天。');
  }
  if (!r.ok || !j || (j.responseStatus && Number(j.responseStatus) !== 200)) {
    throw new Error('MyMemory 返回 ' + ((j && j.responseStatus) || r.status) + (det ? '：' + det : ''));
  }
  const out = decodeEntities(j.responseData && j.responseData.translatedText);
  if (/MYMEMORY WARNING|ALL AVAILABLE FREE TRANSLATIONS/i.test(out)) {
    throw new Error('MyMemory 今日免费额度已用尽；可在「选项」里填邮箱提升到约 50000 字符/天。');
  }
  return out.trim();
}

export async function translateMyMemory(text, opts) {
  opts = opts || {};
  const src = mmLang(opts.srcLang), tgt = mmLang(opts.tgtLang);
  if (src === tgt) throw new Error('源语言与目标语言相同，无需翻译');
  const parts = [];
  for (const c of mmChunks(text, MM_CHUNK_SIZE)) parts.push(await mmOnce(c, src, tgt, opts.mymemoryEmail));
  const out = parts.join('');
  if (!out) throw new Error('MyMemory 返回空内容');
  return out;
}

// ---------------- Ollama 翻译 ----------------
function ollamaBase(opts) {
  return ((opts && opts.ollamaUrl) || OLLAMA_DEFAULT_URL).replace(/\/+$/, '');
}

export async function ollamaHealth(opts) {
  const url = ollamaBase(opts);
  try {
    const r = await fetch(url + '/api/tags', { method: 'GET' });
    if (!r.ok) return { ok: false, error: 'HTTP ' + r.status };
    const j = await r.json();
    const models = (j && j.models) || [];
    return { ok: true, models: models.map((m) => ({ name: m.name, size: m.size, details: m.details })) };
  } catch (e) {
    return { ok: false, error: '无法连接 Ollama（' + ((e && e.message) || e) + '）—— 请确认已安装并启动 Ollama' };
  }
}

export async function listOllamaModels(opts) {
  const h = await ollamaHealth(opts);
  if (!h.ok) throw new Error(h.error);
  return h.models;
}

export async function translateOllama(text, opts) {
  opts = opts || {};
  const model = opts.ollamaModel || '';
  if (!model) throw new Error('未选择本地模型：请到「选项」里检测并选择一个 Ollama 模型');
  const url = ollamaBase(opts) + '/api/chat';
  const sys = 'You are a professional translator. Translate the user text into ' + (opts.tgtLang || 'zh') +
    '. Keep medical/technical terms accurate. Output ONLY the translation, no commentary, no quotes.';
  const body = {
    model: model,
    messages: [{ role: 'system', content: sys }, { role: 'user', content: text }],
    stream: false,
    options: { temperature: 0.3 }
  };
  const ctl = (typeof AbortController !== 'undefined') ? new AbortController() : null;
  const timer = ctl ? setTimeout(() => ctl.abort(), 120000) : null;
  try {
    const r = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      signal: ctl ? ctl.signal : undefined
    });
    if (!r.ok) throw new Error('Ollama 返回 HTTP ' + r.status);
    const j = await r.json();
    const out = (j && j.message && j.message.content) || '';
    if (!out.trim()) throw new Error('Ollama 模型返回空内容');
    return out.trim();
  } catch (e) {
    if (e && e.name === 'AbortError') throw new Error('Ollama 翻译超时（120 秒）');
    if (e instanceof TypeError) throw new Error('无法连接 Ollama 服务：请确认已启动（ollama serve）');
    throw e;
  } finally {
    if (timer) clearTimeout(timer);
  }
}

// ---------------- SiliconFlow 翻译（单条） ----------------
export async function translateSF(text, opts) {
  opts = opts || {};
  const sys = 'You are a professional translator. Translate the user text into ' + (opts.tgtLang || 'zh') +
    '. Keep medical/technical terms accurate. Output ONLY the translation, no commentary, no quotes.';
  const body = {
    model: opts.sfModel || 'Qwen/Qwen3-8B',
    messages: [{ role: 'system', content: sys }, { role: 'user', content: text }],
    temperature: 0.3,
    max_tokens: 4096,
    enable_thinking: false
  };
  const out = await sfChat(opts, body);
  if (!out) throw new Error('模型返回空内容（可能被思考模式占用），请重试或改小文本');
  return out;
}

// ---------------- 统一翻译入口（含术语预处理 + 智能路由） ----------------
export async function translate(text, opts) {
  opts = opts || {};
  const t = String(text || '');
  if (!t.trim()) return '';
  const cached = cacheGet(t, opts);
  if (cached) return cached;

  const engine = opts.engine === 'deeplx' ? 'sf' : (opts.engine || 'sf');
  const tgt = opts.tgtLang || 'zh';
  const src = opts.srcLang || 'en';

  // 术语库预处理
  const settings = opts._settings || {};
  const termsEnabled = settings.termEnabled || false;
  const { getTermsMap, applyTerms, restoreTerms } = await import('./terms.js');
  const termsMap = termsEnabled ? getTermsMap(settings) : {};
  const { text: processedText, restoreMap } = applyTerms(t, termsMap);

  const tryFreeFirst = engine === 'sf' && processedText.length < SMART_ROUTE_SHORT_LEN;

  let result = null;
  if (tryFreeFirst) {
    try {
      result = await translateMyMemory(t, opts);
    } catch (e) {
      console.warn('[WinOCR] 免费引擎失败，回落 SF:', e.message);
    }
  }
  if (result == null) {
    if (engine === 'mymemory') {
      result = await translateMyMemory(processedText, opts);
    } else if (engine === 'ollama') {
      result = await translateOllama(processedText, opts);
    } else if (engine === 'browser' && typeof globalThis.Translator !== 'undefined' && typeof globalThis.Translator.availability === 'function') {
      try {
        const avail = await globalThis.Translator.availability({ sourceLanguage: src, targetLanguage: tgt });
        if (avail === 'available') {
          const tr = await globalThis.Translator.create({ sourceLanguage: src, targetLanguage: tgt });
          result = await tr.translate(processedText);
        }
      } catch (e) { console.warn('[WinOCR] 浏览器内置翻译不可用，回落 SF:', e); }
    }
  }
  if (result == null) result = await translateSF(processedText, opts);

  if (restoreMap) result = restoreTerms(result, restoreMap);

  if (result) cacheSet(t, opts, result);
  return result;
}

// ---------------- AI 多轮对话 ----------------
export async function chat(messages, opts) {
  opts = opts || {};
  const engine = opts.engine || 'sf';
  if (engine === 'ollama') {
    const model = opts.ollamaModel || '';
    if (!model) throw new Error('未选择本地模型');
    const url = ollamaBase(opts) + '/api/chat';
    const body = { model: model, messages: messages, stream: false };
    const ctl = (typeof AbortController !== 'undefined') ? new AbortController() : null;
    const timer = ctl ? setTimeout(() => ctl.abort(), 120000) : null;
    try {
      const r = await fetch(url, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body), signal: ctl ? ctl.signal : undefined
      });
      if (!r.ok) throw new Error('Ollama HTTP ' + r.status);
      const j = await r.json();
      const out = (j && j.message && j.message.content) || '';
      if (!out.trim()) throw new Error('模型返回空内容');
      return out.trim();
    } finally {
      if (timer) clearTimeout(timer);
    }
  }
  const body = {
    model: opts.sfModel || 'Qwen/Qwen3-8B',
    messages: messages,
    temperature: 0.7, max_tokens: 4096, enable_thinking: false
  };
  const out = await sfChat(opts, body);
  if (!out) throw new Error('模型返回空内容');
  return out;
}

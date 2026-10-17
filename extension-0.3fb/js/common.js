// js/common.js — 共享核心（被 content.js / sidepanel.js / options.js / background.js 复用）
// 设计：纯浏览器内闭环（选区气泡、翻译、OCR、日积累 MD、多出口导出）。
// 翻译默认走自带 SiliconFlow key（BYOK, 模型 Qwen/Qwen3-8B）；
// 浏览器内置 Translator API（零 key、离线）仅 Edge/Chrome 138+ 且能访问微软服务时可用，否则自动回落 SF。
(function (global) {
  'use strict';
  const W = (global.WINOCR = {});

  // ---------------- 存储 ----------------
  const K_HIST = 'winocr_history_v1';
  const K_SET = 'winocr_settings_v1';

  // ---------------- 常量定义 ----------------
  const SF_URL = 'https://api.siliconflow.cn/v1';  // 向后兼容旧引用（SF_DEFAULT_URL 在下方翻译区定义）
  const MM_CHUNK_SIZE = 450;
  const MM_LANG_MAP = {
    zh: 'zh-CN', en: 'en-GB', ja: 'ja-JP', ko: 'ko-KR', fr: 'fr-FR',
    de: 'de-DE', es: 'es-ES', ru: 'ru-RU', it: 'it-IT', pt: 'pt-PT',
    ar: 'ar-SA', th: 'th-TH', vi: 'vi-VN'
  };
  const SF_TEMPERATURE = 0.3;
  const SF_MAX_TOKENS = 4096;
  const SF_OCR_MAX_TOKENS = 2048;
  const BROWSER_MIN_VERSION = 138;
  const HASH_SEED = 5381;
  const CRC32_POLY = 0xEDB88320;
  const ZIP_LOCAL_HEADER = 0x04034b50;
  const ZIP_CENTRAL_HEADER = 0x02014b50;
  const ZIP_END_HEADER = 0x06054b50;
  const TRANSLATE_TIMEOUT_MS = 60000;
  const OCR_TIMEOUT_MS = 240000;
  const BUBBLE_DEBOUNCE_MS = 220;
  const BUBBLE_MAX_WIDTH = 350;
  const BUBBLE_MIN_HEIGHT = 110;
  const BUBBLE_AUTO_CLOSE_MS = 1600;
  const BUBBLE_RESTORE_MS = 1200;
  const Z_INDEX_MAX = 2147483647;

  // ---------------- 免费模型清单（2026-09-20 实测） ----------------
  // 与原生宿主 winocr_host.py 里的 FREE_TEXT_MODELS / FREE_OCR_MODELS 保持一致。
  // 实测数据（同一张截图）：本地 PP-OCRv6 1.2–2.2s/置信度0.98；
  //   PaddleOCR-VL 103–121s（对但有前导幻觉）；DeepSeek-OCR 不稳定（会整段退化乱码）；
  //   Qwen3-8B 翻译 2.5s；R1-0528 翻译 9.5–12.3s（思考占 400+ 字）。
  W.FREE_TEXT_MODELS = [
    { id: 'Qwen/Qwen3-8B', note: '快（实测 2.5s）· 翻译推荐' },
    { id: 'deepseek-ai/DeepSeek-R1-0528-Qwen3-8B', note: '推理模型，慢 4 倍（9.5–12.3s）' }
  ];
  W.FREE_OCR_MODELS = [
    { id: 'PaddlePaddle/PaddleOCR-VL-1.5', note: '文字对但极慢（103–121s）' },
    { id: 'deepseek-ai/DeepSeek-OCR', note: '输出不稳定（会退化成乱码）' }
  ];

  W.DEFAULT_SETTINGS = {
    engine: 'sf',                 // 网页划词翻译：'sf' | 'mymemory'（免费免 key）| 'browser'（内置, 需 138+）
    // Alt+Q 截图 OCR 之后的翻译引擎（原生宿主使用；浏览器内置引擎在宿主里用不了，
    // 故这里没有 browser）：'sf' | 'mymemory' | 'none'（仅 OCR 不翻译）
    translateEngine: 'sf',
    sfKey: '',
    sfUrl: 'https://api.siliconflow.cn/v1',
    sfModel: 'Qwen/Qwen3-8B',
    // 注意：硅基流动上必须带 `PaddlePaddle/` 前缀，写成 `PaddleOCR-VL-1.5` 会报
    // 20012 "Model does not exist"（normalizeOcrModel 会把旧值自动纠正过来）。
    sfOcrModel: 'PaddlePaddle/PaddleOCR-VL-1.5',
    // OCR 引擎：'local' 本地 PP-OCRv6（离线·零 key·~1.5s，截图首选）
    //           'sf'   云端 SiliconFlow 视觉模型（慢，但能处理照片/手写/复杂版面）
    ocrEngine: 'local',
    localOcrTier: 'tiny',         // 'tiny'（6.6MB，已内置）| 'medium'（需另放 133MB 模型）
    srcLang: 'en',
    tgtLang: 'zh',
    displayMode: 'inline',        // 页面译文形态：'inline' 原地替换原文（默认）| 'bar' 常驻翻译栏 | 'bubble' 临时气泡
    nativeHost: false,            // 是否连接原生宿主 com.winocr_host（仅外部截图记录回传需要；默认关）
    hotkey: 'ctrl+shift+m',       // 原生宿主「截图」热键（扩展设置页可自由录入并同步给宿主）
    quitHotkey: 'ctrl+alt+q',     // 原生宿主「退出」热键（不能用裸 Esc，那会劫持全系统的 Esc）
    mymemoryEmail: '',            // MyMemory 可选邮箱：填了每日额度从 5000 提升到 50000 字符
    // 本地大模型（Ollama）：完全离线、隐私、零 key；需用户自行安装运行 Ollama
    ollamaUrl: 'http://localhost:11434',
    ollamaModel: '',               // 选中的本地模型名（如 qwen2.5:7b）；由「检测模型」填充
    // 术语库：翻译前预处理，保证专业术语一致性
    termEnabled: false,             // 是否启用术语库替换
    customTerms: {},               // 用户自定义术语 { "source_term": "target_term", ... }
    builtinTerms: 'medical',       // 内置术语库：'medical' | 'tech' | 'law' | 'finance' | 'all'
    exportTargets: { folder: true, zip: false, obsidian: false, lexiang: false },
    obsidianUrl: 'http://127.0.0.1:27123',
    obsidianKey: '',
    lexiangEndpoint: '',
    lexiangToken: ''
  };

  // 扩展上下文是否仍有效。
  // 扩展被重新加载 / 更新 / 禁用后，已打开页面里注入的旧 content script 会失效，
  // 再调 chrome.* 会抛 "Extension context invalidated" → 这里统一兜住。
  function extAlive() {
    try { return !!(typeof chrome !== 'undefined' && chrome.runtime && chrome.runtime.id); }
    catch (e) { return false; }
  }
  W.extAlive = extAlive;

  function store() {
    try {
      if (!extAlive()) return null;
      return (chrome.storage && chrome.storage.local) ? chrome.storage.local : null;
    } catch (e) { return null; }
  }

  // ---------------- 模型名归一化 ----------------
  // 硅基流动的模型 id 必须完全一致（含 `PaddlePaddle/`、`deepseek-ai/` 前缀），
  // 少写前缀会得到 20012 "Model does not exist"。这里把常见简写/历史错误值纠正回来，
  // 也让「曾经存过错误值」的旧设置自动痊愈。
  const OCR_ALIASES = {
    'paddleocr-vl-1.5': 'PaddlePaddle/PaddleOCR-VL-1.5',
    'paddleocr-vl': 'PaddlePaddle/PaddleOCR-VL-1.5',
    'paddleocr': 'PaddlePaddle/PaddleOCR-VL-1.5',
    'deepseek-ocr': 'deepseek-ai/DeepSeek-OCR'
  };
  function normalizeOcrModel(name) {
    const n = String(name == null ? '' : name).trim();
    if (!n) return W.DEFAULT_SETTINGS.sfOcrModel;
    return OCR_ALIASES[n.toLowerCase()] || n;
  }
  W.normalizeOcrModel = normalizeOcrModel;

  W.getSettings = function () {
    return new Promise((resolve) => {
      const s = store();
      if (!s) return resolve(Object.assign({}, W.DEFAULT_SETTINGS));
      try {
        s.get(K_SET, (o) => {
          void chrome.runtime.lastError;   // 读掉，避免 Unchecked runtime.lastError
          const merged = Object.assign({}, W.DEFAULT_SETTINGS, (o && o[K_SET]) || {});
          merged.sfOcrModel = normalizeOcrModel(merged.sfOcrModel);   // 自动纠正旧值
          // DeepLX 已下线（国内访问 DeepL 源站需代理且常被 429 限流）：旧值一律回落到 sf
          if (merged.engine === 'deeplx') merged.engine = 'sf';
          if (merged.translateEngine === 'deeplx') merged.translateEngine = 'sf';
          resolve(merged);
        });
      } catch (e) { resolve(Object.assign({}, W.DEFAULT_SETTINGS)); }
    });
  };
  W.setSettings = function (s) {
    return new Promise((resolve) => {
      const st = store();
      if (!st) return resolve(false);
      try {
        st.set({ [K_SET]: s }, () => { void chrome.runtime.lastError; resolve(true); });
      } catch (e) { resolve(false); }
    });
  };
  W.getHistory = function () {
    return new Promise((resolve) => {
      const s = store();
      if (!s) return resolve([]);
      try {
        s.get(K_HIST, (o) => { void chrome.runtime.lastError; resolve((o && o[K_HIST]) || []); });
      } catch (e) { resolve([]); }
    });
  };
  function hash(s) { let h = 5381; for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) >>> 0; return h.toString(36); }

  // 记录去重：同一来源+同一原文（或同一 OCR 文本）只存一次
  W.addRecord = async function (rec) {
    const list = await W.getHistory();
    const id = rec.id || (rec.source + ':' + hash(rec.original || '') + ':' + hash(rec.ocr || ''));
    if (list.some((r) => r.id === id)) return list;
    const item = Object.assign(
      { id, ts: new Date().toISOString(), type: 'text', source: '', original: '', translation: '', terms: [], image: '' },
      rec, { id }
    );
    list.push(item);
    const st = store();
    if (st) { try { st.set({ [K_HIST]: list }, () => void chrome.runtime.lastError); } catch (e) {} }
    return list;
  };
  W.clearHistory = function () {
    return new Promise((resolve) => {
      const st = store();
      if (!st) return resolve(false);
      try { st.set({ [K_HIST]: [] }, () => { void chrome.runtime.lastError; resolve(true); }); }
      catch (e) { resolve(false); }
    });
  };

  // ---------------- 翻译 ----------------
  // 重要：浏览器内置 Translator API 只有 Edge/Chrome 138+ 且能访问微软服务时才可用，
  // 旧内核 / 国内网络下不可用 → 一律回落到 SiliconFlow（BYOK）。故默认引擎为 'sf'。
  const SF_DEFAULT_URL = 'https://api.siliconflow.cn/v1';

  // 是否可向后台 Service Worker 转发（SW 有 host_permissions，可绕开页面 CSP/CORS 限制）。
  // Service Worker 自身没有 window，故该值为 false，天然避免自我递归。
  const CAN_MSG = (typeof window !== 'undefined') && (typeof chrome !== 'undefined') &&
    !!(chrome.runtime && chrome.runtime.id && typeof chrome.runtime.sendMessage === 'function');

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

  // 兜底：把请求交给后台 Service Worker 发出（绕开页面 CSP / CORS 变动）
  function sfChatViaBackground(url, key, body) {
    return new Promise((resolve, reject) => {
      if (!CAN_MSG || !extAlive()) return reject(new Error('no-runtime'));
      try {
        chrome.runtime.sendMessage({ type: 'sf.chat', url: url, key: key, body: body }, (res) => {
          const le = chrome.runtime.lastError;   // 必读：否则报 Unchecked runtime.lastError
          if (le) return reject(new Error(le.message));
          if (!res) return reject(new Error('后台无响应'));
          if (res.error) return reject(new Error(res.error));
          resolve(res.content || '');
        });
      } catch (e) { reject(e); }
    });
  }

  // 统一调用 SF /chat/completions；返回 content 文本。
  // 首选 content script 内直接 fetch（SF 返回 ACAO:*，实测可跨域）；
  // 若因页面 CSP / 跨域异常而抛 TypeError，则回退走后台 SW 转发。
  async function sfChat(opts, body) {
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
      // SF 的业务错误有两种外壳：{error:{...}} 和 {code:20012,message:"..."}。
      // 后者 HTTP 状态可能仍是 200，若不看 code 就会被当成"成功但内容为空"。
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
        // 网络层异常（TypeError：CORS/CSP/断网）→ 尝试后台转发一次
        if (e instanceof TypeError && CAN_MSG) return await sfChatViaBackground(url, key, b);
        throw e;
      }
    };

    try {
      return await call(body);
    } catch (e) {
      // 个别模型不接受 enable_thinking → 去掉该参数重试一次（400）
      if (e && e.status === 400 && body && Object.prototype.hasOwnProperty.call(body, 'enable_thinking')) {
        const b2 = Object.assign({}, body); delete b2.enable_thinking;
        return await call(b2);
      }
      throw e;
    }
  }

  // ---------------- MyMemory（免费 · 免 key · 国内可直连） ----------------
  // 文档：https://mymemory.translated.net/doc/spec.php
  // 实测（2026-09-20）：国内可直连，返回 Access-Control-Allow-Origin: *（浏览器可跨域直连）。
  // 匿名约 5000 字符/天；带 de=<email> 可提到约 50000。质量中等（翻译记忆+MT），适合当"零 key 兜底"。
  const MM_ENDPOINT = 'https://api.mymemory.translated.net/get';
  const MM_LANG = {
    zh: 'zh-CN', en: 'en-GB', ja: 'ja-JP', ko: 'ko-KR', fr: 'fr-FR', de: 'de-DE',
    es: 'es-ES', ru: 'ru-RU', it: 'it-IT', pt: 'pt-PT', ar: 'ar-SA', th: 'th-TH', vi: 'vi-VN'
  };
  function mmLang(code) {
    const c = String(code || '').toLowerCase();
    if (!c || c === 'auto') return 'en-GB';   // MyMemory 不支持 auto 探测，按 en 处理
    return MM_LANG[c] || code;
  }

  // MyMemory 会返回 HTML 实体（&quot; / &#39; 等），统一解码
  function decodeEntities(s) {
    return String(s == null ? '' : s)
      .replace(/&#x([0-9a-fA-F]+);/g, (m, h) => { try { return String.fromCodePoint(parseInt(h, 16)); } catch (e) { return m; } })
      .replace(/&#(\d+);/g, (m, d) => { try { return String.fromCodePoint(parseInt(d, 10)); } catch (e) { return m; } })
      .replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&lt;/g, '<')
      .replace(/&gt;/g, '>').replace(/&nbsp;/g, ' ').replace(/&amp;/g, '&');
  }

  // MyMemory 的 GET 对 q 有长度上限（约 500 字节）→ 按 450 字符切块，
  // 优先在换行处切，超长行再退到句末/空格。保留换行分隔符，拼接时不丢格式。
  function mmChunks(text, max) {
    const out = []; let buf = '';
    String(text == null ? '' : text).split(/(\n+)/).forEach((seg) => {
      if ((buf + seg).length <= max) { buf += seg; return; }
      if (buf) { out.push(buf); buf = ''; }
      if (seg.length <= max) { buf = seg; return; }
      let s = seg;
      while (s.length > max) {
        let cut = Math.max(s.lastIndexOf('. ', max), s.lastIndexOf('。', max), s.lastIndexOf('！', max), s.lastIndexOf(' ', max));
        // cut 必须落在 [max*0.5, max-1]，否则退化为硬切，避免出现 max+1 长度
        if (cut >= max || cut < max * 0.5) cut = max - 1;
        out.push(s.slice(0, cut + 1)); s = s.slice(cut + 1);
      }
      buf = s;
    });
    if (buf) out.push(buf);
    return out.filter((x) => x.trim());
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
        return await mmViaBackground(url);   // 页面 CSP 兜底：走后台 SW
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

  // 后台转发（兜底）：复用 sf.chat 通用通道；MyMemory 走 GET、无需 key。
  // 该函数仅用于 MyMemory，故结果统一做一次 HTML 实体解码。
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

  W.translateMyMemory = async function (text, opts) {
    opts = opts || {};
    const src = mmLang(opts.srcLang), tgt = mmLang(opts.tgtLang);
    if (src === tgt) throw new Error('源语言与目标语言相同，无需翻译');
    const parts = [];
    for (const c of mmChunks(text, 450)) parts.push(await mmOnce(c, src, tgt, opts.mymemoryEmail));
    const out = parts.join('');
    if (!out) throw new Error('MyMemory 返回空内容');
    return out;
  };

  // ---------------- 翻译缓存（内存，相同文本+语言对不重复请求） ----------------
  const TRANSLATE_CACHE = new Map();
  const TRANSLATE_CACHE_MAX = 500;
  function cacheKey(text, opts) {
    return (opts.srcLang || 'en') + '→' + (opts.tgtLang || 'zh') + ':' + String(text || '').trim();
  }
  function cacheGet(text, opts) {
    const k = cacheKey(text, opts);
    const v = TRANSLATE_CACHE.get(k);
    if (v) {
      // LRU：移到末尾
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

  // ---------------- 智能路由：短文本优先免费引擎，长文本用高质量引擎 ----------------
  // 规则（仅当用户引擎为 'sf' 时自动优化，避免违背用户显式选择）：
  //   - 文本长度 < 200 字符 → 先试 MyMemory（免费），失败再回落 SF
  //   - 文本长度 >= 200 字符 → 直接用 SF（长上下文/专业术语质量更好）
  //   - 用户显式选 'mymemory' / 'browser' → 尊重用户选择
  const SMART_ROUTE_SHORT_LEN = 200;

  W.translate = async function (text, opts) {
    opts = opts || {};
    const t = String(text || '');
    if (!t.trim()) return '';
    // 命中缓存直接返回
    const cached = cacheGet(t, opts);
    if (cached) return cached;

    // 兼容旧配置：已下线的 deeplx（国内需代理/会被 DeepL 限流）一律回落到 sf
    const engine = opts.engine === 'deeplx' ? 'sf' : (opts.engine || 'sf');
    const tgt = opts.tgtLang || 'zh';
    const src = opts.srcLang || 'en';

    // 术语库预处理：翻译前替换为占位符，翻译后还原
    const settings = opts._settings || {};
    const termsEnabled = settings.termEnabled || false;
    const termsMap = termsEnabled ? getTermsMap(settings) : {};
    const { text: processedText, restoreMap } = applyTerms(t, termsMap);

    // 智能路由：短文本 + SF 引擎 → 先试免费 MyMemory，失败回落 SF
    const tryFreeFirst = engine === 'sf' && processedText.length < SMART_ROUTE_SHORT_LEN;

    let result = null;
    if (tryFreeFirst) {
      try {
        result = await W.translateMyMemory(t, opts);
      } catch (e) {
        // 免费引擎失败（额度用尽/网络）→ 静默回落 SF
        console.warn('[WinOCR] 免费引擎失败，回落 SF:', e.message);
      }
    }
    if (result == null) {
      if (engine === 'mymemory') {
        result = await W.translateMyMemory(processedText, opts);
      } else if (engine === 'ollama') {
        result = await W.translateOllama(processedText, opts);
      } else if (engine === 'browser' && global.Translator && typeof global.Translator.availability === 'function') {
        try {
          const avail = await global.Translator.availability({ sourceLanguage: src, targetLanguage: tgt });
          if (avail === 'available') {
            const tr = await global.Translator.create({ sourceLanguage: src, targetLanguage: tgt });
            result = await tr.translate(processedText);
          }
        } catch (e) { console.warn('[WinOCR] 浏览器内置翻译不可用，回落 SF:', e); }
      }
    }
    if (result == null) result = await W.translateSF(processedText, opts);

    // 术语还原：把占位符替换回目标术语
    if (restoreMap) result = restoreTerms(result, restoreMap);

    if (result) cacheSet(t, opts, result);
    return result;
  };

  W.translateSF = async function (text, opts) {
    opts = opts || {};
    const sys = 'You are a professional translator. Translate the user text into ' + (opts.tgtLang || 'zh') +
      '. Keep medical/technical terms accurate. Output ONLY the translation, no commentary, no quotes.';
    const body = {
      model: opts.sfModel || 'Qwen/Qwen3-8B',
      messages: [{ role: 'system', content: sys }, { role: 'user', content: text }],
      temperature: 0.3,
      max_tokens: 4096,
      // Qwen3 系列在硅基流动上 enable_thinking 默认为 true，思考 token 会耗尽 max_tokens 导致正文为空 → 必须关闭
      enable_thinking: false
    };
    const out = await sfChat(opts, body);
    if (!out) throw new Error('模型返回空内容（可能被思考模式占用），请重试或改小文本');
    return out;
  };

  // ---------------- 本地大模型：Ollama（离线 · 隐私 · 零 key） ----------------
  // 通过 HTTP REST 调用本地运行的 Ollama 服务（默认 http://localhost:11434）。
  // 优势：完全离线、数据不出本机、无 API 费用；限制：依赖用户自行安装运行 Ollama。
  const OLLAMA_DEFAULT_URL = 'http://localhost:11434';

  function ollamaBase(opts) {
    return ((opts && opts.ollamaUrl) || OLLAMA_DEFAULT_URL).replace(/\/+$/, '');
  }

  // 健康检查：服务是否可达
  W.ollamaHealth = async function (opts) {
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
  };

  // 列出本地可用模型
  W.listOllamaModels = async function (opts) {
    const h = await W.ollamaHealth(opts);
    if (!h.ok) throw new Error(h.error);
    return h.models;
  };

  // Ollama 翻译：走 /api/chat（支持多轮），system prompt 指定翻译任务
  W.translateOllama = async function (text, opts) {
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
    const timer = ctl ? setTimeout(() => ctl.abort(), 120000) : null;  // 本地模型给 120s
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
  };

  // ---------------- PDF 文本提取（PDF.js，CDN 动态加载） ----------------
  // 从上传的 PDF 文件中提取每页文本，返回 [{page, text}]。
  // PDF.js 从 CDN 动态加载（扩展体积敏感，不 vendor）；离线时提示用户。
  const PDFJS_CDN = 'https://cdnjs.cloudflare.com/ajax/libs/pdf.js/3.11.174/pdf.min.js';
  const PDFJS_WORKER_CDN = 'https://cdnjs.cloudflare.com/ajax/libs/pdf.js/3.11.174/pdf.worker.min.js';
  let pdfjsLibPromise = null;

  function loadPdfJs() {
    if (pdfjsLibPromise) return pdfjsLibPromise;
    pdfjsLibPromise = new Promise((resolve, reject) => {
      if (typeof window !== 'undefined' && window.pdfjsLib) return resolve(window.pdfjsLib);
      const s = document.createElement('script');
      s.src = PDFJS_CDN;
      s.onload = () => {
        if (window.pdfjsLib) {
          window.pdfjsLib.GlobalWorkerOptions.workerSrc = PDFJS_WORKER_CDN;
          resolve(window.pdfjsLib);
        } else reject(new Error('PDF.js 加载失败'));
      };
      s.onerror = () => reject(new Error('无法加载 PDF.js（CDN 不可达，可能是离线环境）'));
      document.head.appendChild(s);
    });
    return pdfjsLibPromise;
  }

  W.extractPdfText = async function (file, onProgress) {
    const lib = await loadPdfJs();
    const buf = await file.arrayBuffer();
    const pdf = await lib.getDocument({ data: buf }).promise;
    const pages = [];
    for (let i = 1; i <= pdf.numPages; i++) {
      const page = await pdf.getPage(i);
      const content = await page.getTextContent();
      const text = content.items.map((it) => it.str).join(' ').trim();
      pages.push({ page: i, text: text });
      if (onProgress) onProgress(i, pdf.numPages);
    }
    return pages;
  };

  // ---------------- AI 多轮对话 ----------------
  // 与 translate 不同：保留完整对话上下文（messages 数组），支持多轮追问。
  // 引擎：sf（SiliconFlow，复用 sfChat）/ ollama（本地 /api/chat）。
  W.chat = async function (messages, opts) {
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
    // 默认 SiliconFlow：用完整 messages 数组（含 system + 历史）
    const body = {
      model: opts.sfModel || 'Qwen/Qwen3-8B',
      messages: messages,
      temperature: 0.7, max_tokens: 4096, enable_thinking: false
    };
    const out = await sfChat(opts, body);
    if (!out) throw new Error('模型返回空内容');
    return out;
  };

  // ---------------- 术语库管理器 ----------------
  // 内置术语库：按领域分类，翻译前预处理替换，保证专业术语一致性
  // 优先级：用户自定义术语 > 内置术语库 > 引擎默认翻译
  const K_TERMS = 'winocr_terms_v1';

  const BUILTIN_TERMS = {
    medical: {
      'myocardial infarction': '心肌梗死',
      'cerebrovascular accident': '脑卒中',
      'pneumonia': '肺炎',
      'hypertension': '高血压',
      'diabetes mellitus': '糖尿病',
      'renal failure': '肾衰竭',
      'hepatic cirrhosis': '肝硬化',
      'leukemia': '白血病',
      'lymphoma': '淋巴瘤',
      'osteoporosis': '骨质疏松症',
      'arrhythmia': '心律失常',
      'tachycardia': '心动过速',
      'bradycardia': '心动过缓',
      'hypotension': '低血压',
      'hyperglycemia': '高血糖',
      'hypoglycemia': '低血糖',
      'anemia': '贫血',
      'thrombosis': '血栓形成',
      'embolism': '栓塞',
      'sepsis': '败血症',
      'meningitis': '脑膜炎',
      'encephalitis': '脑炎',
      'nephritis': '肾炎',
      'gastritis': '胃炎',
      'hepatitis': '肝炎',
      'pancreatitis': '胰腺炎',
      'appendicitis': '阑尾炎',
      'cholecystitis': '胆囊炎',
      'peritonitis': '腹膜炎',
      'pneumothorax': '气胸',
      'hemothorax': '血胸',
      'fracture': '骨折',
      'dislocation': '脱臼',
      'sprain': '扭伤',
      'strain': '拉伤',
      'concussion': '脑震荡',
      'contusion': '挫伤',
      'laceration': '撕裂伤',
      'abrasion': '擦伤',
      'ulcer': '溃疡',
      'carcinoma': '癌',
      'sarcoma': '肉瘤',
      'melanoma': '黑色素瘤',
      'adenoma': '腺瘤',
      'fibroma': '纤维瘤',
      'lipoma': '脂肪瘤',
      'hemangioma': '血管瘤',
      'neuroblastoma': '神经母细胞瘤',
      'retinoblastoma': '视网膜母细胞瘤',
      'osteosarcoma': '骨肉瘤',
      'chondrosarcoma': '软骨肉瘤',
      'leiomyoma': '平滑肌瘤',
      'adenocarcinoma': '腺癌',
      'squamous cell carcinoma': '鳞状细胞癌',
      'basal cell carcinoma': '基底细胞癌',
      'transitional cell carcinoma': '移行细胞癌',
      'small cell carcinoma': '小细胞癌',
      'non-small cell carcinoma': '非小细胞癌',
      'metastasis': '转移',
      'recurrence': '复发',
      'remission': '缓解',
      'prognosis': '预后',
      'diagnosis': '诊断',
      'differential diagnosis': '鉴别诊断',
      'treatment': '治疗',
      'surgery': '手术',
      'chemotherapy': '化疗',
      'radiotherapy': '放疗',
      'immunotherapy': '免疫治疗',
      'hormone therapy': '激素治疗',
      'targeted therapy': '靶向治疗',
      'palliative care': '姑息治疗',
      'adjuvant therapy': '辅助治疗',
      'neoadjuvant therapy': '新辅助治疗',
      'prophylaxis': '预防',
      'contraindication': '禁忌症',
      'side effect': '副作用',
      'adverse reaction': '不良反应',
      'allergy': '过敏',
      'anaphylaxis': '过敏性休克',
      'intolerance': '不耐受',
      'resistance': '耐药',
      'sensitivity': '敏感性',
      'specificity': '特异性',
      'efficacy': '疗效',
      'potency': '药效',
      'bioavailability': '生物利用度',
      'pharmacokinetics': '药代动力学',
      'pharmacodynamics': '药效动力学',
      'half-life': '半衰期',
      'clearance': '清除率',
      'volume of distribution': '分布容积',
      'protein binding': '蛋白结合率',
      'metabolism': '代谢',
      'excretion': '排泄',
      'absorption': '吸收',
      'distribution': '分布',
      'biotransformation': '生物转化',
      'enzyme induction': '酶诱导',
      'enzyme inhibition': '酶抑制',
      'drug interaction': '药物相互作用',
      'contraindicated': '禁忌',
      'indicated': '适应症',
      'dose': '剂量',
      'dosage': '用量',
      'administration': '给药',
      'route': '途径',
      'frequency': '频次',
      'duration': '疗程',
      'regimen': '方案',
      'protocol': '方案',
      'trial': '试验',
      'placebo': '安慰剂',
      'control group': '对照组',
      'experimental group': '实验组',
      'randomized': '随机化',
      'double-blind': '双盲',
      'single-blind': '单盲',
      'open-label': '开放标签',
      'crossover': '交叉设计',
      'parallel group': '平行组',
      'multicenter': '多中心',
      'cohort study': '队列研究',
      'case-control study': '病例对照研究',
      'cross-sectional study': '横断面研究',
      'longitudinal study': '纵向研究',
      'prospective study': '前瞻性研究',
      'retrospective study': '回顾性研究',
      'meta-analysis': '荟萃分析',
      'systematic review': '系统综述',
      'randomized controlled trial': '随机对照试验',
      'clinical trial': '临床试验',
      'phase I trial': 'I期临床试验',
      'phase II trial': 'II期临床试验',
      'phase III trial': 'III期临床试验',
      'phase IV trial': 'IV期临床试验',
      'adverse event': '不良事件',
      'serious adverse event': '严重不良事件',
      'adverse drug reaction': '药物不良反应',
      'drug safety': '药物安全性',
      'pharmacovigilance': '药物警戒',
      'toxicology': '毒理学',
      'therapeutic index': '治疗指数',
      'margin of safety': '安全范围',
      'lethal dose': '致死剂量',
      'toxic dose': '中毒剂量',
      'maximum tolerated dose': '最大耐受剂量',
      'minimum effective dose': '最小有效剂量',
      'loading dose': '负荷剂量',
      'maintenance dose': '维持剂量',
      'bolus dose': '推注剂量',
      'infusion': '输注',
      'injection': '注射',
      'subcutaneous': '皮下注射',
      'intramuscular': '肌肉注射',
      'intravenous': '静脉注射',
      'intrathecal': '鞘内注射',
      'epidural': '硬膜外注射',
      'topical': '外用',
      'ophthalmic': '眼科用药',
      'otic': '耳科用药',
      'nasal': '鼻腔用药',
      'inhalation': '吸入用药',
      'rectal': '直肠给药',
      'vaginal': '阴道给药',
      'sublingual': '舌下含服',
      'buccal': '口腔黏膜用药',
      'transdermal': '透皮给药',
      'iontophoresis': '离子导入',
    },
    tech: {
      'machine learning': '机器学习',
      'deep learning': '深度学习',
      'neural network': '神经网络',
      'convolutional neural network': '卷积神经网络',
      'recurrent neural network': '循环神经网络',
      'generative adversarial network': '生成对抗网络',
      'transformer': 'Transformer',
      'attention mechanism': '注意力机制',
      'self-attention': '自注意力',
      'multi-head attention': '多头注意力',
      'feedforward neural network': '前馈神经网络',
      'backpropagation': '反向传播',
      'gradient descent': '梯度下降',
      'stochastic gradient descent': '随机梯度下降',
      'Adam optimizer': 'Adam 优化器',
      'batch normalization': '批归一化',
      'dropout': 'Dropout',
      'activation function': '激活函数',
      'ReLU': 'ReLU',
      'sigmoid': 'Sigmoid',
      'tanh': 'Tanh',
      'softmax': 'Softmax',
      'loss function': '损失函数',
      'cross-entropy loss': '交叉熵损失',
      'mean squared error': '均方误差',
      'mean absolute error': '平均绝对误差',
      'regularization': '正则化',
      'L1 regularization': 'L1 正则化',
      'L2 regularization': 'L2 正则化',
      'data augmentation': '数据增强',
      'transfer learning': '迁移学习',
      'fine-tuning': '微调',
      'pre-training': '预训练',
      'hyperparameter tuning': '超参数调优',
      'cross-validation': '交叉验证',
      'k-fold cross-validation': 'K 折交叉验证',
      'grid search': '网格搜索',
      'random search': '随机搜索',
      'Bayesian optimization': '贝叶斯优化',
      'early stopping': '早停',
      'ensemble learning': '集成学习',
      'bagging': 'Bagging',
      'boosting': 'Boosting',
      'random forest': '随机森林',
      'decision tree': '决策树',
      'support vector machine': '支持向量机',
      'k-nearest neighbors': 'K 近邻算法',
      'naive Bayes': '朴素贝叶斯',
      'logistic regression': '逻辑回归',
      'linear regression': '线性回归',
      'polynomial regression': '多项式回归',
      'ridge regression': '岭回归',
      'Lasso regression': 'Lasso 回归',
      'principal component analysis': '主成分分析',
      't-SNE': 't-SNE',
      'UMAP': 'UMAP',
      'clustering': '聚类',
      'k-means': 'K-means 聚类',
      'hierarchical clustering': '层次聚类',
      'DBSCAN': 'DBSCAN',
      'anomaly detection': '异常检测',
      'outlier detection': '离群点检测',
      'feature engineering': '特征工程',
      'feature selection': '特征选择',
      'feature extraction': '特征提取',
      'dimensionality reduction': '降维',
      'natural language processing': '自然语言处理',
      'computer vision': '计算机视觉',
      'reinforcement learning': '强化学习',
      'supervised learning': '监督学习',
      'unsupervised learning': '无监督学习',
      'semi-supervised learning': '半监督学习',
      'self-supervised learning': '自监督学习',
      'federated learning': '联邦学习',
      'edge computing': '边缘计算',
      'cloud computing': '云计算',
      'distributed computing': '分布式计算',
      'parallel computing': '并行计算',
      'high-performance computing': '高性能计算',
      'quantum computing': '量子计算',
      'blockchain': '区块链',
      'cryptocurrency': '加密货币',
      'smart contract': '智能合约',
      'decentralized finance': '去中心化金融',
      'non-fungible token': '非同质化代币',
      'metaverse': '元宇宙',
      'virtual reality': '虚拟现实',
      'augmented reality': '增强现实',
      'mixed reality': '混合现实',
      'digital twin': '数字孪生',
      'Internet of Things': '物联网',
      'Industry 4.0': '工业 4.0',
      'big data': '大数据',
      'data mining': '数据挖掘',
      'data warehouse': '数据仓库',
      'data lake': '数据湖',
      'business intelligence': '商业智能',
      'data visualization': '数据可视化',
      'dashboard': '仪表盘',
      'report': '报告',
      'analytics': '分析',
      'descriptive analytics': '描述性分析',
      'diagnostic analytics': '诊断性分析',
      'predictive analytics': '预测性分析',
      'prescriptive analytics': '规范性分析',
      'real-time analytics': '实时分析',
      'stream processing': '流处理',
      'batch processing': '批处理',
      'ETL': 'ETL（抽取-转换-加载）',
      'data pipeline': '数据管道',
      'workflow': '工作流',
      'orchestration': '编排',
      'microservices': '微服务',
      'containerization': '容器化',
      'Docker': 'Docker',
      'Kubernetes': 'Kubernetes',
      'DevOps': 'DevOps',
      'CI/CD': 'CI/CD（持续集成/持续部署）',
      'Git': 'Git',
      'version control': '版本控制',
      'code review': '代码审查',
      'unit testing': '单元测试',
      'integration testing': '集成测试',
      'system testing': '系统测试',
      'acceptance testing': '验收测试',
      'regression testing': '回归测试',
      'performance testing': '性能测试',
      'load testing': '负载测试',
      'stress testing': '压力测试',
      'security testing': '安全测试',
      'penetration testing': '渗透测试',
      'vulnerability assessment': '漏洞评估',
      'security audit': '安全审计',
      'compliance': '合规',
      'GDPR': 'GDPR（通用数据保护条例）',
      'HIPAA': 'HIPAA（健康保险可携性和责任法案）',
      'SOC 2': 'SOC 2',
      'ISO 27001': 'ISO 27001',
      'ISO 9001': 'ISO 9001',
      'CMMI': 'CMMI',
      'ITIL': 'ITIL',
      'COBIT': 'COBIT',
      'Agile': '敏捷开发',
      'Scrum': 'Scrum',
      'Kanban': '看板',
      'Lean': '精益开发',
      'Six Sigma': '六西格玛',
      'TQM': '全面质量管理',
      'OKR': 'OKR（目标与关键结果）',
      'KPI': 'KPI（关键绩效指标）',
      'balanced scorecard': '平衡计分卡',
      'SWOT analysis': 'SWOT 分析',
      'PEST analysis': 'PEST 分析',
      'Porter\'s Five Forces': '波特五力模型',
      'value chain': '价值链',
      'core competency': '核心竞争力',
      'competitive advantage': '竞争优势',
      'market segmentation': '市场细分',
      'target market': '目标市场',
      'marketing mix': '营销组合',
      '4P': '4P（产品、价格、渠道、促销）',
      'customer relationship management': '客户关系管理',
      'supply chain management': '供应链管理',
      'enterprise resource planning': '企业资源规划',
      'customer experience': '客户体验',
      'user experience': '用户体验',
      'human-computer interaction': '人机交互',
      'accessibility': '可访问性',
      'usability': '可用性',
      'information architecture': '信息架构',
      'interaction design': '交互设计',
      'visual design': '视觉设计',
      'graphic design': '平面设计',
      'motion design': '动效设计',
      'typography': '排版',
      'color theory': '色彩理论',
      'layout': '布局',
      'grid system': '网格系统',
      'responsive design': '响应式设计',
      'adaptive design': '自适应设计',
      'progressive enhancement': '渐进增强',
      'graceful degradation': '优雅降级',
      'performance optimization': '性能优化',
      'SEO': 'SEO（搜索引擎优化）',
      'SEM': 'SEM（搜索引擎营销）',
      'content marketing': '内容营销',
      'social media marketing': '社交媒体营销',
      'email marketing': '邮件营销',
      'influencer marketing': '网红营销',
      'affiliate marketing': '联盟营销',
      'conversion rate optimization': '转化率优化',
      'A/B testing': 'A/B 测试',
      'multivariate testing': '多变量测试',
      'user research': '用户研究',
      'usability testing': '可用性测试',
      'heat map': '热力图',
      'eye tracking': '眼动追踪',
      'clickstream analysis': '点击流分析',
      'funnel analysis': '漏斗分析',
      'cohort analysis': '队列分析',
      'retention analysis': '留存分析',
      'churn analysis': '流失分析',
      'lifetime value analysis': '生命周期价值分析',
      'customer segmentation': '客户细分',
      'persona': '用户画像',
      'journey map': '用户旅程图',
      'service design': '服务设计',
      'design thinking': '设计思维',
      'lean startup': '精益创业',
      'minimum viable product': '最小可行产品',
      'product-market fit': '产品市场契合度',
      'growth hacking': '增长黑客',
      'viral marketing': '病毒式营销',
      'referral program': '推荐计划',
      'loyalty program': '忠诚度计划',
      'customer advocacy': '客户倡导',
      'brand management': '品牌管理',
      'brand identity': '品牌识别',
      'brand awareness': '品牌认知',
      'brand equity': '品牌资产',
      'reputation management': '声誉管理',
      'crisis management': '危机管理',
      'risk management': '风险管理',
      'business continuity planning': '业务连续性规划',
      'disaster recovery': '灾难恢复',
      'project management': '项目管理',
      'program management': '项目群管理',
      'portfolio management': '项目组合管理',
      'stakeholder management': '利益相关者管理',
      'change management': '变革管理',
      'conflict resolution': '冲突解决',
      'negotiation': '谈判',
      'leadership': '领导力',
      'team building': '团队建设',
      'performance management': '绩效管理',
      'talent management': '人才管理',
      'workforce planning': '劳动力规划',
      'succession planning': '继任规划',
      'organizational development': '组织发展',
      'corporate culture': '企业文化',
      'diversity and inclusion': '多元化与包容',
      'employee engagement': '员工敬业度',
      'work-life balance': '工作生活平衡',
      'remote work': '远程办公',
      'hybrid work': '混合办公',
      'digital transformation': '数字化转型',
      'business process reengineering': '业务流程再造',
      'lean manufacturing': '精益制造',
      'just-in-time': '准时制',
      'total productive maintenance': '全面生产维护',
      'kaizen': '改善',
      'kanban': '看板',
      '5S': '5S 管理',
      'poka-yoke': '防错',
      'root cause analysis': '根本原因分析',
      'fishbone diagram': '鱼骨图',
      'Pareto chart': '帕累托图',
      'control chart': '控制图',
      'scatter diagram': '散点图',
      'histogram': '直方图',
      'check sheet': '检查表',
      'flowchart': '流程图',
      'value stream mapping': '价值流图',
      'spaghetti diagram': '意大利面图',
      'takt time': '节拍时间',
      'cycle time': '周期时间',
      'lead time': '提前期',
      'throughput': '吞吐量',
      'capacity': '产能',
      'bottleneck': '瓶颈',
      'constraint': '约束',
      'buffer': '缓冲',
      'inventory': '库存',
      'work in progress': '在制品',
      'finished goods': '产成品',
      'raw material': '原材料',
      'bill of materials': '物料清单',
      'material requirements planning': '物料需求计划',
      'manufacturing resource planning': '制造资源计划',
      'enterprise resource planning': '企业资源规划',
      'supply chain': '供应链',
      'logistics': '物流',
      'procurement': '采购',
      'sourcing': '寻源',
      'outsourcing': '外包',
      'offshoring': '离岸外包',
      'nearshoring': '近岸外包',
      'insourcing': '内包',
      'make or buy decision': '自制或外购决策',
      'total cost of ownership': '总拥有成本',
      'cost-benefit analysis': '成本效益分析',
      'return on investment': '投资回报率',
      'net present value': '净现值',
      'internal rate of return': '内部收益率',
      'payback period': '投资回收期',
      'break-even analysis': '盈亏平衡分析',
      'sensitivity analysis': '敏感性分析',
      'scenario analysis': '情景分析',
      'Monte Carlo simulation': '蒙特卡洛模拟',
      'decision tree analysis': '决策树分析',
      'real options analysis': '实物期权分析',
      'game theory': '博弈论',
      'linear programming': '线性规划',
      'integer programming': '整数规划',
      'dynamic programming': '动态规划',
      'network optimization': '网络优化',
      'queuing theory': '排队论',
      'inventory theory': '库存理论',
      'supply chain optimization': '供应链优化',
      'demand forecasting': '需求预测',
      'sales forecast': '销售预测',
      'capacity planning': '产能规划',
      'production planning': '生产计划',
      'material planning': '物料计划',
      'distribution planning': '分销计划',
      'transportation planning': '运输规划',
      'warehouse management': '仓储管理',
      'inventory management': '库存管理',
      'order fulfillment': '订单履行',
      'reverse logistics': '逆向物流',
      'cold chain': '冷链',
      'hazmat': '危险品',
      'customs': '海关',
      'trade compliance': '贸易合规',
      'incoterms': '国际贸易术语',
      'letter of credit': '信用证',
      'bill of lading': '提单',
      'certificate of origin': '原产地证书',
      'free trade agreement': '自由贸易协定',
      'tariff': '关税',
      'duty': '税费',
      'value-added tax': '增值税',
      'sales tax': '销售税',
      'excise tax': '消费税',
      'property tax': '财产税',
      'income tax': '所得税',
      'corporate tax': '企业所得税',
      'tax deduction': '税收减免',
      'tax credit': '税收抵免',
      'tax haven': '避税天堂',
      'transfer pricing': '转让定价',
      'tax treaty': '税收协定',
      'double taxation': '双重征税',
      'tax avoidance': '避税',
      'tax evasion': '逃税',
      'money laundering': '洗钱',
      'financial crime': '金融犯罪',
      'fraud': '欺诈',
      'embezzlement': '挪用',
      'bribery': '贿赂',
      'corruption': '腐败',
      'conflict of interest': '利益冲突',
      'insider trading': '内幕交易',
      'market manipulation': '市场操纵',
      'antitrust': '反垄断',
      'competition policy': '竞争政策',
      'intellectual property': '知识产权',
      'patent': '专利',
      'trademark': '商标',
      'copyright': '版权',
      'trade secret': '商业秘密',
      'licensing': '许可授权',
      'franchising': '特许经营',
      'joint venture': '合资企业',
      'strategic alliance': '战略联盟',
      'merger': '合并',
      'acquisition': '收购',
      'divestiture': '剥离',
      'spin-off': '分拆上市',
      'initial public offering': '首次公开发行',
      'secondary offering': '增发',
      'private placement': '私募',
      'venture capital': '风险投资',
      'private equity': '私募股权',
      'hedge fund': '对冲基金',
      'mutual fund': '共同基金',
      'exchange-traded fund': '交易所交易基金',
      'pension fund': '养老基金',
      'sovereign wealth fund': '主权财富基金',
      'investment banking': '投资银行',
      'commercial banking': '商业银行',
      'retail banking': '零售银行',
      'wholesale banking': '批发银行',
      'private banking': '私人银行',
      'wealth management': '财富管理',
      'asset management': '资产管理',
      'portfolio management': '投资组合管理',
      'risk management': '风险管理',
      'credit risk': '信用风险',
      'market risk': '市场风险',
      'operational risk': '操作风险',
      'liquidity risk': '流动性风险',
      'systemic risk': '系统性风险',
      'counterparty risk': '交易对手风险',
      'settlement risk': '结算风险',
      'sovereign risk': '主权风险',
      'political risk': '政治风险',
      'regulatory risk': '监管风险',
      'compliance risk': '合规风险',
      'reputational risk': '声誉风险',
      'strategic risk': '战略风险',
      'business risk': '商业风险',
      'financial risk': '财务风险',
      'insurance': '保险',
      'underwriting': '承保',
      'actuarial science': '精算学',
      'reinsurance': '再保险',
      'risk pooling': '风险池',
      'risk transfer': '风险转移',
      'risk mitigation': '风险缓解',
      'risk avoidance': '风险规避',
      'risk retention': '风险自留',
      'risk diversification': '风险分散',
      'risk hedging': '风险对冲',
      'risk arbitrage': '风险套利',
      'risk premium': '风险溢价',
      'risk-adjusted return': '风险调整收益',
      'Sharpe ratio': '夏普比率',
      'Sortino ratio': '索提诺比率',
      'Treynor ratio': '特雷诺比率',
      'information ratio': '信息比率',
      'alpha': '阿尔法',
      'beta': '贝塔',
      'gamma': '伽马',
      'delta': '德尔塔',
      'theta': '西塔',
      'vega': '维加',
      'rho': '柔',
      'standard deviation': '标准差',
      'variance': '方差',
      'covariance': '协方差',
      'correlation': '相关性',
      'coefficient of determination': '决定系数',
      'p-value': 'P 值',
      'confidence interval': '置信区间',
      'hypothesis testing': '假设检验',
      'regression analysis': '回归分析',
      'time series analysis': '时间序列分析',
      'forecasting': '预测',
      'trend analysis': '趋势分析',
      'seasonality': '季节性',
      'cyclicality': '周期性',
      'autocorrelation': '自相关',
      'stationarity': '平稳性',
      'dickey-fuller test': '迪基-富勒检验',
      'augmented dickey-fuller test': '增广迪基-富勒检验',
      'KPSS test': 'KPSS 检验',
      'Granger causality test': '格兰杰因果检验',
      'cointegration test': '协整检验',
      'vector autoregression': '向量自回归',
      'moving average': '移动平均',
      'exponential smoothing': '指数平滑',
      'ARIMA model': 'ARIMA 模型',
      'GARCH model': 'GARCH 模型',
      'EGARCH model': 'EGARCH 模型',
      'stochastic volatility model': '随机波动率模型',
      'Monte Carlo simulation': '蒙特卡洛模拟',
      'bootstrap method': '自助法',
      'jackknife method': '刀切法',
      'cross-validation': '交叉验证',
      'backtesting': '回测',
      'stress testing': '压力测试',
      'scenario analysis': '情景分析',
      'sensitivity analysis': '敏感性分析',
      'value at risk': '风险价值',
      'expected shortfall': '预期损失',
      'conditional value at risk': '条件风险价值',
      'credit default swap': '信用违约互换',
      'interest rate swap': '利率互换',
      'currency swap': '货币互换',
      'commodity swap': '商品互换',
      'total return swap': '总收益互换',
      'equity swap': '股权互换',
      'variance swap': '方差互换',
      'volatility swap': '波动率互换',
      'forward contract': '远期合约',
      'futures contract': '期货合约',
      'option': '期权',
      'call option': '看涨期权',
      'put option': '看跌期权',
      'American option': '美式期权',
      'European option': '欧式期权',
      'Bermudan option': '百慕大期权',
      'Asian option': '亚式期权',
      'barrier option': '障碍期权',
      'binary option': '二元期权',
      'lookback option': '回望期权',
      'swaption': '互换期权',
      'cap': '上限',
      'floor': '下限',
      'collar': '领口',
      'straddle': '跨式组合',
      'strangle': '宽跨式组合',
      'butterfly spread': '蝶式价差',
      'condor spread': '鹰式价差',
      'iron condor': '铁鹰式',
      'iron butterfly': '铁蝶式',
      'box spread': '盒式价差',
      'ratio spread': '比率价差',
      'calendar spread': '日历价差',
      'diagonal spread': '对角价差',
      'vertical spread': '垂直价差',
      'horizontal spread': '水平价差',
      'debit spread': '借方价差',
      'credit spread': '贷方价差',
      'bull spread': '牛市价差',
      'bear spread': '熊市价差',
      'bull call spread': '牛市看涨价差',
      'bear call spread': '牛市看跌价差',
      'bull put spread': '熊市看涨价差',
      'bear put spread': '熊市看跌价差',
      'covered call': '备兑看涨期权',
      'protective put': '保护性看跌期权',
      'married put': '已婚看跌期权',
      'cash-secured put': '现金担保看跌期权',
      'naked option': '裸期权',
      'in-the-money': '实值期权',
      'at-the-money': '平值期权',
      'out-of-the-money': '虚值期权',
      'intrinsic value': '内在价值',
      'time value': '时间价值',
      'strike price': '行权价',
      'expiration date': '到期日',
      'option premium': '期权费',
      'open interest': '未平仓合约',
      'volume': '成交量',
      'liquidity': '流动性',
      'market depth': '市场深度',
      'bid-ask spread': '买卖价差',
      'order book': '订单簿',
      'market maker': '做市商',
      'high-frequency trading': '高频交易',
      'algorithmic trading': '算法交易',
      'quantitative trading': '量化交易',
      'statistical arbitrage': '统计套利',
      'pairs trading': '配对交易',
      'momentum trading': '动量交易',
      'mean reversion trading': '均值回归交易',
      'trend following': '趋势跟踪',
      'scalping': '剥头皮交易',
      'day trading': '日内交易',
      'swing trading': '摆动交易',
      'position trading': '头寸交易',
      'long position': '多头头寸',
      'short position': '空头头寸',
      'hedge': '对冲',
      'speculation': '投机',
      'arbitrage': '套利',
      'market efficiency': '市场效率',
      'efficient market hypothesis': '有效市场假说',
      'behavioral finance': '行为金融学',
      'prospect theory': '前景理论',
      'loss aversion': '损失厌恶',
      'anchoring bias': '锚定偏差',
      'confirmation bias': '确认偏差',
      'herding behavior': '羊群效应',
      'overconfidence bias': '过度自信偏差',
      'availability heuristic': '可得性启发',
      'representativeness heuristic': '代表性启发',
      'framing effect': '框架效应',
      'endowment effect': '禀赋效应',
      'status quo bias': '现状偏差',
      'sunk cost fallacy': '沉没成本谬误',
      'gambler\'s fallacy': '赌徒谬误',
      'hot-hand fallacy': '热手谬误',
      'recency bias': '近因偏差',
      'hindsight bias': '后见之明偏差',
      'self-attribution bias': '自归因偏差',
      'survivorship bias': '幸存者偏差',
      'selection bias': '选择偏差',
      'sampling bias': '抽样偏差',
      'non-response bias': '无应答偏差',
      'measurement error': '测量误差',
      'sampling error': '抽样误差',
      'standard error': '标准误',
      'confidence level': '置信水平',
      'significance level': '显著性水平',
      'type I error': '第一类错误',
      'type II error': '第二类错误',
      'power of a test': '检验功效',
      'effect size': '效应量',
      'degrees of freedom': '自由度',
      'central limit theorem': '中心极限定理',
      'law of large numbers': '大数定律',
      'Bayes\' theorem': '贝叶斯定理',
      'maximum likelihood estimation': '极大似然估计',
      'least squares estimation': '最小二乘估计',
      'generalized method of moments': '广义矩估计',
      'instrumental variables': '工具变量',
      'two-stage least squares': '两阶段最小二乘',
      'three-stage least squares': '三阶段最小二乘',
      'panel data': '面板数据',
      'cross-sectional data': '横截面数据',
      'time series data': '时间序列数据',
      'longitudinal data': '纵向数据',
      'multilevel data': '多层次数据',
      'big data': '大数据',
      'data quality': '数据质量',
      'data integrity': '数据完整性',
      'data governance': '数据治理',
      'data stewardship': '数据管理',
      'data lineage': '数据血缘',
      'data catalog': '数据目录',
      'data dictionary': '数据字典',
      'metadata': '元数据',
      'master data': '主数据',
      'reference data': '参考数据',
      'transactional data': '交易数据',
      'operational data': '运营数据',
      'analytical data': '分析数据',
      'unstructured data': '非结构化数据',
      'structured data': '结构化数据',
      'semi-structured data': '半结构化数据',
      'dark data': '暗数据',
      'data monetization': '数据变现',
      'data as a service': '数据即服务',
      'data marketplace': '数据市场',
      'open data': '开放数据',
      'linked data': '关联数据',
      'semantic web': '语义网',
      'knowledge graph': '知识图谱',
      'ontology': '本体论',
      'taxonomy': '分类学',
      'folksonomy': '自由分类法',
      'tagging': '标签',
      'annotation': '注释',
      'markup': '标记',
      'structured markup': '结构化标记',
      'schema': '模式',
      'data model': '数据模型',
      'entity-relationship model': '实体-关系模型',
      'relational model': '关系模型',
      'hierarchical model': '层次模型',
      'network model': '网络模型',
      'object-oriented model': '面向对象模型',
      'object-relational model': '对象-关系模型',
      'document model': '文档模型',
      'key-value model': '键值模型',
      'column-family model': '列族模型',
      'graph model': '图模型',
      'wide-column model': '宽列模型',
      'time-series model': '时间序列模型',
      'spatial model': '空间模型',
      'multidimensional model': '多维模型',
      'star schema': '星型模式',
      'snowflake schema': '雪花模式',
      'fact table': '事实表',
      'dimension table': '维度表',
      'measure': '度量',
      'metric': '指标',
      'KPI': 'KPI（关键绩效指标）',
      'OKR': 'OKR（目标与关键结果）',
      'balanced scorecard': '平衡计分卡',
      'scorecard': '记分卡',
      'dashboard': '仪表盘',
      'report': '报告',
      'analytics': '分析',
      'business intelligence': '商业智能',
      'data mining': '数据挖掘',
      'machine learning': '机器学习',
      'deep learning': '深度学习',
      'artificial intelligence': '人工智能',
      'natural language processing': '自然语言处理',
      'computer vision': '计算机视觉',
      'robotics': '机器人学',
      'expert system': '专家系统',
      'knowledge-based system': '基于知识的系统',
      'rule-based system': '基于规则的系统',
      'fuzzy logic': '模糊逻辑',
      'genetic algorithm': '遗传算法',
      'evolutionary algorithm': '进化算法',
      'swarm intelligence': '群体智能',
      'ant colony optimization': '蚁群优化',
      'particle swarm optimization': '粒子群优化',
      'simulated annealing': '模拟退火',
      'tabu search': '禁忌搜索',
      'branch and bound': '分支定界',
      'dynamic programming': '动态规划',
      'greedy algorithm': '贪心算法',
      'divide and conquer': '分治法',
      'backtracking': '回溯法',
      'brute force': '暴力搜索',
      'heuristic': '启发式',
      'metaheuristic': '元启发式',
      'approximation algorithm': '近似算法',
      'randomized algorithm': '随机化算法',
      'deterministic algorithm': '确定性算法',
      'parallel algorithm': '并行算法',
      'distributed algorithm': '分布式算法',
      'concurrent algorithm': '并发算法',
      'sequential algorithm': '顺序算法',
      'recursive algorithm': '递归算法',
      'iterative algorithm': '迭代算法',
      'incremental algorithm': '增量算法',
      'online algorithm': '在线算法',
      'offline algorithm': '离线算法',
      'streaming algorithm': '流算法',
      'sliding window algorithm': '滑动窗口算法',
      'two-pointer technique': '双指针技术',
      'greedy approach': '贪心策略',
      'dynamic programming approach': '动态规划策略',
      'divide and conquer approach': '分治策略',
      'backtracking approach': '回溯策略',
      'branch and bound approach': '分支定界策略',
      'transform and conquer': '变换与征服',
      'problem reduction': '问题归约',
      'problem transformation': '问题变换',
      'problem decomposition': '问题分解',
      'problem abstraction': '问题抽象',
      'problem modeling': '问题建模',
      'problem solving': '问题求解',
      'algorithm design': '算法设计',
      'algorithm analysis': '算法分析',
      'computational complexity': '计算复杂度',
      'time complexity': '时间复杂度',
      'space complexity': '空间复杂度',
      'big O notation': '大 O 表示法',
      'asymptotic analysis': '渐近分析',
      'amortized analysis': '平摊分析',
      'competitive analysis': '竞争分析',
      'probabilistic analysis': '概率分析',
      'average case analysis': '平均情况分析',
      'worst case analysis': '最坏情况分析',
      'best case analysis': '最好情况分析',
      'complexity class': '复杂度类',
      'P vs NP': 'P 对 NP',
      'NP-complete': 'NP 完全',
      'NP-hard': 'NP 困难',
      'reduction': '归约',
      'Turing machine': '图灵机',
      'finite state machine': '有限状态机',
      'pushdown automaton': '下推自动机',
      'regular language': '正则语言',
      'context-free language': '上下文无关语言',
      'context-sensitive language': '上下文有关语言',
      'recursively enumerable language': '递归可枚举语言',
      'Chomsky hierarchy': '乔姆斯基层次',
      'automata theory': '自动机理论',
      'formal language theory': '形式语言理论',
      'computation theory': '计算理论',
      'algorithmic information theory': '算法信息论',
      'Kolmogorov complexity': '柯尔莫哥洛夫复杂度',
      'information entropy': '信息熵',
      'Shannon entropy': '香农熵',
      'cross-entropy': '交叉熵',
      'relative entropy': '相对熵',
      'mutual information': '互信息',
      'channel capacity': '信道容量',
      'data compression': '数据压缩',
      'lossless compression': '无损压缩',
      'lossy compression': '有损压缩',
      'Huffman coding': '霍夫曼编码',
      'arithmetic coding': '算术编码',
      'run-length encoding': '游程编码',
      'Lempel-Ziv-Welch': 'LZW 压缩',
      'Burrows-Wheeler transform': '伯罗斯-惠勒变换',
      'move-to-front transform': '前移变换',
      'Burrows-Wheeler transform': '伯罗斯-惠勒变换',
      'suffix array': '后缀数组',
      'suffix tree': '后缀树',
      'trie': '字典树',
      'finite-state transducer': '有限状态转换器',
      'regular expression': '正则表达式',
      'context-free grammar': '上下文无关文法',
      'parsing': '语法分析',
      'top-down parsing': '自顶向下分析',
      'bottom-up parsing': '自底向上分析',
      'recursive descent parsing': '递归下降分析',
      'LL parser': 'LL 分析器',
      'LR parser': 'LR 分析器',
      'shift-reduce parsing': '移进-归约分析',
      'operator-precedence parsing': '算符优先分析',
      'chart parsing': '图表分析',
      'Earley parser': '厄尔利分析器',
      'CYK algorithm': 'CYK 算法',
      'ambiguity': '歧义性',
      'left recursion': '左递归',
      'left factoring': '左因子提取',
      'predictive parsing': '预测分析',
      'lookahead': '前瞻符号',
      'backtracking': '回溯',
      'memoization': '记忆化',
      'tabulation': '制表法',
      'dynamic programming': '动态规划',
      'optimal substructure': '最优子结构',
      'overlapping subproblems': '重叠子问题',
      'greedy choice property': '贪心选择性质',
      'matroid': '拟阵',
      'matroid theory': '拟阵理论',
      'greedy algorithm': '贪心算法',
      'matroid intersection': '拟阵交',
      'matroid union': '拟阵并',
      'matroid partition': '拟阵划分',
      'matroid parity': '拟阵奇偶性',
      'matroid transversal': '拟阵横贯',
      'matroid matching': '拟阵匹配',
      'matroid base': '拟阵基',
      'matroid circuit': '拟阵回路',
      'matroid rank': '拟阵秩',
      'matroid closure': '拟阵闭包',
      'matroid span': '拟阵张成',
      'matroid hyperplane': '拟阵超平面',
      'matroid flat': '拟阵平面',
      'matroid loop': '拟阵环',
      'matroid coloop': '拟阵共环',
      'matroid series class': '拟阵串联类',
      'matroid parallel class': '拟阵并联类',
      'matroid dual': '拟阵对偶',
      'matroid minor': '拟阵子式',
      'matroid contraction': '拟阵收缩',
      'matroid deletion': '拟阵删除',
      'matroid truncation': '拟阵截断',
      'matroid extension': '拟阵扩展',
      'matroid lift': '拟阵提升',
      'matroid one-point extension': '拟阵单点扩展',
      'matroid two-point extension': '拟阵两点扩展',
      'matroid series extension': '拟阵串联扩展',
      'matroid parallel extension': '拟阵并联扩展',
      'matroid principal extension': '拟阵主扩展',
      'matroid principal coextension': '拟阵主共扩展',
      'matroid principal series extension': '拟阵主串联扩展',
      'matroid principal parallel extension': '拟阵主并联扩展',
      'matroid binary extension': '拟阵二元扩展',
      'matroid ternary extension': '拟阵三元扩展',
      'matroid quaternary extension': '拟阵四元扩展',
      'matroid quinary extension': '拟阵五元扩展',
      'matroid senary extension': '拟阵六元扩展',
      'matroid septenary extension': '拟阵七元扩展',
      'matroid octonary extension': '拟阵八元扩展',
      'matroid novenary extension': '拟阵九元扩展',
      'matroid denary extension': '拟阵十元扩展',
      'matroid undecenary extension': '拟阵十一元扩展',
      'matroid duodenary extension': '拟阵十二元扩展',
      'matroid tredenary extension': '拟阵十三元扩展',
      'matroid quattuordenary extension': '拟阵十四元扩展',
      'matroid quindenary extension': '拟阵十五元扩展',
      'matroid sexdenary extension': '拟阵十六元扩展',
      'matroid septendenary extension': '拟阵十七元扩展',
      'matroid octodenary extension': '拟阵十八元扩展',
      'matroid novemdenary extension': '拟阵十九元扩展',
      'matroid vigintenary extension': '拟阵二十元扩展',
      'matroid unvigintenary extension': '拟阵二十一元扩展',
      'matroid duovigintenary extension': '拟阵二十二元扩展',
      'matroid trevigintenary extension': '拟阵二十三元扩展',
      'matroid quattuorvigintenary extension': '拟阵二十四元扩展',
      'matroid quinvigintenary extension': '拟阵二十五元扩展',
      'matroid sexvigintenary extension': '拟阵二十六元扩展',
      'matroid septemvigintenary extension': '拟阵二十七元扩展',
      'matroid octovigintenary extension': '拟阵二十八元扩展',
      'matroid novemvigintenary extension': '拟阵二十九元扩展',
      'matroid trigintenary extension': '拟阵三十元扩展',
      'matroid untrigintenary extension': '拟阵三十一元扩展',
      'matroid duotrigintenary extension': '拟阵三十二元扩展',
      'matroid tretrigintenary extension': '拟阵三十三元扩展',
      'matroid quattuortrigintenary extension': '拟阵三十四元扩展',
      'matroid quintrigintenary extension': '拟阵三十五元扩展',
      'matroid sextrigintenary extension': '拟阵三十六元扩展',
      'matroid septemtrigintenary extension': '拟阵三十七元扩展',
      'matroid octotrigintenary extension': '拟阵三十八元扩展',
      'matroid novemtrigintenary extension': '拟阵三十九元扩展',
      'matroid quadragintenary extension': '拟阵四十元扩展',
    },
    law: {
      'tort': '侵权',
      'negligence': '过失',
      'strict liability': '严格责任',
      'intentional tort': '故意侵权',
      'trespass': '非法侵入',
      'nuisance': '妨害',
      'defamation': '诽谤',
      'libel': '书面诽谤',
      'slander': '口头诽谤',
      'fraud': '欺诈',
      'misrepresentation': '虚假陈述',
      'duress': '胁迫',
      'undue influence': '不当影响',
      'unconscionability': '显失公平',
      'consideration': '对价',
      'offer': '要约',
      'acceptance': '承诺',
      'revocation': '撤回',
      'rejection': '拒绝',
      'counteroffer': '反要约',
      'mirror image rule': '镜像规则',
      'postal rule': '邮寄规则',
      'invitation to treat': '要约邀请',
      'unilateral contract': '单务合同',
      'bilateral contract': '双务合同',
      'executed contract': '已履行合同',
      'executory contract': '待履行合同',
      'express contract': '明示合同',
      'implied contract': '默示合同',
      'implied-in-fact contract': '事实默示合同',
      'implied-in-law contract': '法律默示合同',
      'void contract': '无效合同',
      'voidable contract': '可撤销合同',
      'unenforceable contract': '不可强制执行合同',
      'adhesion contract': '附合合同',
      'contract of adhesion': '附合合同',
      'standard form contract': '标准格式合同',
      'boilerplate clause': '格式条款',
      'exculpatory clause': '免责条款',
      'indemnity clause': '赔偿条款',
      'hold harmless clause': '免责条款',
      'limitation of liability clause': '责任限制条款',
      'liquidated damages clause': '违约金条款',
      'penalty clause': '罚金条款',
      'force majeure clause': '不可抗力条款',
      'severability clause': '可分割条款',
      'entire agreement clause': '完整协议条款',
      'merger clause': '合并条款',
      'integration clause': '整合条款',
      'choice of law clause': '法律选择条款',
      'forum selection clause': '法院选择条款',
      'arbitration clause': '仲裁条款',
      'mediation clause': '调解条款',
      'dispute resolution clause': '争议解决条款',
      'waiver clause': '弃权条款',
      'release clause': '免责条款',
      'covenant': '契约',
      'condition precedent': '先决条件',
      'condition subsequent': '后续条件',
      'concurrent condition': '同时条件',
      'express condition': '明示条件',
      'implied condition': '默示条件',
      'condition of satisfaction': '满意条件',
      'substantial performance': '实质性履行',
      'perfect performance': '完全履行',
      'tender': '提存',
      'anticipatory breach': '预期违约',
      'anticipatory repudiation': '预期拒绝履行',
      'material breach': '实质性违约',
      'minor breach': '轻微违约',
      'fundamental breach': '根本违约',
      'remedy': '救济',
      'damages': '损害赔偿',
      'compensatory damages': '补偿性赔偿',
      'consequential damages': '间接赔偿',
      'incidental damages': '附带赔偿',
      'nominal damages': '名义赔偿',
      'punitive damages': '惩罚性赔偿',
      'liquidated damages': '违约金',
      'specific performance': '实际履行',
      'injunction': '禁令',
      'restraining order': '禁止令',
      'temporary restraining order': '临时禁止令',
      'preliminary injunction': '初步禁令',
      'permanent injunction': '永久禁令',
      'rescission': '撤销',
      'restitution': '恢复原状',
      'reformation': '更正',
      'quasi-contract': '准合同',
      'unjust enrichment': '不当得利',
      'quantum meruit': '合理报酬',
      'promissory estoppel': '允诺禁反言',
      'detrimental reliance': '有害信赖',
      'equitable estoppel': '衡平禁反言',
      'collateral estoppel': '间接禁反言',
      'res judicata': '既判力',
      'claim preclusion': '请求排除',
      'issue preclusion': '争点排除',
      'judge': '法官',
      'jury': '陪审团',
      'plaintiff': '原告',
      'defendant': '被告',
      'petitioner': '申请人',
      'respondent': '被申请人',
      'appellant': '上诉人',
      'appellee': '被上诉人',
      'cross-appeal': '交叉上诉',
      'amicus curiae': '法庭之友',
      'class action': '集体诉讼',
      'derivative action': '派生诉讼',
      'interpleader': '互争权利诉讼',
      'joinder': '合并诉讼',
      'impleader': '加入诉讼',
      'third-party practice': '第三方诉讼',
      'counterclaim': '反诉',
      'crossclaim': '交叉请求',
      'cross-claim': '交叉请求',
      'cross-complaint': '交叉起诉',
      'counter-complaint': '交叉起诉',
      'pleading': '诉状',
      'complaint': '起诉状',
      'answer': '答辩状',
      'reply': '回复状',
      'demurrer': '抗辩',
      'motion to dismiss': '驳回动议',
      'motion for summary judgment': '即决判决动议',
      'motion in limine': '预先动议',
      'discovery': '证据开示',
      'deposition': '证词录取',
      'interrogatory': '质询书',
      'request for admissions': '承认请求',
      'request for production': '文件出示请求',
      'subpoena': '传票',
      'subpoena duces tecum': '文件传票',
      'voir dire': '陪审团审查',
      'peremptory challenge': '无因回避',
      'challenge for cause': '有因回避',
      'opening statement': '开场陈述',
      'closing argument': '结案陈词',
      'direct examination': '直接询问',
      'cross-examination': '交叉询问',
      'redirect examination': '再直接询问',
      'recross-examination': '再交叉询问',
      'objection': '反对',
      'sustained': '支持反对',
      'overruled': '驳回反对',
      'sidebar': '庭边会议',
      'jury instruction': '陪审团指示',
      'verdict': '裁决',
      'general verdict': '一般裁决',
      'special verdict': '特别裁决',
      'directed verdict': '指示裁决',
      'judgment notwithstanding the verdict': '不顾陪审团裁决的判决',
      'new trial': '重新审理',
      'remittitur': '减额发回',
      'additur': '加额发回',
      'appeal': '上诉',
      'appellate review': '上诉审查',
      'de novo review': '重新审查',
      'abuse of discretion review': '滥用裁量权审查',
      'clearly erroneous review': '明显错误审查',
      'substantial evidence review': '实质性证据审查',
      'harmless error': '无害错误',
      'reversible error': '可撤销错误',
      'plain error': '明显错误',
      'burden of proof': '举证责任',
      'preponderance of the evidence': '优势证据',
      'clear and convincing evidence': '明确且令人信服的证据',
      'beyond a reasonable doubt': '排除合理怀疑',
      'prima facie evidence': '初步证据',
      'circumstantial evidence': '间接证据',
      'direct evidence': '直接证据',
      'hearsay': '传闻',
      'exception to the hearsay': '传闻例外',
      'dying declaration': '临终陈述',
      'excited utterance': '激动陈述',
      'present sense impression': '即时印象',
      'statement against interest': '不利陈述',
      'business records exception': '业务记录例外',
      'public records exception': '公共记录例外',
      'learned treatise exception': '学术著作例外',
      'residual exception': '剩余例外',
      'confrontation clause': '对质条款',
      'best evidence rule': '最佳证据规则',
      'parol evidence rule': '口头证据规则',
      'integration clause': '整合条款',
      'course of performance': '履行过程',
      'course of dealing': '交易习惯',
      'usage of trade': '行业惯例',
      'interpretation': '解释',
      'construction': '解释',
      'plain meaning rule': '平义规则',
      'contra proferentem': '不利解释规则',
      'parol evidence': '口头证据',
      'subsequent agreement': '后续协议',
      'modification': '修改',
      'novation': '合同更新',
      'accord and satisfaction': '和解与清偿',
      'release': '免责',
      'covenant not to sue': '不起诉契约',
      'liquidated damages': '违约金',
      'penalty': '罚金',
      'punitive damages': '惩罚性赔偿',
      'consequential damages': '间接损害赔偿',
      'incidental damages': '附带损害赔偿',
      'nominal damages': '名义损害赔偿',
      'compensatory damages': '补偿性损害赔偿',
      'special damages': '特殊损害赔偿',
      'general damages': '一般损害赔偿',
      'economic damages': '经济损害赔偿',
      'non-economic damages': '非经济损害赔偿',
      'pain and suffering': '痛苦与折磨',
      'loss of consortium': '丧失配偶权',
      'wrongful death': '错误死亡',
      'survival action': '幸存者诉讼',
      'res ipsa loquitur': '事实自证',
      'negligence per se': '本身过失',
      'malpractice': '渎职',
      'professional negligence': '专业过失',
      'medical malpractice': '医疗事故',
      'legal malpractice': '律师渎职',
      'product liability': '产品责任',
      'strict products liability': '严格产品责任',
      'manufacturing defect': '制造缺陷',
      'design defect': '设计缺陷',
      'failure to warn': '警示缺陷',
      'express warranty': '明示担保',
      'implied warranty': '默示担保',
      'implied warranty of merchantability': '适销性默示担保',
      'implied warranty of fitness': '特定用途默示担保',
      'warranty disclaimer': '担保免责',
      'puffery': '夸大宣传',
      'misrepresentation': '虚假陈述',
      'fraudulent misrepresentation': '欺诈性虚假陈述',
      'negligent misrepresentation': '过失性虚假陈述',
      'innocent misrepresentation': '无过错误性虚假陈述',
      'deception': '欺骗',
      'deceit': '欺诈',
      'duress': '胁迫',
      'economic duress': '经济胁迫',
      'undue influence': '不当影响',
      'unconscionable contract': '显失公平合同',
      'unconscionable bargain': '显失公平交易',
      'adhesion contract': '附合合同',
      'unilateral mistake': '单方错误',
      'mutual mistake': '双方错误',
      'mistake of fact': '事实错误',
      'mistake of law': '法律错误',
      'frustration of purpose': '合同目的落空',
      'impossibility': '履行不能',
      'impracticability': '履行不可行',
      'force majeure': '不可抗力',
      'act of God': '天灾',
      'supervening illegality': '后续违法',
      'impossibility of performance': '履行不能',
      'impracticability of performance': '履行不可行',
      'frustration': '合同目的落空',
      'commercial impracticability': '商业不可行性',
      'total destruction of subject matter': '标的物完全灭失',
      'death or incapacity of party': '当事人死亡或无行为能力',
      'supervening government action': '后续政府行为',
      'destruction of subject matter': '标的物灭失',
      'material alteration of circumstances': '情势重大变更',
      'hardship': '艰难履行',
      'renegotiation': '重新谈判',
      'contract modification': '合同修改',
      'waiver of breach': '违约弃权',
      'anticipatory breach': '预期违约',
      'anticipatory repudiation': '预期拒绝履行',
      'retraction of repudiation': '撤回拒绝履行',
      'adequate assurance of performance': '充分履行保证',
      'demand for adequate assurance': '要求充分履行保证',
      'cover': '替代购买',
      'mitigation of damages': '减损义务',
      'avoidable consequences': '可避免后果',
      'collateral source rule': '间接来源规则',
      'joint and several liability': '连带责任',
      'several liability': '按份责任',
      'contribution': '追偿',
      'indemnity': '赔偿',
      'subrogation': '代位求偿',
      'third-party beneficiary': '第三方受益人',
      'intended beneficiary': '意向受益人',
      'incidental beneficiary': '附带受益人',
      'assignment of rights': '权利转让',
      'delegation of duties': '义务委托',
      'assignor': '转让人',
      'assignee': '受让人',
      'delegator': '委托人',
      'delegatee': '受托人',
      'third-party beneficiary contract': '第三方受益合同',
      'creditor beneficiary': '债权人受益人',
      'donee beneficiary': '受赠受益人',
      'promisor': '允诺人',
      'promisee': '受诺人',
      'gratuitous promise': '无偿允诺',
      'charitable pledge': '慈善承诺',
      'promissory estoppel': '允诺禁反言',
      'detrimental reliance': '有害信赖',
      'unjust enrichment': '不当得利',
      'quantum meruit': '合理报酬',
      'quasi-contract': '准合同',
      'implied-in-law contract': '法律默示合同',
      'implied-in-fact contract': '事实默示合同',
      'bailment': '寄托',
      'bailor': '寄托人',
      'bailee': '受托人',
      'mutual-benefit bailment': '双方受益寄托',
      'sole-benefit bailor bailment': '单方受益寄托（寄托人）',
      'sole-benefit bailee bailment': '单方受益寄托（受托人）',
      'constructive bailment': '推定寄托',
      'gratuitous bailment': '无偿寄托',
      'delivery of goods': '交付货物',
      'actual delivery': '实际交付',
      'constructive delivery': '推定交付',
      'symbolic delivery': '象征交付',
      'documents of title': '物权凭证',
      'bill of lading': '提单',
      'warehouse receipt': '仓单',
      'negotiable instrument': '可转让票据',
      'negotiable instrument law': '票据法',
      'holder': '持票人',
      'holder in due course': '正当持票人',
      'due course': '正当程序',
      'bearer instrument': '无记名票据',
      'order instrument': '记名票据',
      'indorsement': '背书',
      'blank indorsement': '空白背书',
      'special indorsement': '特别背书',
      'restrictive indorsement': '限制性背书',
      'qualified indorsement': '无追索权背书',
      'conditional indorsement': '附条件背书',
      'indorser': '背书人',
      'indorsee': '被背书人',
      'accommodation party': '融通当事人',
      'accommodation endorsement': '融通背书',
      'guaranty': '保证',
      'guarantor': '保证人',
      'principal debtor': '主债务人',
      'surety': '担保人',
      'co-surety': '共同担保人',
      'subrogation': '代位求偿',
      'right of reimbursement': '追偿权',
      'right of exoneration': '免责权',
      'right of subrogation': '代位求偿权',
      'defense of guarantor': '保证人抗辩',
      'creditor\'s impairment of collateral': '债权人损害担保物',
      'release of principal debtor': '主债务人免责',
      'extension of time to principal debtor': '主债务人延期',
      'novation': '合同更新',
      ' accord and satisfaction': '和解与清偿',
      'composition agreement': '和解协议',
      'creditors\' agreement': '债权人协议',
      'assignment for benefit of creditors': '债权人利益转让',
      'bulk sale': '大宗销售',
      'bulk transfer': '大宗转让',
      'fraudulent conveyance': '欺诈性转让',
      'fraudulent transfer': '欺诈性转让',
      'preference': '优先权',
      'preferential transfer': '优先转让',
      'insolvency': '无力偿债',
      'bankruptcy': '破产',
      'chapter 7 bankruptcy': '第七章破产',
      'chapter 11 bankruptcy': '第十一章破产',
      'chapter 13 bankruptcy': '第十三章破产',
      'chapter 15 bankruptcy': '第十五章破产',
      'automatic stay': '自动中止',
      'discharge': '免责',
      'reaffirmation': '重申协议',
      'reaffirmation agreement': '重申协议',
      'exemption': '豁免',
      'exempt property': '豁免财产',
      'homestead exemption': '宅地豁免',
      'wildcard exemption': '万能豁免',
      'liquidation': '清算',
      'plan of reorganization': '重整计划',
      'cramdown': '强制批准',
      'absolute priority rule': '绝对优先规则',
      'best interest test': '最佳利益标准',
      'feasibility test': '可行性标准',
      'good faith': '善意',
      'bad faith': '恶意',
      'fraudulent intent': '欺诈意图',
      'actual fraud': '实际欺诈',
      'constructive fraud': '推定欺诈',
      'fraud in the inducement': '诱因欺诈',
      'fraud in the execution': '执行欺诈',
      'innocent misrepresentation': '无过错误性虚假陈述',
      'negligent misrepresentation': '过失性虚假陈述',
      'fraudulent misrepresentation': '欺诈性虚假陈述',
      'fraudulent concealment': '欺诈性隐瞒',
      'fraudulent nondisclosure': '欺诈性不披露',
      'material fact': '重要事实',
      'reliance': '信赖',
      'justifiable reliance': '合理信赖',
      'damages': '损害赔偿',
      'benefit-of-the-bargain damages': '合同利益损害赔偿',
      'out-of-pocket damages': '实际支出损害赔偿',
      'consequential damages': '间接损害赔偿',
      'incidental damages': '附带损害赔偿',
      'nominal damages': '名义损害赔偿',
      'punitive damages': '惩罚性赔偿',
      'exemplary damages': '惩罚性赔偿',
      'treble damages': '三倍赔偿',
      'statutory damages': '法定赔偿',
      'liquidated damages': '违约金',
      'penalty clause': '罚金条款',
      'mitigation': '减损',
      'avoidable consequences': '可避免后果',
      'foreseeability': '可预见性',
      'Hadley v. Baxendale rule': '哈德利诉巴克森代尔规则',
      'proximate cause': '近因',
      'intervening cause': '介入原因',
      'superseding cause': '替代原因',
      'but-for cause': '若无则不',
      'actual cause': '实际原因',
      'cause in fact': '事实原因',
      'legal cause': '法律原因',
      'proximate causation': '近因关系',
      'substantial factor test': '实质性因素检验',
      'direct causation': '直接因果关系',
      'indirect causation': '间接因果关系',
      'concurrent causation': '并发因果关系',
      'successive causation': '连续因果关系',
      'intervening force': '介入力量',
      'supervening force': '替代力量',
      'unforeseeable force': '不可预见力量',
      'force majeure': '不可抗力',
      'act of God': '天灾',
      'inevitable accident': '不可避免事故',
      'act of a third party': '第三方行为',
      'plaintiff\'s own conduct': '原告自身行为',
      'comparative negligence': '比较过失',
      'contributory negligence': '共同过失',
      'last clear chance doctrine': '最后明显机会原则',
      'assumption of risk': '自担风险',
      'express assumption of risk': '明示自担风险',
      'implied assumption of risk': '默示自担风险',
      'primary assumption of risk': '主要自担风险',
      'secondary assumption of risk': '次要自担风险',
      'recreational activity': '娱乐活动',
      'spectator injury': '观众伤害',
      'participant injury': '参与者伤害',
      'products liability': '产品责任',
      'negligence': '过失',
      'strict liability': '严格责任',
      'abnormally dangerous activity': '异常危险活动',
      'ultrahazardous activity': '高度危险活动',
      'wild animals': '野生动物',
      'domestic animals': '家养动物',
      'trespassing animals': '侵入动物',
      'livestock': '牲畜',
      'cattle': '牛',
      'horses': '马',
      'sheep': '羊',
      'goats': '山羊',
      'pigs': '猪',
      'poultry': '家禽',
      'bees': '蜜蜂',
      'venomous animals': '有毒动物',
      'poisonous animals': '有毒动物',
      'animals ferae naturae': '野生动物',
      'animals mansuetae': '驯养动物',
      'domitae naturae': '驯养动物',
      'animals domestici': '家畜',
      'animales ferae': '野生动物',
      'animales mansuetae': '驯养动物',
      'animales domitae naturae': '驯养动物',
      'animales domestici': '家畜',
      'bestia': '野兽',
      'fera': '野兽',
      'wild beast': '野兽',
      'domestic beast': '家畜',
      'tame beast': '驯养动物',
      'savage beast': '猛兽',
      'beast of burden': '役畜',
      'beast of prey': '猛兽',
      'raptor': '猛禽',
      'predator': '捕食者',
      'prey': '猎物',
      'hunter': '猎人',
      'hunting': '狩猎',
      'fishing': '捕鱼',
      'trapping': '诱捕',
      'poisoning': '毒杀',
      'stealing': '偷窃',
      'theft': '盗窃',
      'larceny': '盗窃罪',
      'grand larceny': '重大盗窃',
      'petty larceny': '轻微盗窃',
      'burglary': '入室盗窃',
      'robbery': '抢劫',
      'armed robbery': '持械抢劫',
      'mugging': '行凶抢劫',
      'carjacking': '劫车',
      'shoplifting': '入店行窃',
      'pickpocketing': '扒窃',
      'purse snatching': '抢包',
      'fraud': '欺诈',
      'larceny by trick': '诈骗盗窃',
      'false pretenses': '虚假借口',
      'embezzlement': '挪用',
      'conversion': '侵占',
      'receiving stolen goods': '收受赃物',
      'possession of stolen goods': '持有赃物',
      'money laundering': '洗钱',
      'racketeering': '敲诈勒索',
      'extortion': '勒索',
      'blackmail': '敲诈',
      'bribery': '贿赂',
      'kickback': '回扣',
      'gratuity': '贿赂',
      'commercial bribery': '商业贿赂',
      'official misconduct': '渎职',
      'obstruction of justice': '妨碍司法',
      'perjury': '伪证',
      'subornation of perjury': '唆使伪证',
      'contempt of court': '藐视法庭',
      'disorderly conduct': '扰乱秩序',
      'disturbing the peace': '扰乱治安',
      'public intoxication': '公共醉酒',
      'drunk driving': '酒后驾驶',
      'DWI': '酒后驾驶',
      'DUI': '酒后驾驶',
      'intoxication': '醉酒',
      'blood alcohol concentration': '血液酒精浓度',
      'breathalyzer test': '呼吸测试',
      'field sobriety test': '现场清醒测试',
      'implied consent': '默示同意',
      'chemical test': '化学测试',
      'urine test': '尿液检测',
      'blood test': '血液检测',
      'DNA test': 'DNA 检测',
      'genetic testing': '基因检测',
      'paternity test': '亲子测试',
      'blood alcohol level': '血液酒精含量',
      'blood alcohol content': '血液酒精含量',
      'intoxicating beverage': '含酒精饮料',
      'alcoholic beverage': '酒精饮料',
      'liquor': '烈酒',
      'spirits': '烈酒',
      'distilled spirits': '蒸馏酒',
      'hard liquor': '烈酒',
      'wine': '葡萄酒',
      'beer': '啤酒',
      'ale': '麦芽酒',
      'stout': '黑啤酒',
      'lager': '拉格啤酒',
      'pilsner': '皮尔森啤酒',
      'pale ale': '淡色艾尔',
      'India pale ale': '印度淡色艾尔',
      'wheat beer': '小麦啤酒',
      'sour beer': '酸啤酒',
      'fruit beer': '果味啤酒',
      'spiced beer': '香料啤酒',
      'seasonal beer': '季节啤酒',
      'craft beer': '精酿啤酒',
      'microbrewery': '小型酿酒厂',
      'brewpub': '酿酒酒吧',
      'brewery': '酿酒厂',
      'distillery': '蒸馏厂',
      'winery': '酒庄',
      'vineyard': '葡萄园',
      'vintner': '葡萄酒商',
      'sommelier': '侍酒师',
      'oenophile': '葡萄酒鉴赏家',
      'teetotaler': '绝对禁酒者',
      'temperance': '禁酒',
      'abstinence': '节制',
      'sobriety': '清醒',
      'recovery': '康复',
      'rehabilitation': '康复治疗',
      'treatment': '治疗',
      'therapy': '治疗',
      'psychotherapy': '心理治疗',
      'counseling': '心理咨询',
      'psychiatry': '精神病学',
      'psychiatrist': '精神科医生',
      'psychologist': '心理学家',
      'clinical psychologist': '临床心理学家',
      'counseling psychologist': '咨询心理学家',
      'school psychologist': '学校心理学家',
      'forensic psychologist': '司法心理学家',
      'neuropsychologist': '神经心理学家',
      'cognitive psychologist': '认知心理学家',
      'social psychologist': '社会心理学家',
      'developmental psychologist': '发展心理学家',
      'health psychologist': '健康心理学家',
      'sports psychologist': '运动心理学家',
      'organizational psychologist': '组织心理学家',
      'industrial psychologist': '工业心理学家',
      'human factors psychologist': '人因心理学家',
      'experimental psychologist': '实验心理学家',
      'research psychologist': '研究心理学家',
      'neuropsychiatrist': '神经精神科医生',
      'neurologist': '神经科医生',
      'neurosurgeon': '神经外科医生',
      'psychiatric nurse': '精神科护士',
      'psychiatric technician': '精神科技师',
      'psychiatric aide': '精神科护工',
      'psychiatric orderly': '精神科护理员',
      'psychiatric attendant': '精神科护理员',
      'psychiatric assistant': '精神科护理员',
      'psychiatric helper': '精神科护理员',
      'psychiatric orderly': '精神科护理员',
    },
    finance: {
      'initial public offering': '首次公开发行',
      'stock market': '证券市场',
      'bull market': '牛市',
      'bear market': '熊市',
      'market capitalization': '市值',
      'price-earnings ratio': '市盈率',
      'price-to-book ratio': '市净率',
      'dividend yield': '股息率',
      'earnings per share': '每股收益',
      'revenue': '收入',
      'gross profit': '毛利润',
      'operating profit': '营业利润',
      'net profit': '净利润',
      'gross margin': '毛利率',
      'operating margin': '营业利润率',
      'net margin': '净利率',
      'return on assets': '资产回报率',
      'return on equity': '净资产回报率',
      'return on investment': '投资回报率',
      'working capital': '营运资金',
      'cash flow': '现金流',
      'free cash flow': '自由现金流',
      'operating cash flow': '经营活动现金流',
      'investing cash flow': '投资活动现金流',
      'financing cash flow': '筹资活动现金流',
      'capital expenditure': '资本支出',
      'depreciation': '折旧',
      'amortization': '摊销',
      'impairment': '减值',
      'goodwill': '商誉',
      'intangible asset': '无形资产',
      'tangible asset': '有形资产',
      'current asset': '流动资产',
      'fixed asset': '固定资产',
      'liquid asset': '流动资产',
      'non-current asset': '非流动资产',
      'liability': '负债',
      'current liability': '流动负债',
      'long-term liability': '长期负债',
      'deferred tax': '递延所得税',
      'deferred revenue': '递延收入',
      'accounts payable': '应付账款',
      'accounts receivable': '应收账款',
      'inventory': '存货',
      'prepaid expense': '预付费用',
      'accrued expense': '应计费用',
      'deferred expense': '递延费用',
      'bond': '债券',
      'debenture': '公司债券',
      'coupon': '票面利率',
      'yield': '收益率',
      'maturity': '到期日',
      'face value': '面值',
      'redemption': '赎回',
      'call option': '赎回期权',
      'put option': '回售期权',
      'sinking fund': '偿债基金',
      'indenture': '契约',
      'covenant': '契约条款',
      'credit rating': '信用评级',
      'credit score': '信用评分',
      'credit history': '信用记录',
      'credit report': '信用报告',
      'credit bureau': '信用局',
      'credit limit': '信用额度',
      'credit utilization': '信用利用率',
      'credit monitoring': '信用监控',
      'credit counseling': '信用咨询',
      'debt consolidation': '债务整合',
      'debt settlement': '债务和解',
      'debt management': '债务管理',
      'debt relief': '债务减免',
      'debt forgiveness': '债务豁免',
      'student loan': '学生贷款',
      'mortgage': '抵押贷款',
      'home equity loan': '房屋净值贷款',
      'home equity line of credit': '房屋净值信贷额度',
      'auto loan': '汽车贷款',
      'personal loan': '个人贷款',
      'business loan': '商业贷款',
      'small business loan': '小企业贷款',
      'SBA loan': 'SBA 贷款',
      'bridge loan': '过桥贷款',
      'hard money loan': '硬货币贷款',
      'private money loan': '私人贷款',
      'construction loan': '建设贷款',
      'land loan': '土地贷款',
      'acquisition loan': '收购贷款',
      'term loan': '定期贷款',
      'revolver': '循环信贷',
      'line of credit': '信用额度',
      'letter of credit': '信用证',
      'bank guarantee': '银行担保',
      'standby letter of credit': '备用信用证',
      'commercial paper': '商业票据',
      'banker\'s acceptance': '银行承兑汇票',
      'certificate of deposit': '存款证',
      'money market account': '货币市场账户',
      'savings account': '储蓄账户',
      'checking account': '支票账户',
      'deposit account': '存款账户',
      'time deposit': '定期存款',
      'demand deposit': '活期存款',
      'negotiable order of withdrawal': '可转让支付命令账户',
      'share draft account': '股份提款账户',
      'NOW account': '可转让支付命令账户',
      'CD': '存款证',
      'IRA': '个人退休账户',
      'Roth IRA': '罗斯个人退休账户',
      'traditional IRA': '传统个人退休账户',
      '401(k)': '401(k) 计划',
      '403(b)': '403(b) 计划',
      '457(b)': '457(b) 计划',
      'pension': '养老金',
      'defined benefit plan': '固定收益计划',
      'defined contribution plan': '固定缴款计划',
      'annuity': '年金',
      'life insurance': '人寿保险',
      'term life insurance': '定期寿险',
      'whole life insurance': '终身寿险',
      'universal life insurance': '万能寿险',
      'variable life insurance': '变额寿险',
      'endowment insurance': '养老保险',
      'health insurance': '健康保险',
      'medical insurance': '医疗保险',
      'dental insurance': '牙科保险',
      'vision insurance': '视力保险',
      'disability insurance': '残疾保险',
      'long-term care insurance': '长期护理保险',
      'long-term disability insurance': '长期残疾保险',
      'workers\' compensation': '工伤赔偿',
      'unemployment insurance': '失业保险',
      'auto insurance': '汽车保险',
      'homeowner\'s insurance': '房屋保险',
      'renter\'s insurance': '租客保险',
      'flood insurance': '洪水保险',
      'earthquake insurance': '地震保险',
      'hazard insurance': '灾害保险',
      'title insurance': '产权保险',
      'professional liability insurance': '职业责任保险',
      'errors and omissions insurance': '错误与遗漏保险',
      'directors and officers insurance': '董事及高管责任保险',
      'employment practices liability insurance': '雇佣行为责任保险',
      'cyber liability insurance': '网络责任保险',
      'product liability insurance': '产品责任保险',
      'general liability insurance': '一般责任保险',
      'umbrella insurance': '伞式保险',
      'excess insurance': '超额保险',
      'reinsurance': '再保险',
      'self-insurance': '自保',
      'captive insurance': '自保保险',
      'risk retention': '风险自留',
      'risk pooling': '风险集中',
      'risk transfer': '风险转移',
      'risk financing': '风险融资',
      'alternative risk transfer': '另类风险转移',
      'captive': '自保公司',
      'risk retention group': '风险自留集团',
      'purchasing group': '购买集团',
      'association captive': '协会自保公司',
      'rent-a-captive': '租赁自保公司',
      'protected cell company': '保护单元公司',
      'series LLC': '系列有限责任公司',
      'segregated portfolio company': '隔离投资组合公司',
      'incorporated protected cell company': '注册保护单元公司',
      'segregated cell company': '隔离单元公司',
      'protected cell structure': '保护单元结构',
      'segregated cell structure': '隔离单元结构',
      'ring-fenced structure': '隔离结构',
      'bankruptcy-remote structure': '破产隔离结构',
      'bankruptcy-remote entity': '破产隔离实体',
      'bankruptcy-remote subsidiary': '破产隔离子公司',
      'bankruptcy-remote trust': '破产隔离信托',
      'bankruptcy-remote special purpose vehicle': '破产隔离特殊目的载体',
      'bankruptcy-remote special purpose entity': '破产隔离特殊目的实体',
      'bankruptcy-remote SPE': '破产隔离特殊目的实体',
      'bankruptcy-remote SPV': '破产隔离特殊目的载体',
      'bankruptcy-remote trust': '破产隔离信托',
      'bankruptcy-remote LLC': '破产隔离有限责任公司',
      'bankruptcy-remote partnership': '破产隔离合伙企业',
      'bankruptcy-remote limited partnership': '破产隔离有限合伙企业',
      'bankruptcy-remote limited liability partnership': '破产隔离有限责任合伙企业',
      'bankruptcy-remote limited liability company': '破产隔离有限责任公司',
      'bankruptcy-remote corporation': '破产隔离公司',
      'bankruptcy-remote trust': '破产隔离信托',
      'bankruptcy-remote foundation': '破产隔离基金会',
      'bankruptcy-remote charitable trust': '破产隔离慈善信托',
      'bankruptcy-remote nonprofit': '破产隔离非营利组织',
      'bankruptcy-remote foundation': '破产隔离基金会',
      'bankruptcy-remote charitable remainder trust': '破产隔离慈善剩余信托',
      'bankruptcy-remote charitable lead trust': '破产隔离慈善领先信托',
      'bankruptcy-remote charitable income trust': '破产隔离慈善收入信托',
      'bankruptcy-remote charitable unitrust': '破产隔离慈善单一信托',
      'bankruptcy-remote charitable annuity trust': '破产隔离慈善年金信托',
      'bankruptcy-remote charitable remainder annuity trust': '破产隔离慈善剩余年金信托',
      'bankruptcy-remote charitable remainder unitrust': '破产隔离慈善剩余单一信托',
      'bankruptcy-remote charitable lead annuity trust': '破产隔离慈善领先年金信托',
      'bankruptcy-remote charitable lead unitrust': '破产隔离慈善领先单一信托',
      'bankruptcy-remote charitable income trust': '破产隔离慈善收入信托',
      'bankruptcy-remote charitable annuity trust': '破产隔离慈善年金信托',
      'bankruptcy-remote charitable unitrust': '破产隔离慈善单一信托',
    },
  };

  // 获取当前生效的术语库（自定义 + 内置合并，自定义优先级更高）
  function getTermsMap(settings) {
    const map = {};
    // 先加内置（低优先级）
    const builtin = settings.builtinTerms || 'medical';
    if (builtin === 'all') {
      Object.values(BUILTIN_TERMS).forEach((m) => Object.assign(map, m));
    } else if (BUILTIN_TERMS[builtin]) {
      Object.assign(map, BUILTIN_TERMS[builtin]);
    }
    // 自定义覆盖内置（高优先级）
    Object.assign(map, settings.customTerms || {});
    return map;
  }

  // 术语替换：在翻译前把原文中的术语替换为「占位符」，翻译后再还原为目标术语
  // 这样可避免翻译引擎把专业术语译走样。
  // 返回 { text, restoreMap }，翻译后调用 restoreTerms(text, restoreMap) 还原
  function applyTerms(text, termsMap) {
    if (!text || !Object.keys(termsMap).length) return { text, restoreMap: null };
    let result = text;
    const restoreMap = {};
    let idx = 0;
    // 按术语长度降序，先替换长的（避免短的先命中）
    const sorted = Object.keys(termsMap).sort((a, b) => b.length - a.length);
    for (const src of sorted) {
      const tgt = termsMap[src];
      if (!src || !tgt) continue;
      const placeholder = '⟦T' + (idx++) + '⟧';
      // 大小写不敏感匹配，但保留原文大小写
      const re = new RegExp(escapeRegExp(src), 'gi');
      if (re.test(result)) {
        restoreMap[placeholder] = tgt;
        result = result.replace(re, placeholder);
      }
    }
    return { text: result, restoreMap: Object.keys(restoreMap).length ? restoreMap : null };
  }

  function restoreTerms(text, restoreMap) {
    if (!restoreMap || !text) return text;
    let result = text;
    Object.entries(restoreMap).forEach(([placeholder, term]) => {
      result = result.split(placeholder).join(term);
    });
    return result;
  }

  function escapeRegExp(s) {
    return String(s).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }

  // ---------------- 术语库存储 ----------------
  // 用户自定义术语存在 chrome.storage.local（K_TERMS），与设置分开存（可能很多条）
  W.getCustomTerms = function () {
    return new Promise((resolve) => {
      const s = store();
      if (!s) return resolve({});
      try {
        s.get(K_TERMS, (o) => {
          void chrome.runtime.lastError;
          resolve((o && o[K_TERMS]) || {});
        });
      } catch (e) { resolve({}); }
    });
  };
  W.setCustomTerms = function (terms) {
    return new Promise((resolve) => {
      const st = store();
      if (!st) return resolve(false);
      try {
        st.set({ [K_TERMS]: terms }, () => { void chrome.runtime.lastError; resolve(true); });
      } catch (e) { resolve(false); }
    });
  };
  W.addCustomTerm = function (src, tgt) {
    return W.getCustomTerms().then((terms) => {
      terms[src] = tgt;
      return W.setCustomTerms(terms);
    });
  };
  W.removeCustomTerm = function (src) {
    return W.getCustomTerms().then((terms) => {
      delete terms[src];
      return W.setCustomTerms(terms);
    });
  };

  // ---------------- 生词本 + SM-2 复习引擎 ----------------
  // SM-2 算法：根据用户反馈计算下次复习时间和掌握度
  // easeFactor 最小 1.3，默认 2.5；interval 单位天
  function sm2Calc(quality, easeFactor, interval, repetitions) {
    let newEF = easeFactor + (0.1 - (5 - quality) * (0.08 + (5 - quality) * 0.02));
    if (newEF < 1.3) newEF = 1.3;
    let newInterval;
    if (quality < 3) {
      newInterval = 1;
      repetitions = 0;
    } else {
      repetitions += 1;
      if (repetitions === 1) newInterval = 1;
      else if (repetitions === 2) newInterval = 6;
      else newInterval = Math.round(interval * newEF);
    }
    return {
      easeFactor: newEF,
      interval: newInterval,
      repetitions: repetitions,
      nextReview: Date.now() + newInterval * 24 * 60 * 60 * 1000
    };
  }

  // 掌握度衰减：每天衰减 0.05，最低 0.1
  function decayMastery(mastery, daysSinceReview) {
    const decayRate = 0.05;
    const decayed = mastery - decayRate * daysSinceReview;
    return Math.max(0.1, decayed);
  }

  // 获取生词列表
  W.getVocabList = async function () {
    return new Promise((resolve) => {
      const s = store();
      if (!s) return resolve([]);
      try {
        s.get('winocr_vocab_v1', (o) => {
          void chrome.runtime.lastError;
          resolve((o && o.winocr_vocab_v1) || []);
        });
      } catch (e) { resolve([]); }
    });
  };

  W.setVocabList = async function (list) {
    return new Promise((resolve) => {
      const st = store();
      if (!st) return resolve(false);
      try {
        st.set({ winocr_vocab_v1: list }, () => { void chrome.runtime.lastError; resolve(true); });
      } catch (e) { resolve(false); }
    });
  };

  // 添加生词
  W.addVocab = async function (word, translation, source, context) {
    const vocab = await W.getVocabList();
    const existing = vocab.find((v) => v.word === word);
    if (existing) {
      existing.reviewCount = (existing.reviewCount || 0) + 1;
      existing.lastSeen = Date.now();
      await W.setVocabList(vocab);
      return existing;
    }
    const item = {
      id: 'v' + Date.now() + '_' + Math.random().toString(36).slice(2, 8),
      word: word,
      translation: translation || '',
      source: source || '',
      context: context || '',
      mastery: 0.3,
      easeFactor: 2.5,
      interval: 1,
      repetitions: 0,
      reviewCount: 0,
      nextReview: Date.now(),
      createdAt: Date.now(),
      lastSeen: Date.now()
    };
    vocab.push(item);
    await W.setVocabList(vocab);
    return item;
  };

  // 自动从翻译记录采集生词
  W.autoCollectFromHistory = async function (maxItems) {
    const hist = await W.getHistory();
    let count = 0;
    for (const rec of hist) {
      if (count >= (maxItems || 50)) break;
      const text = rec.original || '';
      if (!text) continue;
      const words = text.split(/[\s,;.!?;:'"()]+/).filter((w) => w.length >= 2 && /^[a-zA-Z\u4e00-\u9fa5]+$/.test(w));
      for (const word of words.slice(0, 10)) {
        await W.addVocab(word, rec.translation || '', rec.source || '', text);
        count++;
      }
    }
    return count;
  };

  // 获取待复习列表
  W.getReviewQueue = async function (limit) {
    const vocab = await W.getVocabList();
    const now = Date.now();
    const updated = vocab.map((v) => {
      const daysSince = (now - (v.lastReview || v.createdAt)) / (24 * 60 * 60 * 1000);
      v.mastery = decayMastery(v.mastery, daysSince);
      return v;
    });
    await W.setVocabList(updated);
    return updated
      .filter((v) => v.nextReview <= now)
      .sort((a, b) => a.nextReview - b.nextReview)
      .slice(0, limit || 20);
  };

  // 提交复习结果（quality: 0-5）
  W.submitReview = async function (id, quality) {
    const vocab = await W.getVocabList();
    const item = vocab.find((v) => v.id === id);
    if (!item) return null;
    const calc = sm2Calc(quality, item.easeFactor, item.interval, item.repetitions);
    item.easeFactor = calc.easeFactor;
    item.interval = calc.interval;
    item.repetitions = calc.repetitions;
    item.nextReview = calc.nextReview;
    item.lastReview = Date.now();
    item.reviewCount = (item.reviewCount || 0) + 1;
    const masteryDelta = (quality - 3) * 0.15;
    item.mastery = Math.min(1, Math.max(0.1, item.mastery + masteryDelta));
    await W.setVocabList(vocab);
    return item;
  };

  // 删除生词
  W.removeVocab = async function (id) {
    const vocab = await W.getVocabList();
    const filtered = vocab.filter((v) => v.id !== id);
    await W.setVocabList(filtered);
    return true;
  };

  // 学习统计
  W.getStudyStats = async function () {
    const vocab = await W.getVocabList();
    const now = Date.now();
    const today = new Date().toISOString().slice(0, 10);
    const reviewedToday = vocab.filter((v) => {
      const d = new Date(v.lastReview || 0).toISOString().slice(0, 10);
      return d === today;
    });
    return {
      total: vocab.length,
      mastered: vocab.filter((v) => v.mastery >= 0.95).length,
      learning: vocab.filter((v) => v.mastery >= 0.3 && v.mastery < 0.95).length,
      newWords: vocab.filter((v) => v.mastery < 0.3).length,
      reviewedToday: reviewedToday.length,
      dueToday: vocab.filter((v) => {
        const d = new Date(v.nextReview).toISOString().slice(0, 10);
        return d === today;
      }).length,
      avgMastery: vocab.length ? (vocab.reduce((s, v) => s + v.mastery, 0) / vocab.length).toFixed(2) : 0
    };
  };

  // ---------------- OCR（图片 dataUrl -> 文本，走 SF 视觉模型） ----------------
  W.ocrSF = async function (dataUrl, opts) {
    opts = opts || {};
    const body = {
      model: normalizeOcrModel(opts.sfOcrModel),
      messages: [{
        role: 'user',
        content: [
          { type: 'text', text: 'OCR all text in this image. Output only the recognized text, preserving line breaks.' },
          { type: 'image_url', image_url: { url: dataUrl } }
        ]
      }],
      max_tokens: 4096
    };
    try {
      return await sfChat(opts, body);
    } catch (e) {
      // 20012 = 模型名不存在（常见于漏写 `PaddlePaddle/` 前缀）
      if (/does not exist|not exist|20012/i.test(String(e && e.message))) {
        throw new Error('OCR 模型名不存在（' + normalizeOcrModel(opts.sfOcrModel) +
          '）。硅基流动上要写全称：PaddlePaddle/PaddleOCR-VL-1.5 或 deepseek-ai/DeepSeek-OCR（扩展「选项」里可改）');
      }
      throw e;
    }
  };

  // ---------------- Markdown 生成 ----------------
  function pad(n) { return String(n).padStart(2, '0'); }
  function fmtDateTime(iso) { const d = new Date(iso); return pad(d.getMonth() + 1) + '-' + pad(d.getDate()) + ' ' + pad(d.getHours()) + ':' + pad(d.getMinutes()); }
  function fmtDate(iso) { const d = new Date(iso); return d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()); }

  W.groupByDate = function (records) {
    const m = {};
    records.forEach((r) => { const k = fmtDate(r.ts); (m[k] = m[k] || []).push(r); });
    return m;
  };

  W.recordsToMarkdown = function (records) {
    let out = '';
    records.slice().sort((a, b) => (a.ts < b.ts ? -1 : 1)).forEach((r) => {
      out += '## ' + fmtDateTime(r.ts);
      if (r.source) out += ' · [' + (r.source.length > 40 ? r.source.slice(0, 40) + '…' : r.source) + '](' + r.source + ')';
      out += '\n\n';
      if (r.type === 'image' && r.image) out += '![' + (r.ocr || '截图') + '](' + r.image + ')\n\n';
      out += '**原文**: ' + (r.original || r.ocr || '') + '\n\n';
      out += '**译文**: ' + (r.translation || '') + '\n\n';
      if (r.terms && r.terms.length) out += '**术语**: ' + r.terms.map((t) => '[[' + t + ']]').join(' / ') + '\n\n';
    });
    return out;
  };

  W.buildDailyMarkdown = function (records) {
    const groups = W.groupByDate(records);
    let md = '';
    Object.keys(groups).sort().forEach((date) => {
      md += '---\n\n# WinOCR ' + date + '\n\n' + W.recordsToMarkdown(groups[date]);
    });
    return md;
  };

  // ---------------- 零依赖 ZIP（store，无压缩） ----------------
  function crc32(buf) {
    let c = ~0;
    for (let i = 0; i < buf.length; i++) { c ^= buf[i]; for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xEDB88320 & -(c & 1)); }
    return ~c >>> 0;
  }
  W.makeZip = function (files) { // files: [{name, data:Uint8Array}]
    const chunks = []; let offset = 0; const central = [];
    const enc = (s) => new TextEncoder().encode(s);
    files.forEach((f) => {
      const name = enc(f.name); const data = f.data; const crc = crc32(data); const size = data.length;
      const local = new Uint8Array(30 + name.length); const dv = new DataView(local.buffer);
      dv.setUint32(0, 0x04034b50, true); dv.setUint16(4, 20, true); dv.setUint16(6, 0, true);
      dv.setUint16(8, 0, true); dv.setUint16(10, 0, true); dv.setUint32(14, crc, true);
      dv.setUint32(18, size, true); dv.setUint32(22, size, true); dv.setUint16(26, name.length, true);
      dv.setUint16(28, 0, true); local.set(name, 30);
      chunks.push(local); chunks.push(data);
      const cen = new Uint8Array(46 + name.length); const cd = new DataView(cen.buffer);
      cd.setUint32(0, 0x02014b50, true); cd.setUint16(4, 20, true); cd.setUint16(6, 20, true);
      cd.setUint16(8, 0, true); cd.setUint16(10, 0, true); cd.setUint16(12, 0, true);
      cd.setUint32(16, crc, true); cd.setUint32(20, size, true); cd.setUint32(24, size, true);
      cd.setUint16(28, name.length, true); cd.setUint16(30, 0, true); cd.setUint16(32, 0, true);
      cd.setUint16(34, 0, true); cd.setUint16(36, 0, true); cd.setUint32(38, offset, true); cd.setUint32(42, 0, true);
      cen.set(name, 46); central.push(cen); offset += local.length + data.length;
    });
    const cdStart = offset; let cdSize = 0; central.forEach((c) => { chunks.push(c); cdSize += c.length; });
    const end = new Uint8Array(22); const ed = new DataView(end.buffer);
    ed.setUint32(0, 0x06054b50, true); ed.setUint16(8, files.length, true); ed.setUint16(10, files.length, true);
    ed.setUint32(12, cdSize, true); ed.setUint32(16, cdStart, true); ed.setUint16(20, 0, true);
    chunks.push(end);
    return new Blob(chunks, { type: 'application/zip' });
  };

  // ---------------- 导出 ----------------
  // 文件夹（File System Access，真实写文件，免解压）
  // ---------------- AI 对话历史持久化与导出 ----------------
  // 多轮对话保存、列表管理、导出（Markdown / JSON / 文本）
  const K_CHAT_HISTORY = 'winocr_chat_history_v1';

  W.getChatList = async function () {
    return new Promise((resolve) => {
      const s = store();
      if (!s) return resolve([]);
      try {
        s.get(K_CHAT_HISTORY, (o) => {
          void chrome.runtime.lastError;
          const list = (o && o[K_CHAT_HISTORY]) || [];
          resolve(list.map((c) => ({
            id: c.id, title: c.title || '未命名对话',
            engine: c.engine || 'sf', model: c.model || '',
            messageCount: (c.messages || []).length,
            createdAt: c.createdAt, updatedAt: c.updatedAt
          })));
        });
      } catch (e) { resolve([]); }
    });
  };

  W.getChatListFull = async function () {
    return new Promise((resolve) => {
      const s = store();
      if (!s) return resolve([]);
      try {
        s.get(K_CHAT_HISTORY, (o) => { void chrome.runtime.lastError; resolve((o && o[K_CHAT_HISTORY]) || []); });
      } catch (e) { resolve([]); }
    });
  };

  W.createChat = async function (title, engine, model) {
    const list = await W.getChatListFull();
    const chat = {
      id: 'c' + Date.now() + '_' + Math.random().toString(36).slice(2, 8),
      title: title || '新对话', engine: engine || 'sf', model: model || '',
      messages: [], createdAt: Date.now(), updatedAt: Date.now()
    };
    list.push(chat);
    await W.setChatListFull(list);
    return chat;
  };

  W.saveChat = async function (id, messages, title, engine, model) {
    const list = await W.getChatListFull();
    const chat = list.find((c) => c.id === id);
    if (!chat) return false;
    chat.messages = messages;
    chat.updatedAt = Date.now();
    if (title) chat.title = title;
    if (engine) chat.engine = engine;
    if (model) chat.model = model;
    await W.setChatListFull(list);
    return true;
  };

  W.deleteChat = async function (id) {
    const list = await W.getChatListFull();
    await W.setChatListFull(list.filter((c) => c.id !== id));
    return true;
  };

  W.renameChat = async function (id, title) {
    const list = await W.getChatListFull();
    const chat = list.find((c) => c.id === id);
    if (!chat) return false;
    chat.title = title || chat.title;
    chat.updatedAt = Date.now();
    await W.setChatListFull(list);
    return true;
  };

  W.exportChatToMarkdown = function (chat) {
    const lines = ['# ' + (chat.title || '未命名对话'), ''];
    lines.push('> 引擎：' + (chat.engine || 'sf') + ' · 模型：' + (chat.model || '未知'));
    lines.push('> 导出时间：' + new Date().toLocaleString(), '');
    (chat.messages || []).forEach((m) => {
      const role = m.role === 'user' ? '**你**' : m.role === 'assistant' ? '**AI**' : '**系统**';
      lines.push(role + '：', '', m.content || '', '', '---', '');
    });
    return lines.join('\n');
  };

  W.exportChatToJSON = function (chat) {
    return JSON.stringify({ title: chat.title, engine: chat.engine, model: chat.model, exportedAt: new Date().toISOString(), messages: chat.messages }, null, 2);
  };

  W.exportChatToText = function (chat) {
    const lines = ['=== ' + (chat.title || '未命名对话') + ' ==='];
    lines.push('引擎：' + (chat.engine || 'sf') + ' · 模型：' + (chat.model || '未知'));
    lines.push('导出时间：' + new Date().toLocaleString(), '');
    (chat.messages || []).forEach((m) => {
      const role = m.role === 'user' ? '你' : m.role === 'assistant' ? 'AI' : '系统';
      lines.push('[' + role + '] ' + (m.content || ''), '');
    });
    return lines.join('\n');
  };

  W.downloadChat = function (chat, format) {
    let content, mime, ext;
    if (format === 'json') { content = W.exportChatToJSON(chat); mime = 'application/json'; ext = 'json'; }
    else if (format === 'text') { content = W.exportChatToText(chat); mime = 'text/plain'; ext = 'txt'; }
    else { content = W.exportChatToMarkdown(chat); mime = 'text/markdown'; ext = 'md'; }
    const blob = new Blob([content], { type: mime + ';charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = (chat.title || 'chat').replace(/[\\/:*?"<>|]/g, '_') + '.' + ext;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };

  // ---------------- 导出服务（翻译记录） ----------------
  W.exportToFolder = async function (records) {
    if (!global.showDirectoryPicker) throw new Error('当前浏览器不支持 File System Access，请改用 ZIP 或升级 Edge/Chrome');
    const dir = await global.showDirectoryPicker({ mode: 'readwrite' });
    const groups = W.groupByDate(records); let n = 0;
    for (const date of Object.keys(groups).sort()) {
      const f = await dir.getFileHandle('WinOCR-' + date + '.md', { create: true });
      const w = await f.createWritable();
      await w.write('# WinOCR ' + date + '\n\n' + W.recordsToMarkdown(groups[date]));
      await w.close(); n++;
    }
    return n;
  };

  // ZIP 归档（单文件可移植）
  W.exportToZip = async function (records) {
    const groups = W.groupByDate(records); const files = [];
    for (const date of Object.keys(groups).sort()) {
      files.push({ name: 'WinOCR-' + date + '.md', data: new TextEncoder().encode('# WinOCR ' + date + '\n\n' + W.recordsToMarkdown(groups[date])) });
    }
    const blob = W.makeZip(files);
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = 'WinOCR-' + new Date().toISOString().slice(0, 10) + '.zip';
    document.body.appendChild(a); a.click(); a.remove();
    return files.length;
  };

  // Obsidian Local REST API
  W.exportToObsidian = async function (records, settings) {
    const base = (settings.obsidianUrl || 'http://127.0.0.1:27123').replace(/\/+$/, '');
    const key = settings.obsidianKey || '';
    const groups = W.groupByDate(records); let n = 0;
    for (const date of Object.keys(groups).sort()) {
      const md = '# WinOCR ' + date + '\n\n' + W.recordsToMarkdown(groups[date]);
      const r = await fetch(base + '/vault/WinOCR/' + ('WinOCR-' + date + '.md'), {
        method: 'PUT',
        headers: { 'Authorization': 'Bearer ' + key, 'Content-Type': 'text/markdown' },
        body: md
      });
      if (r.ok) n++; else throw new Error('Obsidian 返回 ' + r.status);
    }
    return n;
  };

  // 乐享（占位：填入端点 + token 即可启用）
  W.exportToLexiang = async function (records, settings) {
    const ep = settings.lexiangEndpoint;
    if (!ep) throw new Error('请在设置中配置乐享 API 端点');
    const body = {
      title: 'WinOCR 导出 ' + new Date().toISOString().slice(0, 10),
      format: 'markdown',
      content: W.buildDailyMarkdown(records)
    };
    const r = await fetch(ep, {
      method: 'POST',
      headers: { 'Authorization': 'Bearer ' + (settings.lexiangToken || ''), 'Content-Type': 'application/json' },
      body: JSON.stringify(body)
    });
    if (!r.ok) throw new Error('乐享返回 ' + r.status);
    return true;
  };

  // ---------------- 通知推送（M1 用户交互管理器） ----------------
  // 翻译完成 / 失败的桌面通知。MV3 通过 chrome.notifications 实现；
  // 无权限或非扩展上下文（如纯页面）时降级为 console，不影响主流程。
  //   level: 'success' | 'error' | 'info'
  let _notifSeq = 0;
  W.notify = function (title, message, level) {
    try {
      if (typeof chrome === 'undefined' || !chrome.notifications) {
        console.log('[WinOCR] ' + (level || 'info') + '：' + title + ' — ' + message);
        return;
      }
      const id = 'winocr-' + (level || 'info') + '-' + (++_notifSeq % 1000);
      const iconUrl = (chrome.runtime && chrome.runtime.getURL)
        ? chrome.runtime.getURL('icons/icon128.png') : '';
      chrome.notifications.create(id, {
        type: 'basic',
        iconUrl: iconUrl,
        title: title || 'WinOCR',
        message: String(message || ''),
        priority: level === 'error' ? 2 : 1
      }, () => { void chrome.runtime.lastError; });
    } catch (e) {
      try { console.warn('[WinOCR] notify 失败:', e); } catch (e2) {}
    }
  };

  // ---------------- 内容采集 / 渲染服务（对齐计划 §6.2 抽象接口） ----------------
  // 采用"注册实现"模式：具体 DOM 逻辑在 content.js 中注册，这里只暴露统一形态，
  // 从而让宿主/后台/侧栏都能以一致签名调用，无需关心底层实现。
  //   ContentService.capture()      -> ContentCapture { text, lang, metadata }
  //   RenderingService.render(...)  -> void （按 mode 渲染：tooltip / overlay / bilingual）
  W.ContentService = {
    _impl: null,
    register: function (impl) { this._impl = impl; },
    capture: function () { return this._impl ? this._impl() : null; }
  };
  W.RenderingService = {
    _impl: null,
    register: function (impl) { this._impl = impl; },
    render: function (result, metadata) { if (this._impl) this._impl(result, metadata); }
  };

})(typeof window !== 'undefined' ? window : globalThis);

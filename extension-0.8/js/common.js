// js/common.js — 兼容层（薄入口）
// 被 content.js / sidepanel.js / options.js / background.js 复用
// 内部通过动态 import() 加载 ES module，对外保持 WINOCR.* 全局命名空间不变。
// 注意：background.js 用 importScripts 加载本文件，故本文件必须是传统脚本（非 ES module）。
(function (global) {
  'use strict';
  const W = (global.WINOCR = {});

  // ---------------- 常量定义（与模块内保持一致，供直接使用） ----------------
  const SF_URL = 'https://api.siliconflow.cn/v1';
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

  // ---------------- 免费模型清单 ----------------
  W.FREE_TEXT_MODELS = [
    { id: 'Qwen/Qwen3-8B', note: '快（实测 2.5s）· 翻译推荐' },
    { id: 'deepseek-ai/DeepSeek-R1-0528-Qwen3-8B', note: '推理模型，慢 4 倍（9.5–12.3s）' }
  ];
  W.FREE_OCR_MODELS = [
    { id: 'PaddlePaddle/PaddleOCR-VL-1.5', note: '文字对但极慢（103–121s）' },
    { id: 'deepseek-ai/DeepSeek-OCR', note: '输出不稳定（会退化成乱码）' }
  ];

  W.DEFAULT_SETTINGS = {
    engine: 'sf',
    translateEngine: 'sf',
    sfKey: '',
    sfUrl: 'https://api.siliconflow.cn/v1',
    sfModel: 'Qwen/Qwen3-8B',
    sfOcrModel: 'PaddlePaddle/PaddleOCR-VL-1.5',
    ocrEngine: 'local',
    localOcrTier: 'tiny',
    srcLang: 'en',
    tgtLang: 'zh',
    displayMode: 'inline',
    nativeHost: false,
    hotkey: 'ctrl+shift+m',
    quitHotkey: 'ctrl+alt+q',
    mymemoryEmail: '',
    ollamaUrl: 'http://localhost:11434',
    ollamaModel: '',
    termEnabled: false,
    customTerms: {},
    builtinTerms: 'medical',
    asrEngine: 'browser',
    asrLang: 'auto',
    sfAsrModel: 'FunAudioLLM/SenseVoiceSmall',
    autoSendAsr: false,
    ttsVoice: 'auto',
    ttsRate: 1.0,
    ttsPitch: 1.0,
    autoSpeak: false,
    exportTargets: { folder: true, zip: false, obsidian: false, lexiang: false },
    obsidianUrl: 'http://127.0.0.1:27123',
    obsidianKey: '',
    lexiangEndpoint: '',
    lexiangToken: ''
  };

  // ---------------- 模块懒加载缓存 ----------------
  const moduleCache = {};

  /**
   * 懒加载模块并返回指定函数
   * 首次调用时动态 import 模块，后续调用直接使用缓存
   */
  function lazyImport(moduleName, fnName) {
    return async function (...args) {
      if (!moduleCache[moduleName]) {
        moduleCache[moduleName] = await import('./modules/' + moduleName + '.js');
      }
      return moduleCache[moduleName][fnName](...args);
    };
  }

  /**
   * 懒加载模块并返回指定常量
   */
  function lazyImportConst(moduleName, constName) {
    // 立即发起加载（不阻塞），但返回同步默认值
    // 注意：常量在模块加载后更新，但大多数使用场景是同步的
    // 对于需要精确常量的场景，使用 lazyImport 异步获取
    if (!moduleCache['_const_' + moduleName]) {
      moduleCache['_const_' + moduleName] = import('./modules/' + moduleName + '.js')
        .then((mod) => mod[constName]);
    }
    return moduleCache['_const_' + moduleName];
  }

  // ---------------- 存储层函数 ----------------
  W.extAlive = function () {
    try { return !!(typeof chrome !== 'undefined' && chrome.runtime && chrome.runtime.id); }
    catch (e) { return false; }
  };
  W.getSettings = lazyImport('storage', 'getSettings');
  W.setSettings = lazyImport('storage', 'setSettings');
  W.getHistory = lazyImport('storage', 'getHistory');
  W.addRecord = lazyImport('storage', 'addRecord');
  W.clearHistory = lazyImport('storage', 'clearHistory');
  W.getVocabList = lazyImport('storage', 'getVocabList');
  W.setVocabList = lazyImport('storage', 'setVocabList');
  W.getHistoryByProject = lazyImport('storage', 'getHistoryByProject');
  W.clearHistoryByProject = lazyImport('storage', 'clearHistoryByProject');
  W.getVocabByProject = lazyImport('storage', 'getVocabByProject');
  W.clearVocabByProject = lazyImport('storage', 'clearVocabByProject');
  W.getProjects = lazyImport('storage', 'getProjects');
  W.addProject = lazyImport('storage', 'addProject');
  W.removeProject = lazyImport('storage', 'removeProject');
  W.getChatList = lazyImport('storage', 'getChatList');
  W.getChatListFull = lazyImport('storage', 'getChatListFull');
  W.getCustomTerms = lazyImport('storage', 'getCustomTerms');
  W.setCustomTerms = lazyImport('storage', 'setCustomTerms');

  // ---------------- 翻译引擎函数 ----------------
  W.translate = lazyImport('translate', 'translate');
  W.translateSF = lazyImport('translate', 'translateSF');
  W.translateMyMemory = lazyImport('translate', 'translateMyMemory');
  W.translateOllama = lazyImport('translate', 'translateOllama');
  W.listOllamaModels = lazyImport('translate', 'listOllamaModels');
  W.ollamaHealth = lazyImport('translate', 'ollamaHealth');
  W.chat = lazyImport('translate', 'chat');

  // ---------------- OCR 引擎函数 ----------------
  W.ocrSF = lazyImport('ocr', 'ocrSF');
  W.extractPdfText = lazyImport('ocr', 'extractPdfText');
  W.renderPdfPageToImage = lazyImport('ocr', 'renderPdfPageToImage');

  // ---------------- 导出服务函数 ----------------
  W.exportToFolder = lazyImport('export', 'exportToFolder');
  W.exportToZip = lazyImport('export', 'exportToZip');
  W.exportToObsidian = lazyImport('export', 'exportToObsidian');
  W.exportToLexiang = lazyImport('export', 'exportToLexiang');
  W.buildDailyMarkdown = lazyImport('export', 'buildDailyMarkdown');
  W.recordsToMarkdown = lazyImport('export', 'recordsToMarkdown');
  W.groupByDate = lazyImport('export', 'groupByDate');
  W.makeZip = lazyImport('export', 'makeZip');

  // ---------------- 生词本 + SM-2 复习引擎函数 ----------------
  W.addVocab = lazyImport('study', 'addVocab');
  W.removeVocab = lazyImport('study', 'removeVocab');
  W.getReviewQueue = lazyImport('study', 'getReviewQueue');
  W.submitReview = lazyImport('study', 'submitReview');
  W.autoCollectFromHistory = lazyImport('study', 'autoCollectFromHistory');
  W.getStudyStats = lazyImport('study', 'getStudyStats');
  W.sm2Calc = lazyImport('study', 'sm2Calc');
  W.decayMastery = lazyImport('study', 'decayMastery');

  // ---------------- AI 对话管理函数 ----------------
  W.createChat = lazyImport('chat', 'createChat');
  W.saveChat = lazyImport('chat', 'saveChat');
  W.deleteChat = lazyImport('chat', 'deleteChat');
  W.renameChat = lazyImport('chat', 'renameChat');
  W.downloadChat = lazyImport('chat', 'downloadChat');
  W.exportChatToMarkdown = lazyImport('chat', 'exportChatToMarkdown');
  W.exportChatToJSON = lazyImport('chat', 'exportChatToJSON');
  W.exportChatToText = lazyImport('chat', 'exportChatToText');

  // ---------------- 术语库管理函数 ----------------
  W.getTermsMap = lazyImport('terms', 'getTermsMap');
  W.applyTerms = lazyImport('terms', 'applyTerms');
  W.restoreTerms = lazyImport('terms', 'restoreTerms');
  W.addCustomTerm = lazyImport('terms', 'addCustomTerm');
  W.removeCustomTerm = lazyImport('terms', 'removeCustomTerm');
  W.BUILTIN_TERMS = {}; // 占位，实际通过 getTermsMap 使用

  // ---------------- UI 工具函数（同步可用） ----------------
  W.escapeHtml = function (t) {
    return (t || '').replace(/[&<>"]/g, (c) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;'
    }[c]));
  };

  W.fileToDataUrl = function (f) {
    return new Promise((res, rej) => {
      const r = new FileReader();
      r.onload = () => res(r.result);
      r.onerror = rej;
      r.readAsDataURL(f);
    });
  };

  W.showToast = function (text, isErr = false, durationMs = 2600) {
    let el = document.getElementById('toast');
    if (!el) {
      el = document.createElement('div');
      el.id = 'toast';
      el.style.cssText = 'position:fixed;bottom:20px;left:50%;transform:translateX(-50%);' +
        'background:#333;color:#fff;padding:10px 20px;border-radius:6px;font-size:14px;' +
        'opacity:0;transition:opacity .3s;z-index:99999;pointer-events:none;';
      document.body.appendChild(el);
    }
    el.textContent = text;
    el.style.background = isErr ? '#c0392b' : '#333';
    el.style.opacity = '1';
    clearTimeout(el.__timer);
    el.__timer = setTimeout(() => { el.style.opacity = '0'; }, durationMs);
  };

  W.setLoading = function (el, text = '加载中...') {
    if (!el) return;
    el.classList.remove('is-empty');
    el.innerHTML = '<span class="ph">' + W.escapeHtml(text) + '</span>';
  };

  W.showResult = function (el, original, translation, err) {
    if (!el) return;
    el.classList.remove('is-empty');
    if (err) {
      el.innerHTML = '<div class="err">' + W.escapeHtml(err) + '</div>';
      return;
    }
    el.innerHTML =
      '<div><div class="lab">原文</div><div class="o">' + W.escapeHtml(original) + '</div></div>' +
      '<div><div class="t">' + W.escapeHtml(translation) + '</div></div>';
  };

  W.prettyKey = function (key) {
    if (!key) return '';
    return key.split('+').map((p) => {
      const map = { ctrl: 'Ctrl', alt: 'Alt', shift: 'Shift', meta: 'Meta' };
      const lower = p.trim().toLowerCase();
      return map[lower] || p.trim();
    }).join(' + ');
  };

  W.fillSelect = function (el, models, current) {
    if (!el) return;
    el.innerHTML = '';
    models.forEach((m) => {
      const o = document.createElement('option');
      o.value = m.id;
      o.textContent = m.note ? (m.id + ' — ' + m.note) : m.id;
      el.appendChild(o);
    });
    if (current && !models.some((m) => m.id === current)) {
      const o = document.createElement('option');
      o.value = current;
      o.textContent = current + ' — 自定义（不在免费清单内）';
      el.appendChild(o);
    }
    el.value = current || (models[0] && models[0].id) || '';
  };

  // ---------------- 模型名归一化（同步，供翻译模块内部使用） ----------------
  const OCR_ALIASES = {
    'paddleocr-vl-1.5': 'PaddlePaddle/PaddleOCR-VL-1.5',
    'paddleocr-vl': 'PaddlePaddle/PaddleOCR-VL-1.5',
    'paddleocr': 'PaddlePaddle/PaddleOCR-VL-1.5',
    'deepseek-ocr': 'deepseek-ai/DeepSeek-OCR'
  };

  W.normalizeOcrModel = function (name) {
    const n = String(name == null ? '' : name).trim();
    if (!n) return W.DEFAULT_SETTINGS.sfOcrModel;
    return OCR_ALIASES[n.toLowerCase()] || n;
  };

  // ---------------- 预加载关键模块（后台静默加载，不阻塞） ----------------
  // 提前加载常用模块，减少首次调用延迟
  const preloadModules = ['storage', 'translate'];
  preloadModules.forEach((name) => {
    import('./modules/' + name + '.js').then((mod) => {
      moduleCache[name] = mod;
    }).catch(() => {});
  });

})(typeof window !== 'undefined' ? window : globalThis);

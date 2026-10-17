// js/modules/storage.js — 存储层（设置/历史/生词/对话/术语）
// 所有 chrome.storage 读写集中在此模块，其他模块通过 import 使用。

// ---------------- 存储 Key 常量 ----------------
export const K_HIST = 'winocr_history_v1';
export const K_SET = 'winocr_settings_v1';
export const K_TERMS = 'winocr_terms_v1';
export const K_VOCAB = 'winocr_vocab_v1';
export const K_CHAT_HISTORY = 'winocr_chat_history_v1';
export const K_PROJECTS = 'winocr_projects_v1';
export const DEFAULT_PROJECT = '未分类';

// ---------------- 默认设置 ----------------
export const DEFAULT_SETTINGS = {
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

// ---------------- 扩展上下文检测 ----------------
export function extAlive() {
  try { return !!(typeof chrome !== 'undefined' && chrome.runtime && chrome.runtime.id); }
  catch (e) { return false; }
}

function store() {
  try {
    if (!extAlive()) return null;
    return (chrome.storage && chrome.storage.local) ? chrome.storage.local : null;
  } catch (e) { return null; }
}

// ---------------- OCR 模型名归一化 ----------------
const OCR_ALIASES = {
  'paddleocr-vl-1.5': 'PaddlePaddle/PaddleOCR-VL-1.5',
  'paddleocr-vl': 'PaddlePaddle/PaddleOCR-VL-1.5',
  'paddleocr': 'PaddlePaddle/PaddleOCR-VL-1.5',
  'deepseek-ocr': 'deepseek-ai/DeepSeek-OCR'
};

export function normalizeOcrModel(name) {
  const n = String(name == null ? '' : name).trim();
  if (!n) return DEFAULT_SETTINGS.sfOcrModel;
  return OCR_ALIASES[n.toLowerCase()] || n;
}

// ---------------- 设置读写 ----------------
export function getSettings() {
  return new Promise((resolve) => {
    const s = store();
    if (!s) return resolve(Object.assign({}, DEFAULT_SETTINGS));
    try {
      s.get(K_SET, (o) => {
        void chrome.runtime.lastError;
        const merged = Object.assign({}, DEFAULT_SETTINGS, (o && o[K_SET]) || {});
        merged.sfOcrModel = normalizeOcrModel(merged.sfOcrModel);
        if (merged.engine === 'deeplx') merged.engine = 'sf';
        if (merged.translateEngine === 'deeplx') merged.translateEngine = 'sf';
        resolve(merged);
      });
    } catch (e) { resolve(Object.assign({}, DEFAULT_SETTINGS)); }
  });
}

export function setSettings(s) {
  return new Promise((resolve) => {
    const st = store();
    if (!st) return resolve(false);
    try {
      st.set({ [K_SET]: s }, () => { void chrome.runtime.lastError; resolve(true); });
    } catch (e) { resolve(false); }
  });
}

// ---------------- 历史记录读写 ----------------
function hash(s) { let h = 5381; for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) >>> 0; return h.toString(36); }

export function getHistory() {
  return new Promise((resolve) => {
    const s = store();
    if (!s) return resolve([]);
    try {
      s.get(K_HIST, (o) => { void chrome.runtime.lastError; resolve((o && o[K_HIST]) || []); });
    } catch (e) { resolve([]); }
  });
}

export async function addRecord(rec) {
  const list = await getHistory();
  const id = rec.id || (rec.source + ':' + hash(rec.original || '') + ':' + hash(rec.ocr || ''));
  if (list.some((r) => r.id === id)) return list;
  const item = Object.assign(
    { id, ts: new Date().toISOString(), type: 'text', source: '', original: '', translation: '', terms: [], image: '', project: DEFAULT_PROJECT },
    rec, { id }
  );
  list.push(item);
  const st = store();
  if (st) { try { st.set({ [K_HIST]: list }, () => void chrome.runtime.lastError); } catch (e) {} }
  return list;
}

export function clearHistory() {
  return new Promise((resolve) => {
    const st = store();
    if (!st) return resolve(false);
    try { st.set({ [K_HIST]: [] }, () => { void chrome.runtime.lastError; resolve(true); }); }
    catch (e) { resolve(false); }
  });
}

export function clearHistoryByProject(project) {
  return new Promise((resolve) => {
    const st = store();
    if (!st) return resolve(false);
    try {
      st.get(K_HIST, (o) => {
        void chrome.runtime.lastError;
        const list = (o && o[K_HIST]) || [];
        const filtered = project === DEFAULT_PROJECT
          ? list.filter((r) => !r.project || r.project === DEFAULT_PROJECT)
          : list.filter((r) => r.project !== project);
        st.set({ [K_HIST]: filtered }, () => { void chrome.runtime.lastError; resolve(true); });
      });
    } catch (e) { resolve(false); }
  });
}

export function getHistoryByProject(project) {
  return new Promise((resolve) => {
    const s = store();
    if (!s) return resolve([]);
    try {
      s.get(K_HIST, (o) => {
        void chrome.runtime.lastError;
        const list = (o && o[K_HIST]) || [];
        if (project === DEFAULT_PROJECT) {
          resolve(list.filter((r) => !r.project || r.project === DEFAULT_PROJECT));
        } else {
          resolve(list.filter((r) => r.project === project));
        }
      });
    } catch (e) { resolve([]); }
  });
}

// ---------------- 生词本读写 ----------------
export function getVocabList() {
  return new Promise((resolve) => {
    const s = store();
    if (!s) return resolve([]);
    try {
      s.get(K_VOCAB, (o) => {
        void chrome.runtime.lastError;
        resolve((o && o[K_VOCAB]) || []);
      });
    } catch (e) { resolve([]); }
  });
}

export function setVocabList(list) {
  return new Promise((resolve) => {
    const st = store();
    if (!st) return resolve(false);
    try {
      st.set({ [K_VOCAB]: list }, () => { void chrome.runtime.lastError; resolve(true); });
    } catch (e) { resolve(false); }
  });
}

export function getVocabByProject(project) {
  return new Promise((resolve) => {
    const s = store();
    if (!s) return resolve([]);
    try {
      s.get(K_VOCAB, (o) => {
        void chrome.runtime.lastError;
        const list = (o && o[K_VOCAB]) || [];
        if (project === DEFAULT_PROJECT) {
          resolve(list.filter((v) => !v.project || v.project === DEFAULT_PROJECT));
        } else {
          resolve(list.filter((v) => v.project === project));
        }
      });
    } catch (e) { resolve([]); }
  });
}

export function clearVocabByProject(project) {
  return new Promise((resolve) => {
    const st = store();
    if (!st) return resolve(false);
    try {
      st.get(K_VOCAB, (o) => {
        void chrome.runtime.lastError;
        const list = (o && o[K_VOCAB]) || [];
        const filtered = project === DEFAULT_PROJECT
          ? list.filter((v) => !v.project || v.project === DEFAULT_PROJECT)
          : list.filter((v) => v.project !== project);
        st.set({ [K_VOCAB]: filtered }, () => { void chrome.runtime.lastError; resolve(true); });
      });
    } catch (e) { resolve(false); }
  });
}

// ---------------- 项目管理 ----------------
export function getProjects() {
  return new Promise((resolve) => {
    const s = store();
    if (!s) return resolve([DEFAULT_PROJECT]);
    try {
      s.get(K_PROJECTS, (o) => {
        void chrome.runtime.lastError;
        const list = (o && o[K_PROJECTS]) || [];
        if (!list.includes(DEFAULT_PROJECT)) list.unshift(DEFAULT_PROJECT);
        resolve(list);
      });
    } catch (e) { resolve([DEFAULT_PROJECT]); }
  });
}

export async function addProject(name) {
  const projects = await getProjects();
  const nm = (name || '').trim();
  if (!nm || projects.includes(nm)) return projects;
  projects.push(nm);
  const st = store();
  if (st) { try { st.set({ [K_PROJECTS]: projects }, () => void chrome.runtime.lastError); } catch (e) {} }
  return projects;
}

export async function removeProject(name) {
  if (name === DEFAULT_PROJECT) return getProjects();
  const projects = await getProjects();
  const filtered = projects.filter((p) => p !== name);
  const st = store();
  if (st) { try { st.set({ [K_PROJECTS]: filtered }, () => void chrome.runtime.lastError); } catch (e) {} }
  await reassignProject(name, DEFAULT_PROJECT);
  return filtered;
}

async function reassignProject(from, to) {
  const hist = await getHistory();
  let changed = false;
  hist.forEach((r) => { if (r.project === from) { r.project = to; changed = true; } });
  if (changed) { const st = store(); if (st) { try { st.set({ [K_HIST]: hist }, () => void chrome.runtime.lastError); } catch (e) {} } }
  const vocab = await getVocabList();
  let vchanged = false;
  vocab.forEach((v) => { if (v.project === from) { v.project = to; vchanged = true; } });
  if (vchanged) { const st = store(); if (st) { try { st.set({ [K_VOCAB]: vocab }, () => void chrome.runtime.lastError); } catch (e) {} } }
}

// ---------------- 对话历史读写 ----------------
export function getChatList() {
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
}

export function getChatListFull() {
  return new Promise((resolve) => {
    const s = store();
    if (!s) return resolve([]);
    try {
      s.get(K_CHAT_HISTORY, (o) => { void chrome.runtime.lastError; resolve((o && o[K_CHAT_HISTORY]) || []); });
    } catch (e) { resolve([]); }
  });
}

export function setChatListFull(list) {
  return new Promise((resolve) => {
    const st = store();
    if (!st) return resolve(false);
    try {
      st.set({ [K_CHAT_HISTORY]: list }, () => { void chrome.runtime.lastError; resolve(true); });
    } catch (e) { resolve(false); }
  });
}

// ---------------- 术语库存储 ----------------
export function getCustomTerms() {
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
}

export function setCustomTerms(terms) {
  return new Promise((resolve) => {
    const st = store();
    if (!st) return resolve(false);
    try {
      st.set({ [K_TERMS]: terms }, () => { void chrome.runtime.lastError; resolve(true); });
    } catch (e) { resolve(false); }
  });
}

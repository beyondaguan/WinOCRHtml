// js/sidepanel-v2.js — v2 侧栏真实逻辑（传统脚本，依赖 window.WINOCR）
// 由 <script src="js/common.js"> 注入的兼容层暴露 WINOCR.* 全局 API。
(function () {
  'use strict';

  const W = window.WINOCR;
  if (!W) {
    console.error('[sidepanel-v2] WINOCR 未就绪，脚本中止');
    return;
  }

  const DEFAULT_PROJECT = '未分类';

  const $ = (sel) => document.querySelector(sel);
  const $$ = (sel) => Array.prototype.slice.call(document.querySelectorAll(sel));

  // 引擎下拉框：界面文案 <-> 设置键
  const ENGINE_MAP = { SiliconFlow: 'sf', MyMemory: 'mymemory', Ollama: 'ollama' };
  const ENGINE_REV = { sf: 'SiliconFlow', mymemory: 'MyMemory', ollama: 'Ollama', browser: 'SiliconFlow' };

  function esc(t) {
    return (t == null ? '' : String(t)).replace(/[&<>"]/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c];
    });
  }

  function fileToDataUrl(file) {
    return new Promise(function (resolve, reject) {
      const r = new FileReader();
      r.onload = function () { resolve(r.result); };
      r.onerror = reject;
      r.readAsDataURL(file);
    });
  }

  // ---------------- TTS（Web Speech API） ----------------
  const TTS = {
    synth: window.speechSynthesis || null,
    voices: [],
    init: function () {
      if (!this.synth) return;
      try { this.voices = this.synth.getVoices(); } catch (e) {}
      const self = this;
      this.synth.onvoiceschanged = function () {
        try { self.voices = self.synth.getVoices(); } catch (e) {}
        fillVoiceSelect();
      };
    },
    getVoice: function (lang) {
      if (!this.synth) return null;
      if (!this.voices.length) {
        try { this.voices = this.synth.getVoices(); } catch (e) {}
      }
      // 优先使用设置里选定的具体语音（真实 speechSynthesis 语音名，由 fillVoiceSelect 填充）
      const sel = $('#ttsVoice') && $('#ttsVoice').value;
      if (sel && sel !== 'auto') {
        const named = this.voices.find(function (x) { return x.name === sel; });
        if (named) return named;
      }
      const langMap = { zh: 'zh-CN', en: 'en-US', ko: 'ko-KR', fr: 'fr-FR', de: 'de-DE', ja: 'ja-JP' };
      const target = langMap[lang] || lang || 'zh-CN';
      let v = this.voices.find(function (x) { return x.lang === target; });
      if (!v) v = this.voices.find(function (x) { return x.lang && x.lang.indexOf(String(lang)) === 0; });
      if (!v) v = this.voices[0];
      return v || null;
    },
    speak: function (text, lang, opts) {
      if (!this.synth || !text) return;
      this.stop();
      const u = new SpeechSynthesisUtterance(text);
      u.voice = (opts && opts.voice) || this.getVoice(lang);
      u.rate = (opts && opts.rate) || parseFloat(($('#ttsRate') && $('#ttsRate').value) || '1.0');
      u.pitch = (opts && opts.pitch) || parseFloat(($('#ttsPitch') && $('#ttsPitch').value) || '1.0');
      u.lang = (opts && opts.lang) || (lang === 'zh' ? 'zh-CN' : lang === 'en' ? 'en-US' : (lang === 'ko' ? 'ko-KR' : 'zh-CN'));
      u.onstart = function () { const b = $('#speakResult'); if (b) { b.textContent = '🔊'; b.classList.add('speaking'); } };
      u.onend = function () { const b = $('#speakResult'); if (b) { b.textContent = '🔈'; b.classList.remove('speaking'); } };
      u.onerror = function () { const b = $('#speakResult'); if (b) { b.textContent = '🔈'; b.classList.remove('speaking'); } };
      this.current = u;
      this.synth.speak(u);
    },
    stop: function () {
      if (this.synth) { try { this.synth.cancel(); } catch (e) {} }
      const b = $('#speakResult');
      if (b) { b.textContent = '🔈'; b.classList.remove('speaking'); }
    },
    toggle: function (text, lang) {
      if (this.synth && this.synth.speaking) this.stop();
      else this.speak(text, lang);
    }
  };

  // 项目管理
  const panelProject = { history: DEFAULT_PROJECT, study: DEFAULT_PROJECT, export: DEFAULT_PROJECT };
  function currentProject(panel) { return panelProject[panel] || DEFAULT_PROJECT; }
  async function refreshProjectSelect(selectId, current) {
    const sel = $(selectId);
    if (!sel) return;
    let projects = [];
    try { projects = await W.getProjects(); } catch (e) { projects = [DEFAULT_PROJECT]; }
    sel.innerHTML = '';
    projects.forEach(function (p) {
      const o = document.createElement('option');
      o.value = p; o.textContent = p;
      sel.appendChild(o);
    });
    const has = Array.prototype.some.call(sel.options, function (o) { return o.value === (current || DEFAULT_PROJECT); });
    sel.value = has ? (current || DEFAULT_PROJECT) : DEFAULT_PROJECT;
  }
  function askProjectName(defaultVal) {
    let name = prompt('新建项目名称：', defaultVal || '');
    return (name || '').trim();
  }
  async function askConfirm(msg) {
    return confirm(msg);
  }
  function wireProjectBar(newId, delId, selectId, panel, onRefresh) {
    const newBtn = $(newId);
    const delBtn = $(delId);
    const sel = $(selectId);
    if (newBtn) newBtn.addEventListener('click', async function () {
      const name = askProjectName();
      if (!name) return;
      try {
        await W.addProject(name);
        await refreshProjectSelect(selectId, name);
        panelProject[panel] = name;
        if (onRefresh) onRefresh();
      } catch (e) { W.showToast('新建项目失败：' + e.message, true); }
    });
    if (delBtn) delBtn.addEventListener('click', async function () {
      const cur = panelProject[panel] || DEFAULT_PROJECT;
      if (cur === DEFAULT_PROJECT) { W.showToast('默认项目不能删除', true); return; }
      const histN = await W.getHistoryByProject(cur);
      const vocabN = await W.getVocabByProject(cur);
      const ok = await askConfirm('确定删除项目「' + cur + '」？该项目下有 ' + histN.length + ' 条历史、' + vocabN.length + ' 个生词，删除后无法恢复。');
      if (!ok) return;
      try {
        await W.removeProject(cur);
        panelProject[panel] = DEFAULT_PROJECT;
        await refreshProjectSelect(selectId, DEFAULT_PROJECT);
        if (onRefresh) onRefresh();
        W.showToast('项目已删除');
      } catch (e) { W.showToast('删除失败：' + e.message, true); }
    });
    if (sel) sel.addEventListener('change', function () {
      panelProject[panel] = sel.value || DEFAULT_PROJECT;
      if (onRefresh) onRefresh();
    });
  }

  // 用系统真实语音列表填充 TTS 语音下拉（替代原先写死的 Edge-TTS 语音名）
  function fillVoiceSelect(saved) {
    const sel = $('#ttsVoice');
    if (!sel || !window.speechSynthesis) return;
    let voices = [];
    try { voices = window.speechSynthesis.getVoices() || []; } catch (e) {}
    if (!voices.length) return;
    const cur = saved || sel.value || 'auto';
    sel.innerHTML = '';
    const auto = document.createElement('option');
    auto.value = 'auto';
    auto.textContent = '自动选择';
    sel.appendChild(auto);
    voices.forEach(function (v) {
      const o = document.createElement('option');
      o.value = v.name;
      o.textContent = v.name + ' (' + v.lang + ')';
      sel.appendChild(o);
    });
    const has = Array.prototype.some.call(sel.options, function (o) { return o.value === cur; });
    sel.value = has ? cur : 'auto';
  }

  // ---------------- 面板开合 ----------------
  const PANELS = ['settingsPanel', 'historyPanel', 'studyPanel', 'exportPanel'];
  function openPanel(id) {
    PANELS.forEach(function (p) {
      const el = document.getElementById(p);
      if (el) el.classList.toggle('open', p === id);
    });
  }
  function closePanel(id) {
    const el = document.getElementById(id);
    if (el) el.classList.remove('open');
  }

  // ---------------- AI 消息气泡 ----------------
  function appendMsg(role, text) {
    const box = $('#aiMessages');
    if (!box) return;
    const d = document.createElement('div');
    d.style.cssText = 'margin:8px 0;padding:10px 12px;border-radius:12px;max-width:85%;white-space:pre-wrap;' +
      'word-break:break-word;line-height:1.6;font-size:14px;' +
      (role === 'user'
        ? 'margin-left:auto;background:var(--accent);color:#fff;'
        : 'margin-right:auto;background:var(--surface-2);color:var(--text);border:1px solid var(--border);');
    d.textContent = text;
    box.appendChild(d);
    box.scrollTop = box.scrollHeight;
  }

  // ---------------- ASR（语音输入：browser / sf 云端 / whisper 回退） ----------------
  let asrRec = null;
  let asrActive = false;
  let asrMedia = null;   // sf 引擎：MediaStream + MediaRecorder
  let asrChunks = [];

  function setMicUi(on, label) {
    const b = $('#micBtn'); if (b) b.classList.toggle('recording', !!on);
    const st = $('#micStatus'); if (st) st.textContent = label || (on ? '录音中…（再点结束）' : '');
  }

  function finishAsrText(t) {
    const inp = $('#inputText'); if (inp) inp.value = t || '';
    const st = $('#micStatus'); if (st) st.textContent = t ? '识别完成' : '未识别到内容';
    if (t && $('#toggleAutoSendAsr') && $('#toggleAutoSendAsr').classList.contains('on')) {
      const tb = $('#translateBtn'); if (tb) tb.click();
    }
  }

  function startAsrBrowser() {
    const SR = window.SpeechRecognition || window.webkitSpeechRecognition;
    if (!SR) { W.showToast('当前浏览器不支持语音输入', true); return; }
    const rec = new SR();
    rec.continuous = false;
    rec.interimResults = true;
    const langVal = ($('#asrLang') && $('#asrLang').value) || 'auto';
    const langMap = { auto: 'zh-CN', zh: 'zh-CN', en: 'en-US', ko: 'ko-KR' };
    rec.lang = langMap[langVal] || langVal;
    rec.onresult = function (e) {
      let t = '';
      for (let i = e.resultIndex; i < e.results.length; i++) t += e.results[i][0].transcript;
      const inp = $('#inputText'); if (inp) inp.value = t;
      const st = $('#micStatus'); if (st) st.textContent = e.results[0].isFinal ? '识别完成' : '识别中…';
    };
    rec.onerror = function (e) {
      W.showToast('语音识别失败' + (e && e.error ? '：' + e.error : ''), true);
      asrActive = false; setMicUi(false);
    };
    rec.onend = function () { asrActive = false; setMicUi(false); };
    asrRec = rec;
    try {
      rec.start();
      asrActive = true;
      setMicUi(true);
    } catch (err) {
      W.showToast('无法启动语音输入：' + err.message, true);
    }
  }

  async function startAsrSF() {
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia || typeof MediaRecorder === 'undefined') {
      W.showToast('当前环境不支持麦克风采集，回退浏览器识别', true);
      return startAsrBrowser();
    }
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (e) {
      W.showToast('麦克风不可用（' + e.message + '），回退浏览器识别', true);
      return startAsrBrowser();
    }
    const mime = (typeof MediaRecorder.isTypeSupported === 'function' && MediaRecorder.isTypeSupported('audio/webm')) ? 'audio/webm' : '';
    let recorder;
    try {
      recorder = new MediaRecorder(stream, mime ? { mimeType: mime } : undefined);
    } catch (e) {
      stream.getTracks().forEach(function (t) { t.stop(); });
      W.showToast('录音器创建失败（' + e.message + '），回退浏览器识别', true);
      return startAsrBrowser();
    }
    asrChunks = [];
    recorder.ondataavailable = function (e) { if (e.data && e.data.size) asrChunks.push(e.data); };
    recorder.onstop = async function () {
      stream.getTracks().forEach(function (t) { t.stop(); });
      asrActive = false; setMicUi(false, '识别中…');
      try {
        const cur = await W.getSettings();
        const blob = new Blob(asrChunks, { type: mime || 'audio/webm' });
        if (!blob.size) { finishAsrText(''); return; }
        const fd = new FormData();
        fd.append('file', blob, 'asr.webm');
        fd.append('model', ($('#sfAsrModel') && $('#sfAsrModel').value) || 'FunAudioLLM/SenseVoiceSmall');
        const base = (cur.sfUrl || 'https://api.siliconflow.cn/v1').replace(/\/+$/, '');
        const r = await fetch(base + '/audio/transcriptions', {
          method: 'POST',
          headers: { 'Authorization': 'Bearer ' + (cur.sfKey || '') },
          body: fd
        });
        if (!r.ok) throw new Error('HTTP ' + r.status);
        const data = await r.json();
        finishAsrText((data && data.text) || '');
      } catch (e) {
        W.showToast('SiliconFlow 识别失败：' + e.message + '，回退浏览器识别', true);
        startAsrBrowser();
      }
    };
    asrMedia = { stream: stream, recorder: recorder };
    recorder.start();
    asrActive = true;
    setMicUi(true, '录音中…（再点结束）');
  }

  function stopAsr() {
    if (asrMedia && asrMedia.recorder && asrMedia.recorder.state !== 'inactive') {
      try { asrMedia.recorder.stop(); } catch (e) {}
    }
    if (asrRec) { try { asrRec.stop(); } catch (e) {} }
    asrActive = false;
  }

  function startAsr() {
    if (asrActive) { stopAsr(); return; }
    const engine = ($('#asrEngine') && $('#asrEngine').value) || 'browser';
    if (engine === 'sf') return startAsrSF();
    if (engine === 'whisper') W.showToast('本地 Whisper 需主程序原生宿主支持，先用浏览器识别', true);
    return startAsrBrowser();
  }

  // ---------------- OCR 处理 ----------------
  async function handleOcrFile(file) {
    if (!file || !/^image\//.test(file.type)) { W.showToast('请选择图片文件', true); return; }
    try {
      const dataUrl = await fileToDataUrl(file);
      const preview = $('#ocrPreview');
      if (preview) { preview.src = dataUrl; preview.style.display = 'block'; }
      W.showToast('正在识别…');
      const cur = await W.getSettings();
      const text = await W.ocrSF(dataUrl, { sfOcrModel: cur.sfOcrModel, sfKey: cur.sfKey, sfUrl: cur.sfUrl });
      const ocrText = $('#ocrText');
      const ocrResult = $('#ocrResult');
      if (ocrText) ocrText.textContent = text;
      if (ocrResult) ocrResult.style.display = 'block';
      const btn = $('#ocrTranslate'); if (btn) btn.style.display = 'block';
    } catch (e) {
      W.showToast('OCR 失败：' + e.message, true);
    }
  }

  // ---------------- 原生宿主 OCR（图像型 PDF 页用；主程序未连接时报错，由调用方回落云端） ----------------
  function ocrViaHost(dataUrl, timeoutMs) {
    return new Promise(function (resolve, reject) {
      try {
        chrome.runtime.sendMessage(
          { type: 'ocr.viaHost', dataUrl: dataUrl, timeoutMs: timeoutMs || 90000 },
          function (res) {
            if (chrome.runtime.lastError) return reject(new Error(chrome.runtime.lastError.message));
            if (!res) return reject(new Error('原生宿主无响应'));
            if (res.error) return reject(new Error(res.error));
            if (!res.text) return reject(new Error('原生宿主未返回文本'));
            resolve(res.text);
          }
        );
      } catch (e) { reject(e); }
    });
  }

  // ---------------- PDF 翻译（文本层直翻；无文本的图像页渲染成图 → 原生宿主 OCR → 云端 OCR 兜底） ----------------
  let pdfLastResult = [];

  function renderPdfResults(items) {
    const box = $('#pdfResult');
    if (!box) return;
    box.innerHTML = '';
    items.forEach(function (it) {
      const wrap = document.createElement('div');
      wrap.style.cssText = 'padding:10px 0;border-bottom:1px solid var(--border)';
      const head = document.createElement('div');
      head.style.cssText = 'font-size:11px;color:var(--faint);margin-bottom:4px';
      head.textContent = '第 ' + it.page + ' 页' + (it.viaOcr ? '（图像页 · OCR）' : '');
      const o = document.createElement('div');
      o.style.cssText = 'font-size:13px;color:var(--text);white-space:pre-wrap;word-break:break-word';
      o.textContent = it.src || '（无文本）';
      const t = document.createElement('div');
      t.style.cssText = 'font-size:13px;color:var(--muted);white-space:pre-wrap;word-break:break-word;margin-top:4px';
      t.textContent = it.out || '';
      wrap.appendChild(head); wrap.appendChild(o); wrap.appendChild(t);
      box.appendChild(wrap);
    });
    box.scrollTop = box.scrollHeight;
  }

  async function handlePdfFile(file) {
    if (!file || !(file.type === 'application/pdf' || /\.pdf$/i.test(file.name || ''))) {
      W.showToast('请选择 PDF 文件', true); return;
    }
    const status = $('#pdfStatus');
    const result = $('#pdfResult');
    const dl = $('#pdfDownload');
    if (status) { status.style.display = 'block'; status.textContent = '加载 PDF…'; }
    if (result) { result.innerHTML = ''; result.style.display = 'none'; }
    if (dl) dl.style.display = 'none';
    pdfLastResult = [];
    try {
      const cur = await W.getSettings();
      const tOpts = {
        engine: cur.engine, sfKey: cur.sfKey, sfUrl: cur.sfUrl, sfModel: cur.sfModel,
        mymemoryEmail: cur.mymemoryEmail, ollamaUrl: cur.ollamaUrl, ollamaModel: cur.ollamaModel,
        srcLang: cur.srcLang, tgtLang: cur.tgtLang, _settings: cur
      };
      const pages = await W.extractPdfText(file, function (i, n) {
        if (status) status.textContent = '提取文本 ' + i + '/' + n + ' 页…';
      });
      for (let k = 0; k < pages.length; k++) {
        const p = pages[k];
        if (status) status.textContent = '处理第 ' + p.page + '/' + pages.length + ' 页…';
        let src = (p.text || '').trim();
        let viaOcr = false;
        if (!src) {
          // 图像型页：渲染成图 → 原生宿主 OCR（接主程序后生效）→ 云端 OCR 兜底
          viaOcr = true;
          try {
            const img = await W.renderPdfPageToImage(file, p.page, 2);
            try {
              src = (await ocrViaHost(img, 90000)).trim();
            } catch (eHost) {
              src = (await W.ocrSF(img, { sfOcrModel: cur.sfOcrModel, sfKey: cur.sfKey, sfUrl: cur.sfUrl })).trim();
            }
          } catch (e2) { src = ''; }
        }
        let out = '';
        if (src) {
          try { out = await W.translate(src, tOpts); }
          catch (e3) { out = '（本页翻译失败：' + e3.message + '）'; }
        }
        pdfLastResult.push({ page: p.page, src: src, out: out, viaOcr: viaOcr });
        renderPdfResults(pdfLastResult);
      }
      if (result) result.style.display = 'block';
      if (dl) dl.style.display = 'block';
      const ocrPages = pdfLastResult.filter(function (x) { return x.viaOcr; }).length;
      if (status) status.textContent = '完成：共 ' + pages.length + ' 页（图像页 ' + ocrPages + '）';
      const joinedSrc = pdfLastResult.map(function (x) { return x.src; }).filter(Boolean).join('\n');
      const joinedOut = pdfLastResult.map(function (x) { return x.out; }).filter(Boolean).join('\n');
      if (joinedSrc) {
        try {
          await W.addRecord({ type: 'pdf', source: 'panel', original: joinedSrc, translation: joinedOut });
        } catch (e4) {}
      }
    } catch (e) {
      if (status) status.textContent = 'PDF 处理失败：' + e.message;
      W.showToast('PDF 处理失败：' + e.message, true);
    }
  }

  // ---------------- 主初始化 ----------------
  (async function init() {
    let s = {};
    try {
      s = await W.getSettings();
    } catch (e) {
      W.showToast('加载设置失败：' + e.message, true);
    }
    s = s || {};

    // 初始值回填
    if ($('#srcLang')) $('#srcLang').value = s.srcLang || 'auto';
    if ($('#tgtLang')) $('#tgtLang').value = s.tgtLang || 'zh';
    const engineSel = $('#engineSelect');
    if (engineSel) engineSel.value = ENGINE_REV[s.engine] || 'SiliconFlow';
    if ($('#asrEngine')) $('#asrEngine').value = s.asrEngine || 'browser';
    if ($('#asrLang')) $('#asrLang').value = s.asrLang || 'auto';
    if ($('#sfAsrModel')) $('#sfAsrModel').value = s.sfAsrModel || 'FunAudioLLM/SenseVoiceSmall';
    if ($('#ttsVoice')) $('#ttsVoice').value = s.ttsVoice || 'auto';
    if ($('#ttsRate')) $('#ttsRate').value = (s.ttsRate != null ? s.ttsRate : 1.0);
    if ($('#ttsPitch')) $('#ttsPitch').value = (s.ttsPitch != null ? s.ttsPitch : 1.0);
    if (s.autoSendAsr && $('#toggleAutoSendAsr')) $('#toggleAutoSendAsr').classList.add('on');
    if (s.autoSpeak && $('#toggleAutoSpeak')) $('#toggleAutoSpeak').classList.add('on');

    TTS.init();
    fillVoiceSelect(s.ttsVoice || 'auto');

    // ---- Tab 切换 ----
    $$('.tab').forEach(function (tab) {
      tab.addEventListener('click', function () {
        $$('.tab').forEach(function (t) { t.classList.remove('active'); });
        tab.classList.add('active');
        const name = tab.dataset.tab;
        $$('.tab-panel').forEach(function (p) { p.style.display = 'none'; });
        const panel = document.getElementById('panel-' + name);
        if (panel) panel.style.display = 'flex';
      });
    });
    // 默认显示激活的 Tab
    const activeTab = $('.tab.active') || $('.tab');
    if (activeTab) activeTab.click();

    // ---- 翻译 ----
    const translateBtn = $('#translateBtn');
    if (translateBtn) translateBtn.addEventListener('click', async function () {
      const inputEl = $('#inputText');
      const text = ((inputEl && inputEl.value) || '').trim();
      if (!text) { W.showToast('请输入要翻译的文本', true); return; }
      translateBtn.classList.add('loading');
      translateBtn.disabled = true;
      try {
        const cur = await W.getSettings();
        const out = await W.translate(text, {
          engine: cur.engine, sfKey: cur.sfKey, sfUrl: cur.sfUrl, sfModel: cur.sfModel,
          mymemoryEmail: cur.mymemoryEmail, ollamaUrl: cur.ollamaUrl, ollamaModel: cur.ollamaModel,
          srcLang: cur.srcLang, tgtLang: cur.tgtLang, _settings: cur
        });
        const resultText = $('#resultText');
        if (resultText) { resultText.textContent = out; resultText.style.display = 'block'; }
        const ph = document.querySelector('.result-placeholder');
        if (ph) ph.style.display = 'none';
        await W.addRecord({ type: 'text', source: 'panel', original: text, translation: out });
        if (cur.autoSpeak) { try { TTS.speak(out, cur.tgtLang || 'zh'); } catch (e2) {} }
      } catch (e) {
        W.showToast('翻译失败：' + e.message, true);
      } finally {
        translateBtn.classList.remove('loading');
        translateBtn.disabled = false;
      }
    });

    const inputEl = $('#inputText');
    if (inputEl) inputEl.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        if (translateBtn) translateBtn.click();
      }
    });

    // ---- 交换语言 ----
    const swapBtn = $('#swapLang');
    if (swapBtn) swapBtn.addEventListener('click', async function () {
      const a = $('#srcLang'), b = $('#tgtLang');
      if (!a || !b) return;
      const tmp = a.value; a.value = b.value; b.value = tmp;
      try {
        const cur = await W.getSettings();
        cur.srcLang = a.value; cur.tgtLang = b.value;
        await W.setSettings(cur);
      } catch (e) { W.showToast('保存失败：' + e.message, true); }
    });

    // ---- 复制译文 ----
    const copyBtn = $('#copyResult');
    if (copyBtn) copyBtn.addEventListener('click', async function () {
      const txt = (($('#resultText') && $('#resultText').textContent) || '');
      if (!txt) return;
      try { await navigator.clipboard.writeText(txt); W.showToast('已复制译文'); }
      catch (e) { W.showToast('复制失败：' + e.message, true); }
    });

    // ---- 收藏生词 ----
    const saveBtn = $('#saveResult');
    if (saveBtn) saveBtn.addEventListener('click', async function () {
      const orig = ((($('#inputText') && $('#inputText').value) || '').trim());
      const trans = ((($('#resultText') && $('#resultText').textContent) || '').trim());
      if (!orig || !trans) { W.showToast('没有可收藏的内容', true); return; }
      const word = orig.split(/\s+/)[0] || orig;
      try {
        await W.addVocab(word, trans, 'panel', orig);
        W.showToast('已加入生词本');
      } catch (e) { W.showToast('收藏失败：' + e.message, true); }
    });

    // ---- 朗读 ----
    const speakBtn = $('#speakResult');
    if (speakBtn) speakBtn.addEventListener('click', function () {
      const txt = (($('#resultText') && $('#resultText').textContent) || '');
      if (!txt) return;
      TTS.toggle(txt, (($('#tgtLang') && $('#tgtLang').value) || 'zh'));
    });
    const tgtSel = $('#tgtLang');
    if (tgtSel) tgtSel.addEventListener('change', function () { TTS.stop(); });

    // ---- 麦克风（ASR） ----
    const micBtn = $('#micBtn');
    if (micBtn) micBtn.addEventListener('click', startAsr);

    // ---- OCR 交互 ----
    const dropZone = $('#dropZone');
    const ocrFile = $('#ocrFile');
    if (dropZone) {
      dropZone.addEventListener('click', function () { if (ocrFile) ocrFile.click(); });
      dropZone.addEventListener('dragover', function (e) { e.preventDefault(); dropZone.classList.add('drag'); });
      dropZone.addEventListener('dragleave', function () { dropZone.classList.remove('drag'); });
      dropZone.addEventListener('drop', async function (e) {
        e.preventDefault();
        dropZone.classList.remove('drag');
        const f = e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0];
        if (!f) return;
        if (f.type === 'application/pdf' || /\.pdf$/i.test(f.name || '')) await handlePdfFile(f);
        else await handleOcrFile(f);
      });
    }
    if (ocrFile) ocrFile.addEventListener('change', async function () {
      const f = ocrFile.files && ocrFile.files[0];
      if (f) await handleOcrFile(f);
      ocrFile.value = '';
    });

    // ---- PDF 翻译入口 ----
    const pickPdf = $('#pickPdf');
    const pdfFileEl = $('#pdfFile');
    if (pickPdf && pdfFileEl) {
      pickPdf.addEventListener('click', function () { pdfFileEl.click(); });
      pdfFileEl.addEventListener('change', async function () {
        const f = pdfFileEl.files && pdfFileEl.files[0];
        if (f) await handlePdfFile(f);
        pdfFileEl.value = '';
      });
    }
    const pdfDl = $('#pdfDownload');
    if (pdfDl) pdfDl.addEventListener('click', function () {
      if (!pdfLastResult.length) return;
      const txt = pdfLastResult.map(function (x) {
        return '==== 第 ' + x.page + ' 页' + (x.viaOcr ? '（图像页 OCR）' : '') + ' ====\n[原文]\n' +
          (x.src || '（无文本）') + '\n[译文]\n' + (x.out || '');
      }).join('\n\n');
      const blob = new Blob([txt], { type: 'text/plain;charset=utf-8' });
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = 'WinOCR-PDF-' + new Date().toISOString().slice(0, 10) + '.txt';
      document.body.appendChild(a); a.click(); a.remove();
      setTimeout(function () { URL.revokeObjectURL(a.href); }, 5000);
    });
    document.addEventListener('paste', async function (e) {
      const items = e.clipboardData && e.clipboardData.items;
      if (!items) return;
      for (let i = 0; i < items.length; i++) {
        if (items[i].type && items[i].type.indexOf('image/') === 0) {
          const f = items[i].getAsFile();
          if (f) await handleOcrFile(f);
          break;
        }
      }
    });

    // ---- OCR 结果翻译 ----
    const ocrTranslateBtn = $('#ocrTranslate');
    if (ocrTranslateBtn) ocrTranslateBtn.addEventListener('click', async function () {
      const ocrText = $('#ocrText');
      const text = ((ocrText && ocrText.textContent) || '').trim();
      if (!text) return;
      ocrTranslateBtn.classList.add('loading');
      ocrTranslateBtn.disabled = true;
      try {
        const cur = await W.getSettings();
        const out = await W.translate(text, {
          engine: cur.engine, sfKey: cur.sfKey, sfUrl: cur.sfUrl, sfModel: cur.sfModel,
          mymemoryEmail: cur.mymemoryEmail, ollamaUrl: cur.ollamaUrl, ollamaModel: cur.ollamaModel,
          srcLang: cur.srcLang, tgtLang: cur.tgtLang, _settings: cur
        });
        const trans = $('#ocrTrans');
        if (trans) { trans.textContent = out; trans.style.display = 'block'; }
        await W.addRecord({
          type: 'image', source: 'panel', original: text, translation: out,
          ocr: text, image: (($('#ocrPreview') && $('#ocrPreview').src) || '')
        });
        W.showToast('已翻译并保存');
        if (cur.autoSpeak) { try { TTS.speak(out, cur.tgtLang || 'zh'); } catch (e2) {} }
      } catch (e) {
        W.showToast('翻译失败：' + e.message, true);
      } finally {
        ocrTranslateBtn.classList.remove('loading');
        ocrTranslateBtn.disabled = false;
      }
    });

    // ---- AI 对话 ----
    const aiSend = $('#aiSend');
    if (aiSend) aiSend.addEventListener('click', async function () {
      const aiInput = $('#aiInput');
      const text = ((aiInput && aiInput.value) || '').trim();
      if (!text) { W.showToast('请输入问题', true); return; }
      appendMsg('user', text);
      if (aiInput) aiInput.value = '';
      aiSend.disabled = true;
      try {
        const cur = await W.getSettings();
        const out = await W.chat([{ role: 'user', content: text }], {
          engine: cur.engine, sfKey: cur.sfKey, sfUrl: cur.sfUrl, sfModel: cur.sfModel,
          ollamaUrl: cur.ollamaUrl, ollamaModel: cur.ollamaModel
        });
        appendMsg('assistant', out);
      } catch (e) {
        appendMsg('assistant', '出错：' + e.message);
        W.showToast('AI 调用失败：' + e.message, true);
      } finally {
        aiSend.disabled = false;
      }
    });
    const aiInput = $('#aiInput');
    if (aiInput) aiInput.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        if (aiSend) aiSend.click();
      }
    });

    // ---- 设置面板：引擎 + ASR/TTS + 开关 ----
    if (engineSel) engineSel.addEventListener('change', async function () {
      try {
        const cur = await W.getSettings();
        cur.engine = ENGINE_MAP[engineSel.value] || 'sf';
        await W.setSettings(cur);
        W.showToast('引擎已切换为 ' + engineSel.value);
      } catch (e) { W.showToast('保存失败：' + e.message, true); }
    });
    const settingSelectors = {
      '#asrEngine': 'asrEngine', '#asrLang': 'asrLang', '#sfAsrModel': 'sfAsrModel',
      '#ttsVoice': 'ttsVoice', '#ttsRate': 'ttsRate', '#ttsPitch': 'ttsPitch'
    };
    Object.keys(settingSelectors).forEach(function (sel) {
      const el = $(sel);
      if (!el) return;
      el.addEventListener('change', async function () {
        try {
          const cur = await W.getSettings();
          const key = settingSelectors[sel];
          cur[key] = (sel === '#ttsRate' || sel === '#ttsPitch') ? parseFloat(el.value) : el.value;
          await W.setSettings(cur);
        } catch (e) { W.showToast('保存失败：' + e.message, true); }
      });
    });
    function wireToggle(id, key) {
      const el = $(id);
      if (!el) return;
      el.addEventListener('click', async function () {
        el.classList.toggle('on');
        try {
          const cur = await W.getSettings();
          cur[key] = el.classList.contains('on');
          await W.setSettings(cur);
        } catch (e) { W.showToast('保存失败：' + e.message, true); }
      });
    }
    wireToggle('#toggleAutoSendAsr', 'autoSendAsr');
    wireToggle('#toggleAutoSpeak', 'autoSpeak');

    // ---- 工具栏：打开/关闭面板 ----
    ['#openSettings', '#btnSettings'].forEach(function (id) {
      const b = $(id);
      if (b) b.addEventListener('click', function () { openPanel('settingsPanel'); });
    });
    const closeMap = {
      '#closeSettings': 'settingsPanel', '#closeHistory': 'historyPanel',
      '#closeStudy': 'studyPanel', '#closeExport': 'exportPanel'
    };
    Object.keys(closeMap).forEach(function (id) {
      const b = $(id);
      if (b) b.addEventListener('click', function () { closePanel(closeMap[id]); });
    });

    // ---- 历史记录面板 ----
    wireProjectBar('#historyProjectNew', '#historyProjectDel', '#historyProject', 'history', async function () {
      const list = $('#historyList');
      if (!list) return;
      list.innerHTML = '<div style="color:var(--muted);font-size:13px">加载中…</div>';
      try {
        const hist = await W.getHistoryByProject('history');
        if (!hist.length) { list.innerHTML = '<div style="color:var(--muted);font-size:13px">暂无记录</div>'; return; }
        list.innerHTML = '';
        hist.slice(-50).reverse().forEach(function (r) {
          const row = document.createElement('div');
          row.style.cssText = 'padding:12px 0;border-bottom:1px solid var(--border)';
          const meta = document.createElement('div');
          meta.style.cssText = 'font-size:11px;color:var(--faint);margin-bottom:4px';
          meta.textContent = (r.ts || '').slice(0, 16).replace('T', ' ') + ' · ' + (r.type === 'image' ? '图片' : r.type === 'pdf' ? 'PDF' : '文本');
          const o = document.createElement('div');
          o.style.cssText = 'font-size:13px;color:var(--text);white-space:pre-wrap;word-break:break-word';
          o.textContent = (r.original || r.ocr || '');
          const t = document.createElement('div');
          t.style.cssText = 'font-size:13px;color:var(--muted);white-space:pre-wrap;word-break:break-word';
          t.textContent = (r.translation || '');
          row.appendChild(meta); row.appendChild(o); row.appendChild(t);
          list.appendChild(row);
        });
      } catch (e) { list.innerHTML = '<div style="color:var(--err)">加载失败：' + esc(e.message) + '</div>'; }
    });
    const btnHistory = $('#btnHistory');
    if (btnHistory) btnHistory.addEventListener('click', function () { openPanel('historyPanel'); });

    // ---- 学习（生词本）面板 ----
    wireProjectBar('#studyProjectNew', '#studyProjectDel', '#studyProject', 'study', async function () {
      const statsEl = $('#studyStats');
      const queueEl = $('#studyQueue');
      if (statsEl) statsEl.innerHTML = '加载中…';
      if (queueEl) queueEl.innerHTML = '';
      try {
        const vocab = await W.getVocabByProject('study');
        const now = Date.now();
        const due = vocab.filter((v) => v.nextReview <= now).sort((a, b) => a.nextReview - b.nextReview).slice(0, 20);
        const avg = vocab.length ? Math.round(vocab.reduce((s, v) => s + v.mastery, 0) / vocab.length * 100) : 0;
        if (statsEl) statsEl.innerHTML = '<div style="font-size:13px;color:var(--text);margin-bottom:12px">生词本：' + vocab.length + ' 词 · 待复习：' + due.length + ' · 平均掌握度：' + avg + '%</div>';
        if (queueEl) {
          queueEl.innerHTML = '';
          if (!due.length) { queueEl.innerHTML = '<div style="color:var(--muted);font-size:13px">暂无待复习词</div>'; return; }
          due.forEach(function (it) {
            const row = document.createElement('div');
            row.style.cssText = 'padding:10px 0;border-bottom:1px solid var(--border)';
            row.innerHTML = '<div style="font-size:14px;color:var(--text)">' + esc(it.word) +
              ' <span style="color:var(--muted);font-size:12px">→ ' + esc(it.translation) + '</span></div>' +
              '<div style="font-size:11px;color:var(--faint)">掌握度：' + Math.round((it.mastery || 0) * 100) + '%</div>';
            queueEl.appendChild(row);
          });
        }
      } catch (e) { if (statsEl) statsEl.innerHTML = '<div style="color:var(--err)">加载失败：' + esc(e.message) + '</div>'; }
    });
    const btnStudy = $('#btnStudy');
    if (btnStudy) btnStudy.addEventListener('click', function () { openPanel('studyPanel'); });

    // ---- 导出面板 ----
    wireProjectBar('#exportProjectNew', '#exportProjectDel', '#exportProject', 'export', null);
    const btnExport = $('#btnExport');
    if (btnExport) btnExport.addEventListener('click', function () { openPanel('exportPanel'); });
    const exportFolder = $('#exportFolder');
    if (exportFolder) exportFolder.addEventListener('click', async function () {
      const status = $('#exportStatus');
      if (status) status.textContent = '导出中…';
      try {
        const cur = currentProject('export');
        const recs = cur === DEFAULT_PROJECT ? await W.getHistory() : await W.getHistoryByProject(cur);
        const n = await W.exportToFolder(recs);
        if (status) status.textContent = '已导出「' + cur + '」' + (n || recs.length) + ' 条到文件夹';
        W.showToast('导出完成');
      } catch (e) { if (status) status.textContent = '导出失败：' + e.message; W.showToast('导出失败：' + e.message, true); }
    });
    const exportZip = $('#exportZip');
    if (exportZip) exportZip.addEventListener('click', async function () {
      const status = $('#exportStatus');
      if (status) status.textContent = '打包中…';
      try {
        const cur = currentProject('export');
        const recs = cur === DEFAULT_PROJECT ? await W.getHistory() : await W.getHistoryByProject(cur);
        const n = await W.exportToZip(recs);
        if (status) status.textContent = '已导出「' + cur + '」' + (n || recs.length) + ' 条为 ZIP';
        W.showToast('导出完成');
      } catch (e) { if (status) status.textContent = '导出失败：' + e.message; W.showToast('导出失败：' + e.message, true); }
    });
  })();
})();

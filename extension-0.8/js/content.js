// content.js — 选区译文（三种形态，设置里可切）
//   inline      ：【原地替换】气泡只作触发，译文直接写回页面原位置；点译文可在「译文/原文」间切换
//   bar（默认） ：原位常驻翻译栏（原文 + 译文，可拖动，不自动消失）
//   bubble      ：临时气泡（滚动 / 点外部 / 取消选中即消失）
// 第一性：无选中不弹、不主动重排页面结构（inline 只替换被选中的那段，不整页重排）。
(function () {
  'use strict';
  let bubble = null;
  let MODE = 'bar';
  const DEBOUNCE = 220;
  const INLINE_CLASS = 'winocr-inline';
  let curSel = null;                 // 最近一次选中：{ range, text }

  // ---------------- 常量定义 ----------------
  const BUBBLE_MAX_WIDTH = 350;
  const BUBBLE_MIN_HEIGHT = 110;
  const BUBBLE_AUTO_CLOSE_MS = 1600;
  const BUBBLE_RESTORE_MS = 1200;
  const Z_INDEX_MAX = 2147483647;

  // 扩展上下文是否仍有效（扩展被重载/更新后，旧页面里的本脚本会失效）
  function ctxAlive() {
    try { return !!(typeof chrome !== 'undefined' && chrome.runtime && chrome.runtime.id); }
    catch (e) { return false; }
  }
  function stopEverything() {
    removeBubble();
    document.removeEventListener('mouseup', onSelect);
  }

  function loadMode() {
    if (!ctxAlive()) return;
    try {
      WINOCR.getSettings()
        .then((s) => { MODE = s.displayMode || 'bar'; })
        .catch(() => {});
    } catch (e) {}
  }
  loadMode();
  try {
    if (ctxAlive()) {
      chrome.storage.onChanged.addListener((ch, area) => {
        if (area === 'local' && ch && ch.winocr_settings_v1) loadMode();
      });
    }
  } catch (e) {}

  function removeBubble() {
    if (!bubble) return;
    try { if (bubble.__cleanup) bubble.__cleanup(); } catch (e) {}
    bubble.remove();
    bubble = null;
  }

  // 拖动（单一全局监听，避免每个气泡都挂监听造成泄漏）
  let drag = null;
  document.addEventListener('mousemove', (e) => {
    if (!drag) return;
    drag.el.style.left = (drag.ox + e.clientX - drag.sx) + 'px';
    drag.el.style.top = (drag.oy + e.clientY - drag.sy) + 'px';
  });
  document.addEventListener('mouseup', () => { drag = null; });

  // ---------------- 原地替换（inline） ----------------
  function inlineSpans() { return Array.prototype.slice.call(document.querySelectorAll('.' + INLINE_CLASS)); }

  function restoreOne(span) {
    try {
      if (span && span.__orig && span.parentNode) span.parentNode.replaceChild(span.__orig, span);
      else if (span) span.remove();
    } catch (e) {}
  }
  function restoreAllInline() { inlineSpans().forEach(restoreOne); }

  // 用译文替换 range 覆盖的原文（保留原 DOM fragment 以便还原）
  function applyInline(range, translation, originalText) {
    if (!range) return null;
    const frag = range.extractContents();          // 原文（保留结构）
    const span = document.createElement('span');
    span.className = INLINE_CLASS;
    span.textContent = translation;
    span.title = '点击显示原文';
    span.setAttribute('data-tr', '1');
    span.__orig = frag;
    span.__tr = translation;
    span.__src = originalText || frag.textContent || '';
    span.addEventListener('click', (e) => {
      e.stopPropagation();
      if (span.getAttribute('data-tr') === '1') {           // → 原文
        span.textContent = span.__src;
        span.setAttribute('data-tr', '0');
        span.title = '点击显示译文';
      } else {                                             // → 译文
        span.textContent = span.__tr;
        span.setAttribute('data-tr', '1');
        span.title = '点击显示原文';
      }
    });
    range.insertNode(span);
    return span;
  }

  // ---------------- 气泡 / 翻译栏 ----------------
  // ⚠️ 修「译文永远不显示 / 秒退」的根因（不是模型问题）：
  //    气泡内的 mousedown 必须 preventDefault（否则选区被清空、inline 取不到 range），
  //    代价是**点气泡按钮时页面选区依然在** → 这次 mouseup 冒泡到 document → onSelect() 又跑一遍 →
  //    220ms 后 showBubble() 开头那句 removeBubble() **把正在翻译的气泡整块拆掉**并新建一个空的，
  //    而 await 回来的译文最终写进一个已脱离文档（orphan）的节点 → 永远看不见，
  //    视觉上就是 "翻译中…" 闪一下（约 220ms）→ 秒退，然后什么都没有。
  //    故：① onSelect 忽略来自气泡内部的 mouseup；② 同一段文本复用同一气泡，绝不重建。
  function showBubble(text, rect, preset) {
    const hasPreset = !!(preset && (preset.translation || preset.error));
    if ((!text || !text.trim()) && !hasPreset) return;
    if (!hasPreset && bubble && bubble.__text === text) return;   // 同一段文本 → 复用，别拆掉重建
    removeBubble();
    const mode = MODE;                    // 固定住本气泡的形态：翻译途中改设置也不影响它
    const b = document.createElement('div');
    b.className = 'winocr-bubble winocr-mode-' + mode;
    b.__text = text || '';
    if (mode === 'inline') {
      b.innerHTML =
        '<div class="winocr-head">' +
        '<span class="winocr-grip">译文（原地替换）</span>' +
        '<span class="winocr-btns">' +
        '<button data-act="trans">译</button>' +
        '<button data-act="restore">还原</button>' +
        '<button data-act="close">×</button>' +
        '</span></div>' +
        '<div class="winocr-out"></div>';
    } else {
      b.innerHTML =
        '<div class="winocr-head">' +
        '<span class="winocr-grip">译文</span>' +
        '<span class="winocr-btns">' +
        '<button data-act="trans">译</button>' +
        '<button data-act="save">记录</button>' +
        '<button data-act="copy">复制</button>' +
        '<button data-act="panel">→栏</button>' +
        '<button data-act="close">×</button>' +
        '</span></div>' +
        '<div class="winocr-src"></div>' +
        '<div class="winocr-out"></div>';
    }
    document.body.appendChild(b);
    bubble = b;
    const x = Math.max(4, Math.min(rect.left, window.innerWidth - BUBBLE_MAX_WIDTH));
    let y = rect.bottom + window.scrollY + 6;
    // 别让常驻栏掉出屏幕底部（划到页面最后几行时很常见，译文区会被挤出视口）
    const maxY = window.scrollY + window.innerHeight - BUBBLE_MIN_HEIGHT;
    if (y > maxY) y = Math.max(window.scrollY + 6, maxY);
    b.style.left = x + 'px';
    b.style.top = y + 'px';
    const srcEl = b.querySelector('.winocr-src');
    if (srcEl) srcEl.textContent = text || '';
    if (hasPreset) {
      b.querySelector('.winocr-out').textContent = preset.error ? ('翻译失败：' + preset.error) : preset.translation;
    }

    // 点气泡不夺走页面选中（否则 mousedown 会清空 selection，取不到 range）
    b.addEventListener('mousedown', (e) => {
      if (e.target && (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA')) return;
      e.preventDefault();
    });

    // 拖动：仅常驻栏
    if (mode === 'bar') {
      const head = b.querySelector('.winocr-head');
      head.addEventListener('mousedown', (e) => {
        if (e.target && e.target.tagName === 'BUTTON') return;
        drag = {
          el: b, sx: e.clientX, sy: e.clientY,
          ox: parseInt(b.style.left, 10) || 0, oy: parseInt(b.style.top, 10) || 0
        };
      });
    }

    // 翻译：抽成函数（① 常驻栏开栏即自动翻译；② 供「译」按钮手动重译）
    async function runTranslate() {
      const out = b.querySelector('.winocr-out');
      if (!out) return;
      out.textContent = '翻译中…';
      try {
        const s = await WINOCR.getSettings();
        const t = await WINOCR.translate(text, {
          engine: s.engine, sfKey: s.sfKey, sfUrl: s.sfUrl, sfModel: s.sfModel,
          mymemoryEmail: s.mymemoryEmail,
          ollamaUrl: s.ollamaUrl, ollamaModel: s.ollamaModel,
          srcLang: s.srcLang, tgtLang: s.tgtLang
        });
        if (!b.isConnected) return;      // 气泡已被关掉/替换：别往孤儿节点写（那才是"看不见"的原因）
        if (mode === 'inline') {
          const span = applyInline(curSel && curSel.range, t, text);
          out.textContent = span ? '已原地替换 · 点译文可切回原文' : ('已翻译（未取到选区，仅显示）：' + t);
          try { await WINOCR.addRecord({ type: 'text', source: location.href, original: text, translation: t }); } catch (e) {}
          setTimeout(() => { if (bubble === b) removeBubble(); }, BUBBLE_AUTO_CLOSE_MS);
        } else {
          out.textContent = t;
        }
      } catch (e) {
        if (b.isConnected) out.textContent = '翻译失败：' + ((e && e.message) || e);
      }
    }
    b.querySelector('[data-act="trans"]').onclick = runTranslate;

    // 常驻翻译栏：开栏就自动翻译。
    // 「选中后只给原文、要点一下才有译文」本身就是这个形态的设计缺陷——它叫翻译栏，不该是照抄栏。
    if (mode === 'bar' && !hasPreset && text && text.trim()) runTranslate();

    const btnRestore = b.querySelector('[data-act="restore"]');
    if (btnRestore) btnRestore.onclick = () => { restoreAllInline(); flash(b, '已还原原文'); };

    const btnSave = b.querySelector('[data-act="save"]');
    if (btnSave) btnSave.onclick = async () => {
      const out = b.querySelector('.winocr-out').textContent || '';
      await WINOCR.addRecord({ type: 'text', source: location.href, original: text, translation: out });
      flash(b, '已记录');
    };
    const btnCopy = b.querySelector('[data-act="copy"]');
    if (btnCopy) btnCopy.onclick = () => {
      const out = b.querySelector('.winocr-out').textContent || '';
      navigator.clipboard.writeText(out || text);
      flash(b, '已复制');
    };
    const btnPanel = b.querySelector('[data-act="panel"]');
    if (btnPanel) btnPanel.onclick = () => {
      const out = b.querySelector('.winocr-out').textContent || '';
      const hasTr = out && out !== '翻译中…' && out.indexOf('翻译失败') !== 0;
      const swallow = () => { void chrome.runtime.lastError; };
      try {
        chrome.runtime.sendMessage({ type: 'panel-input', text: text }, swallow);
        if (hasTr) chrome.runtime.sendMessage({ type: 'panel-result', original: text, translation: out }, swallow);
        flash(b, '已送面板');
      } catch (e) { flash(b, '面板未打开'); }
    };
    b.querySelector('[data-act="close"]').onclick = removeBubble;

    // 非「常驻栏」形态：沿用旧的「不干扰阅读」自动消失行为
    if (mode !== 'bar') {
      const onScroll = () => removeBubble();
      const onDown = (e) => { if (bubble && bubble.contains(e.target)) return; removeBubble(); };
      const onSel = () => { const s = window.getSelection(); if (!s || !s.toString().trim()) removeBubble(); };
      window.addEventListener('scroll', onScroll, { passive: true });
      document.addEventListener('mousedown', onDown, true);
      document.addEventListener('selectionchange', onSel);
      b.__cleanup = () => {
        window.removeEventListener('scroll', onScroll);
        document.removeEventListener('mousedown', onDown, true);
        document.removeEventListener('selectionchange', onSel);
      };
    }
  }

  function flash(b, msg) {
    const o = b.querySelector('.winocr-out');
    if (!o) return;
    const old = o.textContent;
    o.textContent = msg;
    setTimeout(() => { if (o.textContent === msg) o.textContent = old; }, BUBBLE_RESTORE_MS);
  }

  function onSelect(e) {
    // 关键：点/拖气泡内部（译 · 记录 · 复制 · →栏 · × · 标题栏）时，mousedown 已被 preventDefault，
    // 页面选区并未被清空，于是这次 mouseup 会跑到这里。若在此重建气泡，正在进行的翻译会被打断
    // （原文写进孤儿节点）→ 这就是"译文永远不显示 / 秒退"的根因；拖动也会被重置。故直接忽略。
    if (bubble && e && e.target && bubble.contains(e.target)) return;
    setTimeout(() => {
      if (!ctxAlive()) { stopEverything(); return; }
      const sel = window.getSelection();
      const text = sel ? sel.toString().trim() : '';
      if (!text) return;                 // 无选中：不弹，不干扰阅读
      let rect = { left: 80, bottom: 80 };
      let range = null;
      try {
        range = sel.getRangeAt(0).cloneRange();
        rect = sel.getRangeAt(0).getBoundingClientRect();
      } catch (e) { console.warn('[WinOCR] 选区获取失败:', e); }
      curSel = { range: range, text: text };      // 存起来：点「译」时选区可能已被清空
      showBubble(text, rect);
    }, DEBOUNCE);
  }

  document.addEventListener('mouseup', onSelect);

  // ---------------- 实时悬停翻译（轻量 Tooltip） ----------------
  // 在划词气泡的基础上，新增「鼠标悬停 + 已选文本」时的快速预览 Tooltip。
  // 与 mouseup 气泡的区别：不抢占焦点、无按钮、鼠标移开即消失，适合"扫一眼译文"。
  let hoverTip = null;
  let hoverTimer = null;
  let hoverLastText = '';
  const HOVER_DELAY_MS = 350;        // 悬停多久后弹出
  const HOVER_HIDE_DELAY_MS = 200;   // 鼠标移开后多久消失
  let hoverHideTimer = null;

  function removeHoverTip() {
    if (hoverTip) { try { hoverTip.remove(); } catch (e) {} hoverTip = null; }
  }
  function clearHoverTimers() {
    if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = null; }
    if (hoverHideTimer) { clearTimeout(hoverHideTimer); hoverHideTimer = null; }
  }

  function showHoverTip(text, x, y) {
    removeHoverTip();
    const el = document.createElement('div');
    el.className = 'winocr-hover-tip';
    el.textContent = '翻译中…';
    document.body.appendChild(el);
    hoverTip = el;
    // 定位：鼠标右下方，避开屏幕边缘
    const tipW = 280, tipH = 40;
    let px = x + 14, py = y + 18;
    if (px + tipW > window.innerWidth + window.scrollX) px = x - tipW - 14;
    if (py + tipH > window.innerHeight + window.scrollY) py = y - tipH - 10;
    el.style.left = Math.max(4, px) + 'px';
    el.style.top = Math.max(4, py) + 'px';

    (async () => {
      try {
        const s = await WINOCR.getSettings();
        const t = await WINOCR.translate(text, {
          engine: s.engine, sfKey: s.sfKey, sfUrl: s.sfUrl, sfModel: s.sfModel,
          mymemoryEmail: s.mymemoryEmail,
          ollamaUrl: s.ollamaUrl, ollamaModel: s.ollamaModel,
          srcLang: s.srcLang, tgtLang: s.tgtLang
        });
        if (hoverTip === el) el.textContent = t;
      } catch (e) {
        if (hoverTip === el) el.textContent = '翻译失败：' + ((e && e.message) || e);
      }
    })();
  }

  function onMouseMove(e) {
    // 只在"已有选中文本"时触发，避免无意义的请求
    const sel = window.getSelection();
    const text = sel ? sel.toString().trim() : '';
    if (!text || text.length < 2) { clearHoverTimers(); removeHoverTip(); return; }
    // 鼠标在气泡/Tooltip 内部时不处理（让用户能点按钮）
    if (bubble && bubble.contains(e.target)) return;
    if (hoverTip && hoverTip.contains(e.target)) {
      // 在自己的 tooltip 上 → 取消隐藏计时
      if (hoverHideTimer) { clearTimeout(hoverHideTimer); hoverHideTimer = null; }
      return;
    }
    clearHoverTimers();
    hoverLastText = text;
    const cx = e.clientX, cy = e.clientY;
    hoverTimer = setTimeout(() => {
      // 再次确认选区还在（用户可能已经取消选择）
      const sel2 = window.getSelection();
      const t2 = sel2 ? sel2.toString().trim() : '';
      if (t2 === text && text) showHoverTip(text, cx, cy);
    }, HOVER_DELAY_MS);
  }

  function onMouseLeave() {
    clearHoverTimers();
    hoverHideTimer = setTimeout(removeHoverTip, HOVER_HIDE_DELAY_MS);
  }

  document.addEventListener('mousemove', onMouseMove);
  document.addEventListener('mouseleave', onMouseLeave);

  // 供侧栏 / 后台读取选中文本；并接收「就地显示结果」（右键菜单 / Alt+Shift+Z）
  try {
    chrome.runtime.onMessage.addListener((m, sender, sendResponse) => {
      if (!m) return;
      if (m.type === 'winocr.getSelection') {
        const sel = window.getSelection();
        sendResponse({ text: sel ? String(sel) : '' });
      } else if (m.type === 'winocr.showBubble') {
        let rect = { left: 120, bottom: 120 };
        try {
          const sel = window.getSelection();
          if (sel && sel.rangeCount) {
            rect = sel.getRangeAt(0).getBoundingClientRect();
            curSel = { range: sel.getRangeAt(0).cloneRange(), text: m.original || '' };
          }
        } catch (e) {}
        showBubble(m.original || '', rect, { translation: m.translation, error: m.error });
        sendResponse({ ok: true });
      } else if (m.type === 'winocr.restoreInline') {
        restoreAllInline();
        sendResponse({ ok: true });
      } else if (m.type === 'winocr.regionSelect') {
        // 浏览器内区域截图：展示可见区域截图，让用户拖拽框选，然后裁剪回传
        startRegionSelect(m.dataUrl);
        sendResponse({ ok: true });
      } else if (m.type === 'winocr.fullpage') {
        // 整页翻译
        fpRun();
        sendResponse({ ok: true, started: true });
        return true;   // fpRun 是异步的，但我们立即响应启动
      } else if (m.type === 'winocr.fullpage.restore') {
        fpRestore();
        sendResponse({ ok: true });
      } else if (m.type === 'winocr.subtitle.start') {
        subStart();
        sendResponse({ ok: subActive });
      } else if (m.type === 'winocr.subtitle.stop') {
        subStop();
        sendResponse({ ok: true });
      }
    });
  } catch (e) {}

  // ---------------- 浏览器内区域截图 ----------------
  // 用 chrome.tabs.captureVisibleTab 拿到的整页可见截图做底图，用户拖拽画框，
  // 松开后用 canvas 裁剪出选区并交给 sidepanel 做 OCR。
  function startRegionSelect(dataUrl) {
    if (!dataUrl) return;
    removeBubble(); removeHoverTip();
    const overlay = document.createElement('div');
    overlay.className = 'winocr-region-overlay';
    overlay.innerHTML =
      '<img class="winocr-region-bg" />' +
      '<div class="winocr-region-dim"></div>' +
      '<div class="winocr-region-box"></div>' +
      '<div class="winocr-region-hint">拖拽选择 OCR 区域 · 按 Esc 取消</div>';
    document.body.appendChild(overlay);
    const bg = overlay.querySelector('.winocr-region-bg');
    const dim = overlay.querySelector('.winocr-region-dim');
    const box = overlay.querySelector('.winocr-region-box');
    bg.src = dataUrl;

    let sx = 0, sy = 0, drawing = false;

    function onDown(e) {
      drawing = true;
      sx = e.clientX; sy = e.clientY;
      box.style.left = sx + 'px'; box.style.top = sy + 'px';
      box.style.width = '0px'; box.style.height = '0px';
      box.style.display = 'block';
    }
    function onMove(e) {
      if (!drawing) return;
      const x = Math.min(sx, e.clientX), y = Math.min(sy, e.clientY);
      const w = Math.abs(e.clientX - sx), h = Math.abs(e.clientY - sy);
      box.style.left = x + 'px'; box.style.top = y + 'px';
      box.style.width = w + 'px'; box.style.height = h + 'px';
    }
    function onUp(e) {
      if (!drawing) return;
      drawing = false;
      const x = Math.min(sx, e.clientX), y = Math.min(sy, e.clientY);
      const w = Math.abs(e.clientX - sx), h = Math.abs(e.clientY - sy);
      if (w < 5 || h < 5) { cleanup(); return; }
      // 用 canvas 裁剪
      const cv = document.createElement('canvas');
      cv.width = w; cv.height = h;
      const ctx = cv.getContext('2d');
      const img = new Image();
      img.onload = () => {
        try {
          ctx.drawImage(img, x, y, w, h, 0, 0, w, h);
          const cropped = cv.toDataURL('image/png');
          cleanup();
          // 回传 sidepanel 做 OCR
          try {
            chrome.runtime.sendMessage({ type: 'winocr.regionCrop', dataUrl: cropped }, () => { void chrome.runtime.lastError; });
          } catch (e) {}
        } catch (e) { cleanup(); }
      };
      img.src = dataUrl;
    }
    function onKey(e) {
      if (e.key === 'Escape') cleanup();
    }
    function cleanup() {
      try { overlay.remove(); } catch (e) {}
      document.removeEventListener('keydown', onKey);
    }
    overlay.addEventListener('mousedown', onDown);
    overlay.addEventListener('mousemove', onMove);
    overlay.addEventListener('mouseup', onUp);
    document.addEventListener('keydown', onKey);
  }

  // ---------------- 整页翻译 ----------------
  // 智能识别正文区域 → 遍历文本节点 → 批量翻译 → 双语对照渲染
  // 双语模式：译文插入原文下方（淡灰底），可一键还原
  // 仅译文模式：原文替换为译文（可点回原文）
  const FP_CLASS = 'winocr-fp';          // 已翻译的文本节点标记
  const FP_ORIG_ATTR = 'data-winocr-orig';
  let fpActive = false;                  // 当前是否处于整页翻译状态
  let fpBusy = false;

  // 估算元素的"正文得分"：文本量 × (1 - 链接密度)
  function fpScore(el) {
    const text = (el.innerText || '').trim();
    if (!text || text.length < 30) return 0;
    const links = el.querySelectorAll('a').length;
    const linkDensity = links / Math.max(1, text.length / 20);
    return text.length * Math.max(0, 1 - linkDensity * 0.5);
  }

  function fpFindMain() {
    // 优先语义标签
    const sem = document.querySelector('article, main, [role="main"], .post-content, .article-content, #content');
    if (sem) return sem;
    // 否则按得分选
    const candidates = Array.from(document.querySelectorAll('div, section, article'))
      .filter((e) => e.offsetParent !== null)
      .map((e) => ({ el: e, score: fpScore(e) }))
      .sort((a, b) => b.score - a.score);
    return candidates[0] && candidates[0].score > 200 ? candidates[0].el : document.body;
  }

  // 收集正文区域内的可翻译文本节点（跳过 script/style/pre/code/已标记）
  function fpCollectTextNodes(root) {
    const skip = new Set(['SCRIPT', 'STYLE', 'PRE', 'CODE', 'NOSCRIPT', 'TEXTAREA', 'INPUT', 'BUTTON']);
    const nodes = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode(n) {
        const p = n.parentElement;
        if (!p) return NodeFilter.FILTER_REJECT;
        if (skip.has(p.tagName)) return NodeFilter.FILTER_REJECT;
        if (p.classList && p.classList.contains(FP_CLASS)) return NodeFilter.FILTER_REJECT;
        const t = n.textContent.trim();
        if (!t || t.length < 2) return NodeFilter.FILTER_REJECT;
        // 跳过纯数字/符号
        if (/^[\d\s\W]+$/.test(t)) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      }
    });
    let n;
    while ((n = walker.nextNode())) nodes.push(n);
    return nodes;
  }

  // 单个文本节点的翻译与渲染
  async function fpTranslateNode(node) {
    const orig = node.textContent.trim();
    try {
      const s = await WINOCR.getSettings();
      const tr = await WINOCR.translate(orig, {
        engine: s.engine, sfKey: s.sfKey, sfUrl: s.sfUrl, sfModel: s.sfModel,
        mymemoryEmail: s.mymemoryEmail,
        ollamaUrl: s.ollamaUrl, ollamaModel: s.ollamaModel,
        srcLang: s.srcLang, tgtLang: s.tgtLang
      });
      if (!tr || tr === orig) return;
      const span = document.createElement('span');
      span.className = FP_CLASS;
      span.setAttribute(FP_ORIG_ATTR, orig);
      span.textContent = tr;
      span.title = orig;
      // 用 span 替换原文本节点
      node.parentNode.replaceChild(span, node);
    } catch (e) { /* 单个失败不影响其他 */ }
  }

  async function fpRun(mode) {
    if (fpBusy) return;
    fpBusy = true;
    // 还原已翻译的（避免重复套娃）
    fpRestore();
    const root = fpFindMain();
    const nodes = fpCollectTextNodes(root);
    if (!nodes.length) { fpBusy = false; return; }

    // 并发控制：同时最多 5 个请求
    const CONCURRENCY = 5;
    let idx = 0;
    const workers = [];
    for (let i = 0; i < CONCURRENCY; i++) {
      workers.push((async () => {
        while (idx < nodes.length) {
          const cur = idx++;
          await fpTranslateNode(nodes[cur]);
        }
      })());
    }
    await Promise.all(workers);
    fpActive = true;
    fpBusy = false;
    // 通知后台进度完成
    try { chrome.runtime.sendMessage({ type: 'winocr.fpDone', count: nodes.length }, () => { void chrome.runtime.lastError; }); } catch (e) {}
  }

  function fpRestore() {
    const spans = document.querySelectorAll('.' + FP_CLASS);
    spans.forEach((sp) => {
      const orig = sp.getAttribute(FP_ORIG_ATTR);
      if (orig != null) {
        const tn = document.createTextNode(orig);
        sp.parentNode.replaceChild(tn, sp);
      } else {
        sp.remove();
      }
    });
    fpActive = false;
  }

  // ---------------- 视频双语字幕（YouTube / Bilibili / Netflix） ----------------
  // 监听字幕 DOM（YouTube: .caption-window .ytp-caption-segment；
  // Bilibili: .bilibili-player-video-subtitle .subtitle-item；
  // Netflix: video.textTracks 轨道），实时翻译并在下方叠加译文。
  // 不碰播放器 API，只做 DOM 观察 + 翻译叠加，兼容性最好。
  const SUB_CLASS = 'winocr-sub-trans';
  let subObserver = null;
  let subActive = false;
  let subLastText = '';
  let subTransTimer = null;
  let subPlatform = '';          // 'youtube' | 'bilibili' | 'netflix'
  const SUB_DEBOUNCE_MS = 400;

  // 各平台字幕文本选择器
  const SUB_SELECTORS = {
    youtube: '.caption-window .ytp-caption-segment, .ytp-caption-segment',
    bilibili: '.bilibili-player-video-subtitle .subtitle-item, .bilibili-player-video-subtitle span, .subtitle-item span',
    netflix: '.player-timedtext-text-container, .player-timedtext span, video::cue'
  };

  // 各平台字幕容器选择器（用于定位译文叠加位置）
  const SUB_CONTAINER_SELECTORS = {
    youtube: '.caption-window',
    bilibili: '.bilibili-player-video-subtitle, .subtitle-item',
    netflix: '.player-timedtext, .player-timedtext-text-container'
  };

  function subDetectPlatform() {
    if (/youtube\.com|youtu\.be/.test(location.hostname)) return 'youtube';
    if (/bilibili\.com/.test(location.hostname)) return 'bilibili';
    if (/netflix\.com/.test(location.hostname)) return 'netflix';
    return '';
  }

  function subGetCurrentText() {
    const sel = SUB_SELECTORS[subPlatform] || SUB_SELECTORS.youtube;
    const segs = document.querySelectorAll(sel);
    return Array.from(segs).map((s) => s.textContent).join(' ').trim();
  }

  function subGetOrCreateOverlay() {
    let ov = document.querySelector('.' + SUB_CLASS);
    if (!ov) {
      ov = document.createElement('div');
      ov.className = SUB_CLASS;
      document.body.appendChild(ov);
    }
    return ov;
  }

  function subPositionOverlay() {
    const ov = subGetOrCreateOverlay();
    const capSel = SUB_CONTAINER_SELECTORS[subPlatform] || SUB_CONTAINER_SELECTORS.youtube;
    const cap = document.querySelector(capSel);
    if (!cap) { ov.style.display = 'none'; return; }
    const r = cap.getBoundingClientRect();
    ov.style.left = r.left + 'px';
    ov.style.top = (r.bottom + window.scrollY + 4) + 'px';
    ov.style.width = r.width + 'px';
    ov.style.display = 'block';
  }

  async function subTranslate(text) {
    if (!text || text === subLastText) return;
    subLastText = text;
    const ov = subGetOrCreateOverlay();
    ov.textContent = '翻译中…';
    try {
      const s = await WINOCR.getSettings();
      const t = await WINOCR.translate(text, {
        engine: s.engine, sfKey: s.sfKey, sfUrl: s.sfUrl, sfModel: s.sfModel,
        mymemoryEmail: s.mymemoryEmail,
        ollamaUrl: s.ollamaUrl, ollamaModel: s.ollamaModel,
        srcLang: s.srcLang, tgtLang: s.tgtLang
      });
      // 字幕可能已经切换，只在文本仍匹配时显示
      if (subLastText === text) ov.textContent = t;
    } catch (e) {
      if (subLastText === text) ov.textContent = '';
    }
  }

  function subOnMutations() {
    if (!subActive) return;
    const text = subGetCurrentText();
    subPositionOverlay();
    if (!text) {
      subGetOrCreateOverlay().textContent = '';
      subLastText = '';
      return;
    }
    if (subTransTimer) clearTimeout(subTransTimer);
    subTransTimer = setTimeout(() => subTranslate(text), SUB_DEBOUNCE_MS);
  }

  function subStart() {
    if (subActive) return;
    subPlatform = subDetectPlatform();
    if (!subPlatform) return;
    subActive = true;
    subLastText = '';
    const capSel = SUB_CONTAINER_SELECTORS[subPlatform] || SUB_CONTAINER_SELECTORS.youtube;
    const target = document.querySelector(capSel) || document.body;
    subObserver = new MutationObserver(subOnMutations);
    subObserver.observe(target, { childList: true, subtree: true, characterData: true });
    subOnMutations();
  }

  function subStop() {
    subActive = false;
    if (subObserver) { subObserver.disconnect(); subObserver = null; }
    if (subTransTimer) { clearTimeout(subTransTimer); subTransTimer = null; }
    const ov = document.querySelector('.' + SUB_CLASS);
    if (ov) ov.remove();
    subLastText = '';
  }

  // 注入样式（仅一次）
  if (!document.getElementById('winocr-style')) {
    const st = document.createElement('style');
    st.id = 'winocr-style';
    st.textContent =
      '.winocr-bubble{position:absolute;z-index:' + Z_INDEX_MAX + ';background:#fff;border:0.5px solid #d9d9d9;border-radius:10px;' +
      'box-shadow:0 4px 16px rgba(0,0,0,.16);font:13px/1.55 system-ui,sans-serif;width:330px;padding:0;color:#222;overflow:hidden}' +
      '.winocr-head{display:flex;align-items:center;justify-content:space-between;gap:6px;padding:5px 8px;' +
      'background:#f7f9fc;border-bottom:0.5px solid #eef1f5;user-select:none}' +
      '.winocr-mode-bar .winocr-head{cursor:move}' +
      '.winocr-grip{font-size:12px;font-weight:600;color:#1677ff}' +
      '.winocr-mode-bar .winocr-grip::before{content:"⠿ ";color:#b8c4d4}' +
      '.winocr-btns{display:inline-flex}' +
      '.winocr-btns button{font:12px system-ui;border:0.5px solid #d9d9d9;background:#fff;border-radius:6px;' +
      'margin-left:4px;padding:1px 7px;cursor:pointer;color:#555}' +
      '.winocr-btns button:hover{background:#e6f1fb;border-color:#1677ff;color:#1677ff}' +
      '.winocr-src{padding:7px 9px 0;color:#8a8a8a;white-space:pre-wrap;word-break:break-word;max-height:18vh;overflow:auto}' +
      '.winocr-out{padding:7px 9px 9px;white-space:pre-wrap;word-break:break-word;max-height:46vh;overflow:auto;' +
      'border-top:0.5px solid #f0f2f5;margin-top:7px}' +
      // 原地替换后的译文样式：淡蓝底 + 虚线，一眼可辨且不打断阅读
      '.' + INLINE_CLASS + '{background:rgba(22,119,255,.10);border-bottom:1px dotted rgba(22,119,255,.65);' +
      'border-radius:2px;cursor:pointer;transition:background .15s ease-out}' +
      '.' + INLINE_CLASS + ':hover{background:rgba(22,119,255,.18)}' +
      '.winocr-src,.winocr-out{scrollbar-width:thin}' +
      '.winocr-src::-webkit-scrollbar,.winocr-out::-webkit-scrollbar{width:10px;height:10px}' +
      '.winocr-src::-webkit-scrollbar-track,.winocr-out::-webkit-scrollbar-track{background:transparent}' +
      '.winocr-src::-webkit-scrollbar-thumb,.winocr-out::-webkit-scrollbar-thumb{' +
      'background:rgba(18,24,31,.20);border-radius:100px;border:3px solid transparent;background-clip:content-box}' +
      // 悬停快速预览 Tooltip：深色、紧凑、无按钮，鼠标移开即消失
      '.winocr-hover-tip{position:fixed;z-index:' + Z_INDEX_MAX + ';max-width:280px;padding:6px 10px;' +
      'background:rgba(30,33,40,.96);color:#e8edf3;border-radius:8px;box-shadow:0 4px 16px rgba(0,0,0,.25);' +
      'font:13px/1.55 system-ui,sans-serif;word-break:break-word;white-space:pre-wrap;' +
      'pointer-events:none;animation:winocr-fadein .12s ease-out}' +
      '@keyframes winocr-fadein{from{opacity:0;transform:translateY(2px)}to{opacity:1;transform:none}}' +
      // 浏览器内区域截图遮罩
      '.winocr-region-overlay{position:fixed;inset:0;z-index:' + Z_INDEX_MAX + ';cursor:crosshair}' +
      '.winocr-region-bg{position:absolute;inset:0;width:100%;height:100%;object-fit:cover;display:block}' +
      '.winocr-region-dim{position:absolute;inset:0;background:rgba(0,0,0,.35);pointer-events:none}' +
      '.winocr-region-box{position:absolute;border:2px dashed #1677ff;background:rgba(22,119,255,.08);' +
      'box-shadow:0 0 0 9999px rgba(0,0,0,.15);display:none;pointer-events:none}' +
      '.winocr-region-hint{position:absolute;top:12px;left:50%;transform:translateX(-50%);' +
      'background:rgba(0,0,0,.7);color:#fff;padding:6px 14px;border-radius:6px;font:13px system-ui;pointer-events:none}' +
      // 整页翻译：译文以淡色高亮，悬停显示原文（title 属性）
      '.' + FP_CLASS + '{background:rgba(22,119,255,.06);border-bottom:1px dotted rgba(22,119,255,.4);' +
      'border-radius:2px;transition:background .15s ease-out}' +
      '.' + FP_CLASS + ':hover{background:rgba(22,119,255,.15)}' +
      // 视频双语字幕译文层：在字幕下方，半透明深色背景，跟随字幕位置
      '.winocr-sub-trans{position:absolute;z-index:' + Z_INDEX_MAX + ';background:rgba(0,0,0,.75);' +
      'color:#fff;border-radius:4px;padding:3px 8px;font:14px/1.5 system-ui,sans-serif;' +
      'text-align:center;pointer-events:none;text-shadow:0 1px 2px rgba(0,0,0,.8)}';
    document.documentElement.appendChild(st);
  }
})();

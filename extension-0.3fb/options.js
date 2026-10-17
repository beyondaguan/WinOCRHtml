// options.js — 设置页
(async function () {
  const $ = (s) => document.querySelector(s);
  const s = await WINOCR.getSettings();

  // ---------------- 常量定义 ----------------
  const TEST_TIMEOUT_MS = 45000;
  const OCR_TEST_TIMEOUT_MS = 90000;
  const CANVAS_WIDTH = 760;
  const CANVAS_HEIGHT = 130;
  const CANVAS_FONT_SIZE = 27;

  // ---- 模型下拉：只用免费模型（杜绝手打错名 → 20012 Model does not exist 那类问题） ----
  function fillSelect(el, models, current) {
    el.innerHTML = '';
    models.forEach((m) => {
      const o = document.createElement('option');
      o.value = m.id;
      o.textContent = m.note ? (m.id + ' — ' + m.note) : m.id;
      el.appendChild(o);
    });
    // 已存的值不在清单里（旧配置 / 自定义）→ 追加一项，避免保存时被悄悄改写
    if (current && !models.some((m) => m.id === current)) {
      const o = document.createElement('option');
      o.value = current;
      o.textContent = current + ' — 自定义（不在免费清单内）';
      el.appendChild(o);
    }
    el.value = current || (models[0] && models[0].id) || '';
  }
  fillSelect($('#sfModel'), WINOCR.FREE_TEXT_MODELS || [], s.sfModel);
  fillSelect($('#sfOcrModel'), WINOCR.FREE_OCR_MODELS || [], s.sfOcrModel);
  $('#ocrEngine').value = s.ocrEngine || 'local';
  $('#localOcrTier').value = s.localOcrTier || 'tiny';

  $('#engine').value = s.engine;
  $('#translateEngine').value = s.translateEngine || 'sf';
  $('#sfKey').value = s.sfKey;
  $('#sfUrl').value = s.sfUrl;
  $('#srcLang').value = s.srcLang;
  $('#tgtLang').value = s.tgtLang;
  $('#displayMode').value = s.displayMode || 'inline';
  $('#nativeHost').checked = !!s.nativeHost;
  $('#mymemoryEmail').value = s.mymemoryEmail || '';
  $('#ollamaUrl').value = s.ollamaUrl || 'http://localhost:11434';
  $('#ollamaModel').value = s.ollamaModel || '';
  // 若已有保存的 ollama 模型，确保它在下拉里
  if (s.ollamaModel && !Array.prototype.some.call($('#ollamaModel').options, (o) => o.value === s.ollamaModel)) {
    const o = document.createElement('option');
    o.value = s.ollamaModel; o.textContent = s.ollamaModel + '（已保存）';
    $('#ollamaModel').appendChild(o);
  }
  $('#hotkey').value = s.hotkey || 'ctrl+shift+m';
  $('#quitHotkey').value = s.quitHotkey || 'ctrl+alt+q';
  $('#t_folder').checked = !!s.exportTargets.folder;
  $('#t_zip').checked = !!s.exportTargets.zip;
  $('#t_obsidian').checked = !!s.exportTargets.obsidian;
  $('#obsidianUrl').value = s.obsidianUrl;
  $('#obsidianKey').value = s.obsidianKey;
  $('#t_lexiang').checked = !!s.exportTargets.lexiang;
  $('#lexiangEndpoint').value = s.lexiangEndpoint;
  $('#lexiangToken').value = s.lexiangToken;

  // ---- 本地配置文件路径显示 ----
  // 独立模式 / 桌面热键截图读的是 native_host\winocr_config.json，和本页保存的
  // chrome.storage 是两份互不同步的文件。用户最需要知道的就是这个绝对路径，
  // 所以直接显示在面板上，而不是让他去翻 README 找。
  function showCfgPath(p) {
    const el = $('#cfgPath');
    if (el && p) el.textContent = p;
  }
  function pullCfgPath() {
    // 打开选项页就补一次：已桥接时立刻能显示，不必先点「检测宿主连接」
    chrome.runtime.sendMessage({ type: 'native.status' }, (st) => {
      void chrome.runtime.lastError;
      if (st && st.host && st.host.configPath) showCfgPath(st.host.configPath);
    });
  }

  // ---- 表单 ↔ 设置对象 ----
  // 保存与「检测宿主连接」共用同一条读取路径：两处各写一遍最容易漂移，
  // 而漂移的代价是「页面上看到的 key」和「真正落盘的 key」并非同一个。
  function formSettings() {
    return Object.assign({}, s, {
      engine: $('#engine').value,
      translateEngine: $('#translateEngine').value,
      sfKey: $('#sfKey').value,
      sfUrl: $('#sfUrl').value,
      sfModel: $('#sfModel').value,
      sfOcrModel: $('#sfOcrModel').value,
      ocrEngine: $('#ocrEngine').value,
      localOcrTier: $('#localOcrTier').value,
      srcLang: $('#srcLang').value,
      tgtLang: $('#tgtLang').value,
      displayMode: $('#displayMode').value,
      nativeHost: $('#nativeHost').checked,
      hotkey: $('#hotkey').value.trim(),
      quitHotkey: $('#quitHotkey').value.trim(),
      mymemoryEmail: $('#mymemoryEmail').value.trim(),
      ollamaUrl: $('#ollamaUrl').value.trim() || 'http://localhost:11434',
      ollamaModel: $('#ollamaModel').value,
      obsidianUrl: $('#obsidianUrl').value,
      obsidianKey: $('#obsidianKey').value,
      lexiangEndpoint: $('#lexiangEndpoint').value,
      lexiangToken: $('#lexiangToken').value,
      exportTargets: {
        folder: $('#t_folder').checked,
        zip: $('#t_zip').checked,
        obsidian: $('#t_obsidian').checked,
        lexiang: $('#t_lexiang').checked
      }
    });
  }

  async function saveForm() {
    const ns = formSettings();
    await WINOCR.setSettings(ns);
    // 同步给原生宿主（外部截图 / 独立模式热键）
    try { chrome.runtime.sendMessage({ type: 'syncSettings', settings: ns }); } catch (e) {}
    return ns;
  }

  // ---- 未保存改动提示 ----
  // sfKey 这类字段最容易「填了却没点保存」：没落盘 → 宿主也拿不到 →
  // 截图时报「未配置 SF key」，而用户在页面上明明填过。
  const FORM_FIELDS = ['#engine', '#translateEngine', '#sfKey', '#sfUrl', '#sfModel', '#sfOcrModel', '#ocrEngine',
    '#localOcrTier', '#srcLang', '#tgtLang', '#displayMode', '#hotkey', '#quitHotkey',
    '#mymemoryEmail', '#ollamaUrl', '#ollamaModel', '#obsidianUrl', '#obsidianKey', '#lexiangEndpoint',
    '#lexiangToken', '#nativeHost', '#t_folder', '#t_zip', '#t_obsidian', '#t_lexiang'];
  let formDirty = false;
  function markDirty() {
    if (formDirty) return;
    formDirty = true;
    const m = $('#msg');
    if (m) { m.textContent = '有未保存的改动 —— 点「保存」后才会生效'; m.classList.add('err'); }
  }
  function markClean() {
    formDirty = false;
    const m = $('#msg');
    if (m) m.classList.remove('err');
  }
  FORM_FIELDS.forEach((sel) => {
    const el = $(sel);
    if (!el) return;
    el.addEventListener('input', markDirty);
    el.addEventListener('change', markDirty);
  });

  $('#save').onclick = async () => {
    await saveForm();
    markClean();
    const m = $('#msg');
    m.textContent = '已保存';
    setTimeout(() => { if (m.textContent === '已保存') m.textContent = ''; }, 2000);
  };

  // ---- Ollama 本地模型检测 ----
  $('#ollamaDetect').onclick = async () => {
    const el = $('#msg');
    const ok = (t) => { el.classList.remove('err'); el.textContent = t; };
    const bad = (t) => { el.classList.add('err'); el.textContent = t; };
    ok('检测 Ollama 服务…');
    try {
      const url = $('#ollamaUrl').value.trim() || 'http://localhost:11434';
      const models = await WINOCR.listOllamaModels({ ollamaUrl: url });
      const sel = $('#ollamaModel');
      const cur = sel.value;
      sel.innerHTML = '';
      if (!models.length) {
        bad('Ollama 已连接，但没有可用模型。请先运行：ollama pull qwen2.5:7b');
        return;
      }
      models.forEach((m) => {
        const o = document.createElement('option');
        o.value = m.name;
        const gb = m.size ? (m.size / 1024 / 1024 / 1024).toFixed(1) + 'GB' : '';
        o.textContent = m.name + (gb ? ' · ' + gb : '');
        sel.appendChild(o);
      });
      if (cur && Array.prototype.some.call(sel.options, (o) => o.value === cur)) sel.value = cur;
      ok('检测到 ' + models.length + ' 个本地模型，请选择一个并「保存」');
    } catch (e) {
      bad('检测失败：' + ((e && e.message) || e));
    }
    setTimeout(() => { el.textContent = ''; el.classList.remove('err'); }, 12000);
  };

  // ---- 浏览器内置 Translator API 辅助 ----
  const bSrc = () => { const v = $('#srcLang').value || 'en'; return v === 'auto' ? 'en' : v; };
  const bTgt = () => $('#tgtLang').value || 'zh';
  const hasTranslator = () => (typeof Translator !== 'undefined') && Translator && typeof Translator.availability === 'function';

  // ---- 截图热键：自由录入（按键捕获，不靠手打字符串） ----
  const MOD_KEYS = ['Control', 'Alt', 'Shift', 'Meta', 'AltGraph'];
  function prettyKey(e) {
    const k = e.key;
    if (k === ' ') return 'space';
    if (k === 'Enter') return 'enter';
    if (k === 'Tab') return 'tab';
    if (/^F([1-9]|1\d|2[0-4])$/i.test(k)) return k.toLowerCase();
    if (k.length === 1 && /[a-z0-9]/i.test(k)) return k.toLowerCase();
    return '';
  }
  (function initHotkeyRecorders() {
    const say = (el, t) => { if (el) { el.textContent = t; el.classList.toggle('err', !!t); } };
    function bind(inputSel, msgSel, clearSel) {
      const el = $(inputSel), tip = $(msgSel);
      if (!el) return;
      el.addEventListener('keydown', (e) => {
        e.preventDefault();                       // 不要真的输入字符
        if (MOD_KEYS.indexOf(e.key) !== -1) return;                 // 还在按修饰键，安静等待
        if (e.key === 'Escape' || e.key === 'Backspace' || e.key === 'Delete') { el.value = ''; say(tip, ''); return; }
        const mods = [];
        if (e.ctrlKey) mods.push('ctrl');
        if (e.altKey) mods.push('alt');
        if (e.shiftKey) mods.push('shift');
        if (e.metaKey) mods.push('win');
        const key = prettyKey(e);
        if (!key) { say(tip, '主键请用 字母 / 数字 / F1–F24'); return; }
        if (!mods.length) { say(tip, '至少需要一个修饰键（Ctrl / Alt / Shift / Win）—— 裸键会劫持全系统，不能用'); return; }
        el.value = mods.join('+') + '+' + key;
        say(tip, '');
      });
      el.addEventListener('keyup', (e) => e.preventDefault());
      const btn = $(clearSel);
      if (btn) btn.onclick = () => { el.value = ''; say(tip, ''); el.focus(); };
    }
    bind('#hotkey', '#hotkeyMsg', '#hotkeyClear');
    bind('#quitHotkey', '#quitHotkeyMsg', '#quitHotkeyClear');
  })();

  // 测试连接：严格按「翻译引擎」下拉实测所选引擎（此前写死测 SF，会误导）
  $('#test').onclick = async () => {
    const el = $('#msg');
    const ok = (t) => { el.classList.remove('err'); el.textContent = t; };
    const bad = (t) => { el.classList.add('err'); el.textContent = t; };
    ok('测试中…');
    const src = bSrc(), tgt = bTgt();
    try {
      if ($('#engine').value === 'browser') {
        if (!hasTranslator()) {
          bad('浏览器内置不可用：本机没有 Translator API（需 Edge/Chrome 138+）。请改用 SiliconFlow。');
        } else {
          const avail = await Translator.availability({ sourceLanguage: src, targetLanguage: tgt });
          if (avail === 'available') {
            const tr = await Translator.create({ sourceLanguage: src, targetLanguage: tgt });
            ok('浏览器内置可用（离线、零 key）：' + (await tr.translate('Hello, world.')));
          } else if (avail === 'downloadable' || avail === 'downloading') {
            bad('内核已支持，但 ' + src + '→' + tgt + ' 语言包还没下好（availability = ' + avail + '）—— 点右边「下载语言包」拉一次即可离线使用。');
          } else {
            bad('浏览器内置不支持该语言对（availability = ' + avail + '）。请改用 SiliconFlow。');
          }
        }
      } else if ($('#engine').value === 'mymemory') {
        const t = await WINOCR.translateMyMemory('Hello, world.', {
          srcLang: src, tgtLang: tgt, mymemoryEmail: $('#mymemoryEmail').value.trim()
        });
        ok('MyMemory 连接成功（免费 · 免 key）：' + t);
      } else if ($('#engine').value === 'ollama') {
        const model = $('#ollamaModel').value;
        if (!model) { bad('请先点「检测本地模型」并选择一个模型'); return; }
        const t = await WINOCR.translateOllama('Hello, world.', {
          ollamaUrl: $('#ollamaUrl').value.trim(), ollamaModel: model,
          srcLang: src, tgtLang: tgt
        });
        ok('Ollama 本地模型翻译成功：' + t);
      } else {
        const t = await WINOCR.translateSF('Hello, world.', {
          sfKey: $('#sfKey').value, sfUrl: $('#sfUrl').value, sfModel: $('#sfModel').value,
          srcLang: src, tgtLang: tgt
        });
        ok('SiliconFlow 连接成功：' + t);
      }
    } catch (e) {
      bad('失败：' + e.message);
    }
    setTimeout(() => { el.textContent = ''; el.classList.remove('err'); }, 12000);
  };

  // 下载浏览器内置语言包（必须在用户手势内触发；monitor 可看进度）
  // 注意：语言包托管在 Google 的 CDN（dl.google.com / gstatic）。国内网络到不了时，
  // Translator.create() 会长时间悬而不决、且不触发 downloadprogress —— 故加计时 + 硬超时。
  $('#dlModel').onclick = async () => {
    const el = $('#msg');
    const btn = $('#dlModel');
    const ok = (t) => { el.classList.remove('err'); el.textContent = t; };
    const bad = (t) => { el.classList.add('err'); el.textContent = t; };
    if (!hasTranslator() || typeof Translator.create !== 'function') {
      return bad('本机没有 Translator API（需 Edge/Chrome 138+），无法下载语言包。请用 SiliconFlow。');
    }

    const src = bSrc(), tgt = bTgt();
    const t0 = Date.now();
    let lastPct = null, done = false;
    btn.disabled = true;

    // 计时：每秒刷新"已等待 Ns"，让用户知道它没死
    const ticker = setInterval(() => {
      if (done) return;
      const sec = Math.round((Date.now() - t0) / 1000);
      ok('下载/检查 ' + src + '→' + tgt + ' 语言包… 已等待 ' + sec + 's' +
         (lastPct === null ? '（尚无进度事件）' : '（' + lastPct + '%）'));
    }, 1000);

    let toId = null;
    const timeout = new Promise((_, rej) => {
      toId = setTimeout(() => rej(new Error('TIMEOUT')), TEST_TIMEOUT_MS);
    });
    const cleanup = () => { done = true; clearInterval(ticker); if (toId) clearTimeout(toId); btn.disabled = false; };

    try {
      const tr = await Promise.race([
        Translator.create({
          sourceLanguage: src, targetLanguage: tgt,
          monitor(m) {
            try {
              m.addEventListener('downloadprogress', (e) => {
                lastPct = (e && e.total) ? Math.round((e.loaded / e.total) * 100) : null;
                ok('正在下载语言包 ' + src + '→' + tgt + '…' + (lastPct === null ? '' : ' ' + lastPct + '%'));
              });
            } catch (err) {}
          }
        }),
        timeout
      ]);
      const out = await tr.translate('Hello, world.');
      cleanup();
      $('#engine').value = 'browser';
      ok('语言包就绪，离线零 key 翻译可用：' + out + '（引擎已切到「浏览器内置」，请点「保存」）');
    } catch (e) {
      cleanup();
      const msg = (e && e.message) || String(e);
      if (msg === 'TIMEOUT') {
        bad('等待 45 秒仍无任何下载进度 —— 语言包源（Google 的 CDN）在当前网络下不可达。' +
            '可打开浏览器自带诊断页 chrome://on-device-translation-internals（Edge 用 edge://）' +
            '查看该语言包是否卡在 Installing；若是则确认是网络问题，请保持引擎为「SiliconFlow」。');
      } else if (/notallowed|user activation|gesture/i.test(msg)) {
        bad('下载被拒（需用户手势）：请直接在这个页面点按钮、不要刷新；或稍后再点一次。');
      } else {
        bad('下载失败：' + msg + ' —— 语言包源在国内常被拦截；继续用 SiliconFlow 即可，功能不受影响。');
      }
    }
    setTimeout(() => { el.textContent = ''; el.classList.remove('err'); }, 20000);
  };

  // 测试 OCR：现画一张中英混排的图，按「OCR 引擎」实测一次并报耗时。
  // 本地引擎要经原生宿主执行（扩展自己没有本地 OCR 能力）。
  const ocrViaHost = (dataUrl, timeoutMs) => new Promise((res, rej) => {
    try {
      chrome.runtime.sendMessage({ type: 'ocr.viaHost', dataUrl: dataUrl, timeoutMs: timeoutMs }, (x) => {
        void chrome.runtime.lastError;
        if (!x) return rej(new Error('扩展后台无响应（请到扩展页重新加载）'));
        if (x.error) return rej(new Error(x.error));
        res(x.text || '');
      });
    } catch (e) { rej(e); }
  });

  $('#testOcr').onclick = async () => {
    const el = $('#msg');
    const ok = (t) => { el.classList.remove('err'); el.textContent = t; };
    const bad = (t) => { el.classList.add('err'); el.textContent = t; };
    ok('OCR 测试中…');
    // 画一张「已知答案」的图，便于肉眼核对
    const cv = document.createElement('canvas');
    cv.width = CANVAS_WIDTH; cv.height = CANVAS_HEIGHT;
    const ctx = cv.getContext('2d');
    ctx.fillStyle = '#fff'; ctx.fillRect(0, 0, cv.width, cv.height);
    ctx.fillStyle = '#000'; ctx.font = CANVAS_FONT_SIZE + 'px "Segoe UI", sans-serif';
    ctx.fillText('Urinalysis: WBC 12 /HPF, nitrite positive.', 20, 46);
    ctx.font = CANVAS_FONT_SIZE + 'px "Microsoft YaHei", sans-serif';
    ctx.fillText('患者因急性尿潴留入院。', 20, 96);
    const want = 'Urinalysis: WBC 12 /HPF, nitrite positive. / 患者因急性尿潴留入院。';
    const dataUrl = cv.toDataURL('image/png');
    const t0 = Date.now();
    try {
      let text = '';
      if ($('#ocrEngine').value === 'local') {
        text = await ocrViaHost(dataUrl, OCR_TEST_TIMEOUT_MS);
      } else {
        text = await WINOCR.ocrSF(dataUrl, {
          sfKey: $('#sfKey').value, sfUrl: $('#sfUrl').value, sfOcrModel: $('#sfOcrModel').value
        });
      }
      const dt = ((Date.now() - t0) / 1000).toFixed(1);
      ok('OCR 成功（' + dt + 's）：' + text.replace(/\s+/g, ' ').slice(0, 90) + '  ｜ 期望：' + want);
    } catch (e) {
      const hint = $('#ocrEngine').value === 'local'
        ? '（本地 OCR 由原生宿主执行：请在下方勾选「连接原生宿主」并保存，且宿主已注册；否则请把 OCR 引擎切到云端）'
        : '';
      bad('OCR 失败：' + ((e && e.message) || e) + ' ' + hint);
    }
    setTimeout(() => { el.textContent = ''; el.classList.remove('err'); }, 25000);
  };

  // 检测原生宿主连接（先落盘当前表单，再按勾选状态连接/断开）
  $('#nativeTest').onclick = async () => {
    const el = $('#nativeMsg');
    const ok = (t) => { el.classList.remove('err'); el.textContent = t; };
    const bad = (t) => { el.classList.add('err'); el.textContent = t; };
    ok('检测中…');
    try {
      // 先保存：否则「填了 key 却没点保存」时宿主拿到的还是旧配置，
      // 检测再成功也白搭 —— 这正是那次「扩展已连接、截图却说没 key」的成因。
      try { await saveForm(); markClean(); } catch (e) {}
      const st = await new Promise((res) => chrome.runtime.sendMessage(
        { type: 'native.reconnect', enable: $('#nativeHost').checked },
        (x) => { void chrome.runtime.lastError; res(x); }
      ));
      if (!st) return bad('扩展后台无响应（请到扩展页点「重新加载」）');
      if (!st.enabled) ok('当前为关闭状态 —— 纯浏览器使用无需开启；要联动外部截图请勾选后保存');
      else if (!st.connected) bad('未连接：' + (st.error || '宿主未注册或未启动') + ' —— 请先运行 install_host.bat 并重新加载扩展');
      else {
        const h = st.host || {};
        showCfgPath(h.configPath);
        ok('已桥接扩展 · ' + (h.hotkey ? '生效热键 ' + String(h.hotkey).toUpperCase() : '（等待宿主自报热键…）')
           + (h.quitHotkey ? ' · 退出键 ' + String(h.quitHotkey).toUpperCase() : '')
           + (h.ocrDesc ? ' · OCR ' + h.ocrDesc : '')
           + (h.pid ? ' · pid ' + h.pid : ''));
      }
    } catch (e) { bad('检测失败：' + e.message); }
    setTimeout(() => { el.textContent = ''; el.classList.remove('err'); }, 9000);
  };

  // 从本地配置导入 —— 独立模式读的那份 native_host/winocr_config.json。
  // 两份配置互不同步，这条路径让「只填了一边」的用户一键对齐，不必手抄 key。
  $('#nativeImport').onclick = async () => {
    const el = $('#nativeMsg');
    const ok = (t) => { el.classList.remove('err'); el.textContent = t; };
    const bad = (t) => { el.classList.add('err'); el.textContent = t; };
    ok('读取本地配置…');
    try {
      const r = await new Promise((res) => chrome.runtime.sendMessage(
        { type: 'winocr.hostSettings' }, (x) => { void chrome.runtime.lastError; res(x); }));
      if (!r) return bad('扩展后台无响应（请到扩展页点「重新加载」）');
      if (r.error) return bad(r.error);
      showCfgPath(r.host && r.host.configPath);
      const h = r.settings || {};
      let n = 0;
      const put = (sel, v) => {
        const e = $(sel);
        if (!e || typeof v !== 'string' || !v) return;
        if (e.tagName === 'SELECT' && !Array.prototype.some.call(e.options, (o) => o.value === v)) {
          const o = document.createElement('option');
          o.value = v; o.textContent = v + '（来自本地配置）';
          e.appendChild(o);
        }
        e.value = v; n++;
      };
      // key 单独处理：本地为空时不覆盖浏览器里已填的值
      if (h.sfKey) { $('#sfKey').value = h.sfKey; n++; }
      else if (!$('#sfKey').value) {
        $('#sfKey').placeholder = '本地配置里 key 也是空的：去 native_host/winocr_config.json 填 sfKey';
      }
      put('#sfUrl', h.sfUrl); put('#sfModel', h.sfModel); put('#sfOcrModel', h.sfOcrModel);
      put('#ocrEngine', h.ocrEngine); put('#localOcrTier', h.localOcrTier);
      put('#translateEngine', h.translateEngine);
      put('#mymemoryEmail', h.mymemoryEmail);
      put('#srcLang', h.srcLang); put('#tgtLang', h.tgtLang);
      put('#hotkey', h.hotkey); put('#quitHotkey', h.quitHotkey);
      markDirty();
      ok('已读入本地配置 ' + n + ' 项（key ' +
         (h.sfKey ? '已带回，长度 ' + h.sfKey.length : '为空') +
         '）—— 确认无误后点「保存」写入浏览器设置');
    } catch (e) { bad('导入失败：' + e.message); }
    setTimeout(() => { el.textContent = ''; el.classList.remove('err'); }, 12000);
  };

  // 打开本地配置文件（资源管理器里选中 winocr_config.json）
  $('#nativeOpenCfg').onclick = () => {
    const el = $('#nativeMsg');
    chrome.runtime.sendMessage({ type: 'winocr.openConfigDir' }, (r) => {
      void chrome.runtime.lastError;
      if (r && r.error) {
        el.classList.add('err'); el.textContent = r.error;
      } else {
        el.classList.remove('err');
        el.textContent = '已在资源管理器里选中 winocr_config.json';
      }
      setTimeout(() => { el.textContent = ''; el.classList.remove('err'); }, 6000);
    });
  };

  pullCfgPath();   // 页面打开时就把本地配置路径补上（若已桥接）
})();

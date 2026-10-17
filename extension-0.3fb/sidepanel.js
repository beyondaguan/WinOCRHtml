// sidepanel.js — 面板中枢：翻译（输入 / 划词 / 右键 / 快捷键）+ OCR + 历史 + 导出
(async function () {
  const $ = (s) => document.querySelector(s);
  let s = await WINOCR.getSettings();
  $('#src').value = s.srcLang; $('#tgt').value = s.tgtLang;
  setBadge(s.engine);

  function setBadge(engine) {
    const b = $('#engineBadge');
    if (!b) return;
    const map = { browser: '浏览器内置', mymemory: 'MyMemory', ollama: '本地Ollama' };
    b.textContent = map[engine] || 'SiliconFlow';
  }
  function escapeHtml(t) { return (t || '').replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c])); }
  function fileToDataUrl(f) { return new Promise((res, rej) => { const r = new FileReader(); r.onload = () => res(r.result); r.onerror = rej; r.readAsDataURL(f); }); }

  // ---------------- 原生宿主 / 本地配置路径 ----------------
  // 桌面热键截图与本地 OCR 都靠原生宿主；而独立模式读的是
  // native_host\winocr_config.json —— 和这个面板里保存的设置**不是同一个文件**。
  // 「在扩展里连上了」不代表本地那份有 key，所以把绝对路径直接摆在面板上。
  function renderHost(st) {
    const el = $('#hostLine');
    if (!el) return;
    const h = (st && st.host) || null;
    if (st && st.connected && h) {
      el.className = 'ok';
      el.innerHTML = '原生宿主：已桥接 · 桌面热键 <b>' + escapeHtml(String(h.hotkey || '—').toUpperCase())
        + '</b> · 本地配置 <code>' + escapeHtml(h.configPath || 'native_host\\winocr_config.json')
        + '</code>（桌面截图读这份，与本面板设置是两份文件）';
    } else if (st && !st.enabled) {
      el.className = '';
      el.textContent = '原生宿主：未连接 —— 桌面热键截图 / 本地 OCR 需要它。'
        + '到「选项」勾选「连接原生宿主」并保存，再重新加载扩展。';
    } else {
      el.className = 'warn';
      el.textContent = '原生宿主：未连接 · ' + ((st && st.error) || '宿主未注册或未启动')
        + ' —— 到「选项」点「检测宿主连接」看原因。';
    }
  }
  function pullHost() {
    try {
      chrome.runtime.sendMessage({ type: 'native.status' }, (st) => {
        void chrome.runtime.lastError;
        renderHost(st);
      });
    } catch (e) {}
  }
  pullHost();
  // 宿主可能比面板晚就绪（浏览器按需拉起），补看两次
  setTimeout(pullHost, 1500);
  setTimeout(pullHost, 5000);

  // toast：CSS 控制淡入淡出（.show）
  let msgTimer = null;
  function msg(t, isErr) {
    const m = $('#msg');
    m.textContent = t;
    m.classList.toggle('err', !!isErr);
    m.classList.add('show');
    clearTimeout(msgTimer);
    msgTimer = setTimeout(() => m.classList.remove('show'), 2600);
  }

  function showResult(original, translation, err) {
    const el = $('#result');
    el.classList.remove('is-empty');
    if (err) { el.innerHTML = '<div class="err">' + escapeHtml(err) + '</div>'; return; }
    el.innerHTML =
      '<div><div class="lab">原文</div><div class="o">' + escapeHtml(original) + '</div></div>' +
      '<div><div class="t">' + escapeHtml(translation) + '</div></div>';
  }
  function setLoading(text) {
    const el = $('#result');
    el.classList.remove('is-empty');
    el.innerHTML = '<span class="ph">' + escapeHtml(text) + '</span>';
  }
  function currentTranslation() { const t = $('#result .t'); return t ? t.textContent : ''; }
  function currentOriginal() { const o = $('#result .o'); return o ? o.textContent : ''; }

  async function refresh() {
    const hist = await WINOCR.getHistory();
    const el = $('#hist'); el.innerHTML = '';
    if (!hist.length) { el.innerHTML = '<div class="empty">暂无记录</div>'; return; }
    hist.slice(-30).reverse().forEach((r) => {
      const d = document.createElement('div'); d.className = 'item';
      d.innerHTML =
        '<div class="meta"><span class="kind">' + (r.type === 'image' ? '图' : '文') + '</span>' +
        '<span>' + ((r.ts || '').slice(0, 16).replace('T', ' ')) + '</span></div>' +
        '<div class="o">' + escapeHtml((r.original || r.ocr || '').slice(0, 160)) + '</div>' +
        '<div class="t">' + escapeHtml((r.translation || '').slice(0, 160)) + '</div>';
      el.appendChild(d);
    });
  }
  await refresh();

  // 历史/设置变化时自动刷新（原生宿主回传的记录也会实时出现）
  try {
    chrome.storage.onChanged.addListener((ch, area) => { if (area === 'local') refresh(); });
  } catch (e) {}

  // ---------------- 文本翻译 ----------------
  async function translateText(text) {
    text = (text || '').trim();
    if (!text) { msg('请输入或选取要翻译的文本', true); return; }
    setLoading('翻译中…');
    s = await WINOCR.getSettings();
    setBadge(s.engine);
    try {
      const out = await WINOCR.translate(text, {
        engine: s.engine, sfKey: s.sfKey, sfUrl: s.sfUrl, sfModel: s.sfModel,
        mymemoryEmail: s.mymemoryEmail,
        ollamaUrl: s.ollamaUrl, ollamaModel: s.ollamaModel,
        srcLang: s.srcLang, tgtLang: s.tgtLang
      });
      showResult(text, out);
      await WINOCR.addRecord({ type: 'text', source: 'panel', original: text, translation: out });
      await refresh();
    } catch (e) {
      showResult('', '', '翻译失败：' + e.message);
    }
  }

  $('#doTranslate').onclick = () => translateText($('#input').value);
  $('#input').addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); translateText($('#input').value); }
  });

  // 取当前页面选中 → 填入输入框
  $('#fromSel').onclick = async () => {
    try {
      const tabs = await new Promise((r) => chrome.tabs.query({ active: true, currentWindow: true }, (t) => r(t || [])));
      const tab = tabs[0]; if (!tab) return;
      const res = await new Promise((resolve) => chrome.tabs.sendMessage(tab.id, { type: 'winocr.getSelection' }, (x) => { void chrome.runtime.lastError; resolve(x); }));
      const text = ((res && res.text) || '').trim();
      if (!text) { msg('当前页未选中文字', true); return; }
      $('#input').value = text;
      $('#input').focus();
    } catch (e) { msg('取选中失败：' + e.message, true); }
  };

  // 整页翻译：通知 content script 遍历正文并翻译
  $('#fpTranslate').onclick = async () => {
    try {
      const tabs = await new Promise((r) => chrome.tabs.query({ active: true, currentWindow: true }, (t) => r(t || [])));
      const tab = tabs[0]; if (!tab) return;
      msg('正在翻译整页（可能需要几秒到几十秒）…');
      $('#fpTranslate').disabled = true;
      await new Promise((resolve) => chrome.tabs.sendMessage(tab.id, { type: 'winocr.fullpage' }, (x) => { void chrome.runtime.lastError; resolve(x); }));
      setTimeout(() => { $('#fpTranslate').disabled = false; }, 2000);
    } catch (e) { msg('整页翻译失败：' + e.message, true); $('#fpTranslate').disabled = false; }
  };
  $('#fpRestore').onclick = async () => {
    try {
      const tabs = await new Promise((r) => chrome.tabs.query({ active: true, currentWindow: true }, (t) => r(t || [])));
      const tab = tabs[0]; if (!tab) return;
      await new Promise((resolve) => chrome.tabs.sendMessage(tab.id, { type: 'winocr.fullpage.restore' }, (x) => { void chrome.runtime.lastError; resolve(x); }));
      msg('已还原原文');
    } catch (e) { msg('还原失败：' + e.message, true); }
  };

  $('#saveRec').onclick = async () => {
    const o = currentOriginal() || ($('#input').value || '').trim();
    const t = currentTranslation();
    if (!o && !t) { msg('暂无可记录内容', true); return; }
    await WINOCR.addRecord({ type: 'text', source: 'panel', original: o, translation: t });
    await refresh(); msg('已记录');
  };
  $('#copyOut').onclick = () => {
    const t = currentTranslation();
    if (!t) { msg('暂无译文', true); return; }
    navigator.clipboard.writeText(t); msg('已复制译文');
  };

  // 接收后台（右键菜单 / 快捷键 Alt+Shift+Z）推来的内容
  try {
    chrome.runtime.onMessage.addListener((m) => {
      if (!m) return;
      if (m.type === 'panel-input') { $('#input').value = m.text || ''; }
      else if (m.type === 'panel-result') {
        showResult(m.original || '', m.translation || '', m.error || '');
        refresh();
      } else if (m.type === 'winocr.regionCrop') {
        // 浏览器内区域截图裁剪结果 → 走 OCR + 翻译
        if (m.dataUrl) doOcr(m.dataUrl);
      }
    });
  } catch (e) {}

  // ---------------- 截图 OCR ----------------
  // OCR 引擎可切：'local' 交给原生宿主的 PP-OCRv6（实测 1.2~2.2s、离线、零 key）；
  // 'sf' 走云端视觉模型（PaddleOCR-VL 实测 103~121s、DeepSeek-OCR 输出不稳）。
  const ocrViaHost = (dataUrl, timeoutMs) => new Promise((res, rej) => {
    try {
      chrome.runtime.sendMessage({ type: 'ocr.viaHost', dataUrl: dataUrl, timeoutMs: timeoutMs }, (x) => {
        void chrome.runtime.lastError;
        if (!x) return rej(new Error('扩展后台无响应'));
        if (x.error) return rej(new Error(x.error));
        res(x.text || '');
      });
    } catch (e) { rej(e); }
  });

  async function runOcr(dataUrl, st) {
    if ((st.ocrEngine || 'local') === 'local') {
      try {
        return await ocrViaHost(dataUrl, 90000);
      } catch (e) {
        // 本地不可用（宿主没连/没注册/缺依赖）→ 回落云端，别让用户两手空空
        msg('本地 OCR 不可用（' + e.message + '），已改用云端…', true);
      }
    }
    return await WINOCR.ocrSF(dataUrl, st);
  }

  async function doOcr(dataUrl) {
    setLoading('OCR 中…');
    try {
      s = await WINOCR.getSettings();
      setBadge(s.engine);
      const ocr = await runOcr(dataUrl, s);
      const tr = await WINOCR.translate(ocr, {
        engine: s.engine, sfKey: s.sfKey, sfUrl: s.sfUrl, sfModel: s.sfModel,
        mymemoryEmail: s.mymemoryEmail,
        ollamaUrl: s.ollamaUrl, ollamaModel: s.ollamaModel,
        srcLang: s.srcLang, tgtLang: s.tgtLang
      });
      showResult(ocr, tr);
      await WINOCR.addRecord({ type: 'image', source: 'clipboard', ocr: ocr, translation: tr });
      await refresh();
    } catch (e) { showResult('', '', 'OCR 失败：' + e.message); }
  }

  document.addEventListener('paste', async (e) => {
    const it = Array.from(e.clipboardData.items).find((i) => i.type.startsWith('image/'));
    if (it) doOcr(await fileToDataUrl(it.getAsFile()));
  });
  const drop = $('#drop');
  ['dragenter', 'dragover'].forEach((ev) => drop.addEventListener(ev, (e) => { e.preventDefault(); drop.classList.add('over'); }));
  ['dragleave', 'drop'].forEach((ev) => drop.addEventListener(ev, (e) => { e.preventDefault(); drop.classList.remove('over'); }));
  drop.addEventListener('drop', async (e) => { const f = e.dataTransfer.files[0]; if (f && f.type.startsWith('image/')) doOcr(await fileToDataUrl(f)); });
  $('#pick').onclick = () => $('#file').click();
  $('#file').onchange = async (e) => { const f = e.target.files[0]; if (f) doOcr(await fileToDataUrl(f)); };

  // 浏览器内区域截图：捕获当前标签页可见区域 → 页面弹选区遮罩 → 裁剪回传 OCR
  $('#captureTab').onclick = () => {
    $('#captureTab').disabled = true;
    msg('正在截取页面…');
    try {
      chrome.runtime.sendMessage({ type: 'capture.visibleTab.start' }, (r) => {
        void chrome.runtime.lastError;
        if (r && r.error) msg('截图失败：' + r.error, true);
        // 选区裁剪结果由 chrome.runtime.onMessage('winocr.regionCrop') 接收
      });
    } catch (e) { msg('截图失败：' + e.message, true); }
    setTimeout(() => { $('#captureTab').disabled = false; }, 1500);
  };

  // 视频双语字幕（YouTube）：开启/关闭实时字幕翻译
  async function sendToActiveTab(msg) {
    try {
      const tabs = await new Promise((r) => chrome.tabs.query({ active: true, currentWindow: true }, (t) => r(t || [])));
      const tab = tabs[0]; if (!tab) return null;
      return await new Promise((res) => chrome.tabs.sendMessage(tab.id, msg, (x) => { void chrome.runtime.lastError; res(x); }));
    } catch (e) { return null; }
  }
  $('#subOn').onclick = async () => {
    const r = await sendToActiveTab({ type: 'winocr.subtitle.start' });
    if (r && r.ok) msg('视频字幕翻译已开启（请先在 YouTube 打开字幕）');
    else msg('当前页面不是 YouTube 或未开启字幕', true);
  };
  $('#subOff').onclick = async () => {
    await sendToActiveTab({ type: 'winocr.subtitle.stop' });
    msg('视频字幕翻译已关闭');
  };

  // PDF 翻译：提取文本 → 逐页翻译 → 双语结果显示在面板
  $('#pickPdf').onclick = () => $('#pdfFile').click();
  $('#pdfFile').onchange = async (e) => {
    const f = e.target.files[0]; if (!f) return;
    $('#pickPdf').disabled = true;
    setLoading('正在解析 PDF（首次会加载 PDF.js）…');
    try {
      s = await WINOCR.getSettings();
      const pages = await WINOCR.extractPdfText(f, (cur, total) => {
        setLoading('解析 PDF：第 ' + cur + ' / ' + total + ' 页');
      });
      const total = pages.length;
      const results = [];
      for (let i = 0; i < pages.length; i++) {
        const p = pages[i];
        setLoading('翻译第 ' + (i + 1) + ' / ' + total + ' 页…');
        if (!p.text) { results.push({ page: p.page, orig: '', tr: '' }); continue; }
        try {
          const tr = await WINOCR.translate(p.text, {
            engine: s.engine, sfKey: s.sfKey, sfUrl: s.sfUrl, sfModel: s.sfModel,
            mymemoryEmail: s.mymemoryEmail,
            ollamaUrl: s.ollamaUrl, ollamaModel: s.ollamaModel,
            srcLang: s.srcLang, tgtLang: s.tgtLang
          });
          results.push({ page: p.page, orig: p.text, tr: tr });
        } catch (err) {
          results.push({ page: p.page, orig: p.text, tr: '翻译失败：' + err.message });
        }
      }
      // 渲染双语结果到结果区
      const el = $('#result');
      el.classList.remove('is-empty');
      el.innerHTML = results.map((r) =>
        '<div style="margin-bottom:12px">' +
        '<div class="lab" style="color:var(--faint)">第 ' + r.page + ' 页</div>' +
        (r.orig ? '<div class="o" style="color:var(--muted);white-space:pre-wrap">' + escapeHtml(r.orig).slice(0, 500) + '</div>' : '') +
        (r.tr ? '<div class="t" style="white-space:pre-wrap">' + escapeHtml(r.tr) + '</div>' : '') +
        '</div>'
      ).join('');
      msg('PDF 翻译完成：共 ' + total + ' 页');
    } catch (e) {
      showResult('', '', 'PDF 翻译失败：' + e.message);
    }
    $('#pickPdf').disabled = false;
    e.target.value = '';
  };

  // ---------------- AI 对话（持久化） ----------------
  // 对话历史保存在 chrome.storage.local（K_CHAT_HISTORY），支持多轮、多对话切换、导出
  let chatHistory = [
    { role: 'system', content: '你是一个乐于助人的 AI 助手。可以帮用户翻译、解释、润色文本，回答问题。回答简洁明了。' }
  ];
  let currentChatId = null;   // 当前对话 id（null = 尚未保存的新对话）
  let chatDirty = false;      // 是否有未保存的改动

  const chatMsgsEl = () => $('#chatMsgs');

  function chatAppend(role, text) {
    const wrap = document.createElement('div');
    wrap.className = 'chat-msg ' + role;
    wrap.style.cssText = role === 'user'
      ? 'align-self:flex-end;max-width:85%;background:var(--accent-soft);border:1px solid var(--accent-line);border-radius:10px;padding:8px 10px;font-size:12px'
      : 'align-self:flex-start;max-width:85%;background:var(--surface-2);border:1px solid var(--border);border-radius:10px;padding:8px 10px;font-size:12px';
    wrap.textContent = text;
    chatMsgsEl().appendChild(wrap);
    chatMsgsEl().scrollTop = chatMsgsEl().scrollHeight;
    return wrap;
  }

  async function chatPersist() {
    if (!chatDirty) return;
    try {
      const engine = $('#chatEngine').value;
      s = await WINOCR.getSettings();
      const model = engine === 'ollama' ? s.ollamaModel : s.sfModel;
      if (currentChatId) {
        await WINOCR.saveChat(currentChatId, chatHistory, null, engine, model);
      } else {
        const title = (chatHistory[1] && chatHistory[1].content || '新对话').slice(0, 20);
        const chat = await WINOCR.createChat(title, engine, model);
        currentChatId = chat.id;
        await WINOCR.saveChat(currentChatId, chatHistory, null, engine, model);
      }
      chatDirty = false;
    } catch (e) { /* 保存失败不阻塞对话 */ }
  }

  async function chatSend() {
    const input = $('#chatInput');
    const text = (input.value || '').trim();
    if (!text) return;
    input.value = '';
    chatAppend('user', text);
    chatHistory.push({ role: 'user', content: text });
    chatDirty = true;
    const aiBubble = chatAppend('ai', '思考中…');
    $('#chatSend').disabled = true;
    try {
      s = await WINOCR.getSettings();
      const engine = $('#chatEngine').value;
      const reply = await WINOCR.chat(chatHistory, {
        engine: engine, sfKey: s.sfKey, sfUrl: s.sfUrl, sfModel: s.sfModel,
        ollamaUrl: s.ollamaUrl, ollamaModel: s.ollamaModel
      });
      aiBubble.textContent = reply;
      chatHistory.push({ role: 'assistant', content: reply });
      chatDirty = true;
      chatPersist();
    } catch (e) {
      aiBubble.textContent = '出错：' + ((e && e.message) || e);
    }
    $('#chatSend').disabled = false;
  }
  $('#chatSend').onclick = chatSend;
  $('#chatInput').addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); chatSend(); }
  });
  $('#chatClear').onclick = async () => {
    // 删除当前对话并新建空白对话
    if (currentChatId) {
      try { await WINOCR.deleteChat(currentChatId); } catch (e) {}
    }
    currentChatId = null;
    chatHistory = [
      { role: 'system', content: '你是一个乐于助人的 AI 助手。可以帮用户翻译、解释、润色文本，回答问题。回答简洁明了。' }
    ];
    chatMsgsEl().innerHTML = '<div class="chat-msg ai" style="align-self:flex-start;max-width:85%;background:var(--surface-2);border:1px solid var(--border);border-radius:10px;padding:8px 10px;font-size:12px">已新建空白对话。</div>';
    msg('已新建空白对话');
  };
  $('#chatNew').onclick = async () => {
    // 保存当前对话后新建空白对话
    await chatPersist();
    currentChatId = null;
    chatHistory = [
      { role: 'system', content: '你是一个乐于助人的 AI 助手。可以帮用户翻译、解释、润色文本，回答问题。回答简洁明了。' }
    ];
    chatMsgsEl().innerHTML = '<div class="chat-msg ai" style="align-self:flex-start;max-width:85%;background:var(--surface-2);border:1px solid var(--border);border-radius:10px;padding:8px 10px;font-size:12px">已新建空白对话，输入消息即开始。</div>';
    await chatLoadList();
  };

  // 加载历史对话列表
  async function chatLoadList() {
    try {
      const list = await WINOCR.getChatList();
      const el = $('#chatList');
      if (!list.length) { el.innerHTML = '<div class="empty">暂无对话历史</div>'; return; }
      el.innerHTML = list.sort((a, b) => b.updatedAt - a.updatedAt).map((c) =>
        '<div class="item" style="display:flex;justify-content:space-between;align-items:center;padding:5px 8px;border-bottom:1px solid var(--border);cursor:pointer" data-chat-id="' + c.id + '">' +
        '<span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">' + escapeHtml(c.title) +
        ' <span style="color:var(--faint);font-size:10px">' + c.messageCount + ' 条</span></span>' +
        '<button data-export-md="' + c.id + '" style="font-size:10px;padding:0 4px;margin-left:4px">MD</button>' +
        '<button data-export-json="' + c.id + '" style="font-size:10px;padding:0 4px">JSON</button>' +
        '<button data-del-chat="' + c.id + '" style="font-size:10px;padding:0 4px;color:var(--err)">×</button></div>'
      ).join('');
    } catch (e) { /* ignore */ }
  }

  // 切换/删除/导出对话（事件委托）
  document.addEventListener('click', async (e) => {
    const loadBtn = e.target.closest('[data-chat-id]');
    if (loadBtn && !e.target.closest('button')) {
      const id = loadBtn.dataset.chatId;
      try {
        const chat = await WINOCR.getChat(id);
        if (!chat) return;
        currentChatId = chat.id;
        chatHistory = chat.messages && chat.messages.length ? chat.messages : [
          { role: 'system', content: '你是一个乐于助人的 AI 助手。可以帮用户翻译、解释、润色文本，回答问题。回答简洁明了。' }
        ];
        chatMsgsEl().innerHTML = '';
        chatHistory.forEach((m) => {
          if (m.role !== 'system') chatAppend(m.role, m.content);
        });
        $('#chatEngine').value = chat.engine || 'sf';
        msg('已加载对话：' + chat.title);
      } catch (err) { msg('加载失败：' + err.message, true); }
      return;
    }
    const delBtn = e.target.closest('[data-del-chat]');
    if (delBtn) {
      const id = delBtn.dataset.delChat;
      try {
        await WINOCR.deleteChat(id);
        if (currentChatId === id) {
          currentChatId = null;
          chatHistory = [
            { role: 'system', content: '你是一个乐于助人的 AI 助手。可以帮用户翻译、解释、润色文本，回答问题。回答简洁明了。' }
          ];
          chatMsgsEl().innerHTML = '<div class="chat-msg ai" style="align-self:flex-start;max-width:85%;background:var(--surface-2);border:1px solid var(--border);border-radius:10px;padding:8px 10px;font-size:12px">已新建空白对话。</div>';
        }
        await chatLoadList();
        msg('已删除对话');
      } catch (err) { msg('删除失败：' + err.message, true); }
      return;
    }
    const exportBtn = e.target.closest('[data-export-md], [data-export-json]');
    if (exportBtn) {
      const id = exportBtn.dataset.exportMd || exportBtn.dataset.exportJson;
      const format = exportBtn.dataset.exportMd ? 'md' : 'json';
      try {
        const chat = await WINOCR.getChat(id);
        if (!chat) return;
        WINOCR.downloadChat(chat, format === 'md' ? 'markdown' : 'json');
        msg('已导出 ' + (format === 'md' ? 'Markdown' : 'JSON'));
      } catch (err) { msg('导出失败：' + err.message, true); }
    }
  });

  await chatLoadList();

  // ---------------- 学习（生词本 + SM-2 复习） ----------------
  let reviewQueue = [];
  let currentReviewIdx = 0;

  async function studyRefreshStats() {
    try {
      const stats = await WINOCR.getStudyStats();
      $('#studyStats').innerHTML =
        '总词数 <b>' + stats.total + '</b> · 已掌握 <b style="color:var(--ok)">' + stats.mastered + '</b> · ' +
        '学习中 <b style="color:var(--warn)">' + stats.learning + '</b> · 新词 <b>' + stats.newWords + '</b> · ' +
        '今日复习 <b>' + stats.reviewedToday + '</b> · 平均掌握度 <b>' + stats.avgMastery + '</b>';
    } catch (e) {
      $('#studyStats').textContent = '加载失败：' + e.message;
    }
  }

  $('#studyAuto').onclick = async () => {
    $('#studyAuto').disabled = true;
    msg('正在从历史记录采集生词…');
    try {
      const n = await WINOCR.autoCollectFromHistory(50);
      msg('已采集 ' + n + ' 个生词');
      await studyRefreshStats();
    } catch (e) {
      msg('采集失败：' + e.message, true);
    }
    $('#studyAuto').disabled = false;
  };

  $('#studyReview').onclick = async () => {
    try {
      reviewQueue = await WINOCR.getReviewQueue(20);
      if (!reviewQueue.length) {
        msg('当前没有待复习的生词');
        $('#reviewArea').style.display = 'none';
        return;
      }
      currentReviewIdx = 0;
      $('#reviewArea').style.display = 'block';
      studyShowReview();
    } catch (e) {
      msg('加载复习队列失败：' + e.message, true);
    }
  };

  function studyShowReview() {
    if (currentReviewIdx >= reviewQueue.length) {
      $('#reviewArea').style.display = 'none';
      msg('本轮复习完成！');
      studyRefreshStats();
      return;
    }
    const item = reviewQueue[currentReviewIdx];
    $('#reviewWord').textContent = item.word;
    $('#reviewTrans').textContent = item.translation || '（无译文）';
    $('#reviewCtx').textContent = item.context || '';
    $('#reviewCard').style.borderColor = 'var(--accent-line)';
  }

  // 复习评分按钮（事件委托）
  document.addEventListener('click', async (e) => {
    const btn = e.target.closest('#reviewArea [data-q]');
    if (!btn) return;
    const quality = parseInt(btn.dataset.q, 10);
    const item = reviewQueue[currentReviewIdx];
    if (!item) return;
    try {
      await WINOCR.submitReview(item.id, quality);
      currentReviewIdx++;
      studyShowReview();
    } catch (err) {
      msg('提交失败：' + err.message, true);
    }
  });

  // 删除生词（事件委托）
  document.addEventListener('click', async (e) => {
    const btn = e.target.closest('#vocabList [data-del]');
    if (!btn) return;
    const id = btn.dataset.del;
    try {
      await WINOCR.removeVocab(id);
      await studyRefreshStats();
      msg('已删除生词');
    } catch (err) {
      msg('删除失败：' + err.message, true);
    }
  });

  // 加载生词列表
  async function studyLoadVocab() {
    try {
      const vocab = await WINOCR.getVocabList();
      const el = $('#vocabList');
      if (!vocab.length) { el.innerHTML = '<div class="empty">暂无生词</div>'; return; }
      el.innerHTML = vocab.slice(-30).reverse().map((v) =>
        '<div class="item" style="display:flex;justify-content:space-between;align-items:center;padding:4px 8px;border-bottom:1px solid var(--border)">' +
        '<span><b>' + escapeHtml(v.word) + '</b> <span style="color:var(--muted);font-size:11px">' + escapeHtml(v.translation || '') + '</span> ' +
        '<span style="font-size:10px;color:var(--faint)">' + (v.mastery * 100).toFixed(0) + '%</span></span>' +
        '<button data-del="' + v.id + '" style="font-size:11px;padding:1px 6px">×</button></div>'
      ).join('');
    } catch (e) { /* ignore */ }
  }

  await studyRefreshStats();
  await studyLoadVocab();

  // ---------------- 导出 ----------------
  document.querySelectorAll('[data-x]').forEach((btn) => {
    btn.onclick = async () => {
      const hist = await WINOCR.getHistory();
      if (!hist.length) { msg('暂无记录', true); return; }
      btn.disabled = true;
      try {
        const x = btn.dataset.x; let n;
        if (x === 'folder') n = await WINOCR.exportToFolder(hist);
        else if (x === 'zip') n = await WINOCR.exportToZip(hist);
        else if (x === 'obsidian') n = await WINOCR.exportToObsidian(hist, s);
        else if (x === 'lexiang') n = await WINOCR.exportToLexiang(hist, s);
        msg('已导出 ' + n + ' 项');
      } catch (e) { msg('导出失败：' + e.message, true); }
      btn.disabled = false;
    };
  });

  $('#clear').onclick = async () => { await WINOCR.clearHistory(); await refresh(); msg('已清空'); };
  $('#reload').onclick = async () => { await refresh(); msg('已刷新'); };
  $('#src').onchange = async () => { s.srcLang = $('#src').value; await WINOCR.setSettings(s); };
  $('#tgt').onchange = async () => { s.tgtLang = $('#tgt').value; await WINOCR.setSettings(s); };

  // 编排入场：给分节加 .in（错峰由 CSS :nth-child 控制）
  requestAnimationFrame(() => {
    document.querySelectorAll('.rv').forEach((el) => el.classList.add('in'));
  });
})();

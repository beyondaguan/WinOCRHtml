// onboarding.js — 首次运行 5 步引导（§16.3）
// 原则：5 步以内、每步可跳过、全程不索取 API Key、结束前必须成功一次。
// 第 5 步不可跳过；完成后写 winocr_onboarded 标志，并通知壳侧清除 first_run。
(function () {
  'use strict';

  const TOTAL = 5;
  let cur = 1;
  let settings = null;          // 当前扩展设置（用于回写引擎/语言）
  let finished = false;

  const $ = (s) => document.querySelector(s);
  const steps = Array.from(document.querySelectorAll('.step'));
  const progress = $('#progress');
  const titleEl = $('#title');
  const subEl = $('#subtitle');
  const dotsEl = $('#dots');
  const prevBtn = $('#prevBtn');
  const nextBtn = $('#nextBtn');
  const skipBtn = $('#skipBtn');

  const TITLES = {
    1: ['欢迎使用 WinOCR-Html', '全场景 OCR 翻译：截图、划词、离线都能用'],
    2: ['选择翻译引擎', '默认免费引擎即可开始，无需任何 Key'],
    3: ['确认快捷键', '都用默认即可，随时能在设置里改'],
    4: ['选择本地 OCR 模型', 'tiny 已内置，开箱即用'],
    5: ['试一次，就完成', '按 Alt+Q 框选一段英文，跑通即结束'],
  };

  // 渲染进度条
  function renderProgress() {
    progress.innerHTML = '';
    for (let i = 1; i <= TOTAL; i++) {
      const b = document.createElement('div');
      b.className = 'bar' + (i < cur ? ' done' : i === cur ? ' active' : '');
      progress.appendChild(b);
    }
    const t = TITLES[cur];
    titleEl.textContent = t[0];
    subEl.textContent = t[1];
    dotsEl.textContent = cur + ' / ' + TOTAL;
    prevBtn.style.visibility = cur === 1 ? 'hidden' : 'visible';
    skipBtn.style.display = cur === 5 ? 'none' : '';
    nextBtn.textContent = cur === TOTAL ? '完成引导' : '下一步';
  }

  function showStep(n) {
    cur = Math.max(1, Math.min(TOTAL, n));
    steps.forEach((s) => s.classList.toggle('show', +s.dataset.step === cur));
    // 第 5 步进入时启动“成功检测”轮询
    if (cur === 5) startSuccessWatch();
    renderProgress();
  }

  // ---- 选项高亮 ----
  function bindOptions(group, attr) {
    document.querySelectorAll('.opt[' + attr + ']').forEach((opt) => {
      opt.addEventListener('click', () => {
        document.querySelectorAll('.opt[' + attr + ']').forEach((o) => o.classList.remove('sel'));
        opt.classList.add('sel');
        const input = opt.querySelector('input');
        if (input) input.checked = true;
        onSelect(group, opt.getAttribute(attr));
      });
    });
  }

  function onSelect(group, val) {
    if (group === 'engine') {
      $('#sfBox').style.display = val === 'sf' ? '' : 'none';
    }
  }
  bindOptions('engine', 'data-engine');
  bindOptions('tier', 'data-tier');

  // ---- 第 5 步：检测一次成功翻译 ----
  let watchTimer = null;
  function startSuccessWatch() {
    if (watchTimer || finished) return;
    $('#tryState').textContent = '监听中…成功一次后将自动完成。';
    watchTimer = setInterval(async () => {
      try {
        const s = await chrome.storage.local.get('winocr_last_success');
        if (s && s.winocr_last_success) {
          clearInterval(watchTimer); watchTimer = null;
          markSuccess();
        }
      } catch (e) {}
    }, 700);
  }

  function markSuccess() {
    if (finished) return;
    finished = true;
    document.querySelector('.trybox').style.display = 'none';
    $('#success').classList.add('show');
    nextBtn.textContent = '完成引导';
    nextBtn.disabled = false;
  }

  // ---- 持久化选择 ----
  async function applyChoices() {
    if (!settings) settings = await WINOCR.getSettings();
    const merged = Object.assign({}, settings);
    merged.srcLang = $('#srcLang').value;
    merged.tgtLang = $('#tgtLang').value;
    merged.uiLang = $('#uiLang').value;
    merged.engine = document.querySelector('input[name=engine]:checked').value;
    merged.translateEngine = merged.engine === 'sf' ? 'sf' : 'mymemory';
    merged.localOcrTier = document.querySelector('input[name=tier]:checked').value;
    const k = $('#sfKey').value.trim();
    if (k) merged.sfKey = k;
    await WINOCR.setSettings(merged);
    settings = merged;
  }

  // 完成：写 onboarded 标志 + 通知壳侧
  async function finish() {
    try { await chrome.storage.local.set({ winocr_onboarded: true }); } catch (e) {}
    try {
      chrome.runtime.sendMessage({ type: 'winocr.firstRunComplete' }, () => void chrome.runtime.lastError);
    } catch (e) {}
    // 轻微延时确保消息发出
    setTimeout(() => window.close(), 300);
  }

  // ---- 按钮事件 ----
  nextBtn.addEventListener('click', async () => {
    if (cur < TOTAL) { await applyChoices(); showStep(cur + 1); }
    else { await applyChoices(); if (!finished) markSuccess(); else finish(); }
  });
  prevBtn.addEventListener('click', () => showStep(cur - 1));
  skipBtn.addEventListener('click', () => { if (cur < TOTAL) showStep(cur + 1); });

  // 初始化
  (async () => {
    try { settings = await WINOCR.getSettings(); } catch (e) {}
    if (settings) {
      $('#srcLang').value = settings.srcLang || 'en';
      $('#tgtLang').value = settings.tgtLang || 'zh';
      if (settings.uiLang) $('#uiLang').value = settings.uiLang;
    }
    showStep(1);
  })();
})();

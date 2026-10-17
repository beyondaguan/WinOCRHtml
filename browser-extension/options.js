// WinOCR 划词助手 - Options
// 读取/保存 chrome.storage.sync 中的设置；显示桌面端连接状态。

(() => {
  "use strict";

  const $ = (id) => document.getElementById(id);

  // ---- 默认设置 ----
  const DEFAULTS = {
    srcLang: "auto",
    tgtLang: "zh",
    engine: "host",
    autoBubble: true,
    bubblePersist: true,
    bubbleTimeout: 0,
    ctxTranslate: true,
    ctxCollect: true,
    ttsRate: "1.0",
    ttsPitch: "1.0",
    sfKey: "",
    sfUrl: "https://api.siliconflow.cn/v1",
    sfModel: "Qwen/Qwen3-8B",
    mymemoryEmail: "",
    ollamaUrl: "http://localhost:11434",
    ollamaModel: "",
    captureShortcut: "Alt+1",
    translateShortcut: ""
  };

  // ---- 折叠卡片 ----
  document.querySelectorAll(".card-header").forEach((hd) => {
    hd.addEventListener("click", () => {
      hd.parentElement.classList.toggle("open");
    });
  });

  // ---- 开关组件 ----
  document.querySelectorAll(".toggle").forEach((tg) => {
    tg.addEventListener("click", () => {
      tg.classList.toggle("on");
    });
  });

  // 常显示开关：控制延迟输入框可用性
  $("bubblePersist").addEventListener("click", () => {
    const on = $("bubblePersist").classList.contains("on");
    $("bubbleTimeout").disabled = on;
  });

  // ---- Toast ----
  let toastTimer = null;
  function showToast(msg, type) {
    const toast = $("toast");
    toast.textContent = msg;
    toast.className = "toast show " + (type || "");
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { toast.className = "toast"; }, 2500);
  }

  // ---- 引擎字段显隐 ----
  function updateEngineFields() {
    const eng = $("engine").value;
    $("sfFields").style.display = eng === "sf" ? "" : "none";
    $("mmFields").style.display = eng === "mymemory" ? "" : "none";
    $("ollamaFields").style.display = eng === "ollama" ? "" : "none";
    $("browserFields").style.display = eng === "browser" ? "" : "none";
  }

  // ---- 读取设置 ----
  function loadSettings() {
    chrome.storage.sync.get(DEFAULTS, (items) => {
      if (chrome.runtime.lastError) {
        showToast("读取设置失败：" + chrome.runtime.lastError.message, "error");
        return;
      }
      $("srcLang").value = items.srcLang;
      $("tgtLang").value = items.tgtLang;
      $("engine").value = items.engine || "host";
      $("sfKey").value = items.sfKey || "";
      $("sfUrl").value = items.sfUrl || DEFAULTS.sfUrl;
      $("sfModel").value = items.sfModel || DEFAULTS.sfModel;
      $("mymemoryEmail").value = items.mymemoryEmail || "";
      $("ollamaUrl").value = items.ollamaUrl || DEFAULTS.ollamaUrl;
      $("ollamaModel").value = items.ollamaModel || "";
      $("autoBubble").classList.toggle("on", !!items.autoBubble);
      // 常显示开关：bubbleTimeout === 0 视为常显示
      const persist = (typeof items.bubblePersist === "boolean")
        ? items.bubblePersist
        : (Number(items.bubbleTimeout) === 0);
      $("bubblePersist").classList.toggle("on", persist);
      $("bubbleTimeout").value = persist ? 8 : (Number(items.bubbleTimeout) || 8);
      $("bubbleTimeout").disabled = persist;
      $("ctxTranslate").classList.toggle("on", !!items.ctxTranslate);
      $("ctxCollect").classList.toggle("on", !!items.ctxCollect);
      $("ttsRate").value = items.ttsRate;
      $("ttsPitch").value = items.ttsPitch;
      $("captureShortcut").value = items.captureShortcut || "";
      $("translateShortcut").value = items.translateShortcut || "";
      updateEngineFields();
    });
  }

  // ---- 保存设置 ----
  function collectSettings() {
    return {
      srcLang: $("srcLang").value,
      tgtLang: $("tgtLang").value,
      engine: $("engine").value,
      sfKey: $("sfKey").value.trim(),
      sfUrl: $("sfUrl").value.trim() || DEFAULTS.sfUrl,
      sfModel: $("sfModel").value,
      mymemoryEmail: $("mymemoryEmail").value.trim(),
      ollamaUrl: $("ollamaUrl").value.trim() || DEFAULTS.ollamaUrl,
      ollamaModel: $("ollamaModel").value.trim(),
      autoBubble: $("autoBubble").classList.contains("on"),
      bubblePersist: $("bubblePersist").classList.contains("on"),
      bubbleTimeout: $("bubblePersist").classList.contains("on")
        ? 0
        : Math.max(1, Math.min(30, parseInt($("bubbleTimeout").value, 10) || 8)),
      ctxTranslate: $("ctxTranslate").classList.contains("on"),
      ctxCollect: $("ctxCollect").classList.contains("on"),
      ttsRate: $("ttsRate").value,
      ttsPitch: $("ttsPitch").value,
      captureShortcut: $("captureShortcut").value,
      translateShortcut: $("translateShortcut").value
    };
  }

  function saveSettings() {
    const settings = collectSettings();
    chrome.storage.sync.set(settings, () => {
      if (chrome.runtime.lastError) {
        showToast("保存失败：" + chrome.runtime.lastError.message, "error");
        return;
      }
      // 通知 background 应用新设置
      chrome.runtime.sendMessage({ type: "settings-updated", settings }, () => {
        void chrome.runtime.lastError; // 忽略无接收方
      });
      showToast("设置已保存", "success");
    });
  }

  // ---- 一键应用推荐配置 ----
  function applyRecommended() {
    const rec = Object.assign({}, DEFAULTS);
    Object.keys(rec).forEach((k) => {
      if (k === "autoBubble" || k === "ctxTranslate" || k === "ctxCollect") rec[k] = true;
    });
    chrome.storage.sync.set(rec, () => {
      if (chrome.runtime.lastError) {
        showToast("应用推荐配置失败：" + chrome.runtime.lastError.message, "error");
        return;
      }
      chrome.runtime.sendMessage({ type: "settings-updated", settings: rec }, () => {
        void chrome.runtime.lastError;
      });
      loadSettings();
      showToast("已应用推荐配置", "success");
    });
  }

  // ---- 恢复默认 ----
  function resetSettings() {
    chrome.storage.sync.set(DEFAULTS, () => {
      if (chrome.runtime.lastError) {
        showToast("恢复默认失败：" + chrome.runtime.lastError.message, "error");
        return;
      }
      chrome.runtime.sendMessage({ type: "settings-updated", settings: DEFAULTS }, () => {
        void chrome.runtime.lastError;
      });
      loadSettings();
      showToast("已恢复默认设置", "success");
    });
  }

  // ---- 桌面端连接状态 ----
  function updateHostStatus(connected) {
    const pill = $("hostStatus");
    const text = $("hostStatusText");
    pill.classList.toggle("on", !!connected);
    pill.classList.toggle("off", !connected);
    text.textContent = connected ? "已连接" : "未连接";
  }

  // ---- 语音检测 ----
  let hasZhVoice = false;
  let hasEnVoice = false;
  let voicesLoaded = false;

  function detectVoices() {
    if (!('speechSynthesis' in window)) {
      voicesLoaded = false;
      return;
    }
    const voices = speechSynthesis.getVoices();
    hasZhVoice = voices.some(v => v.lang && v.lang.toLowerCase().startsWith('zh'));
    hasEnVoice = voices.some(v => v.lang && v.lang.toLowerCase().startsWith('en'));
    voicesLoaded = true;
    updateVoiceUI();
    // voices 可能异步加载
    if (speechSynthesis.onvoiceschanged !== undefined) {
      speechSynthesis.onvoiceschanged = () => {
        const v2 = speechSynthesis.getVoices();
        hasZhVoice = v2.some(v => v.lang && v.lang.toLowerCase().startsWith('zh'));
        hasEnVoice = v2.some(v => v.lang && v.lang.toLowerCase().startsWith('en'));
        updateVoiceUI();
      };
    }
  }

  function updateVoiceUI() {
    const text = $("voiceText");
    const warn = $("voiceWarn");
    const dot = document.querySelector("#voiceStatus .voice-dot");
    if (!voicesLoaded) {
      text.textContent = "检测中…";
      if (dot) dot.className = "voice-dot";
      return;
    }
    if (hasZhVoice && hasEnVoice) {
      text.textContent = "中英语音就绪 ✓";
      warn.style.display = "none";
      if (dot) dot.className = "voice-dot ok";
    } else if (hasEnVoice) {
      text.textContent = "仅英文语音可用";
      warn.style.display = "";
      if (dot) dot.className = "voice-dot warn";
    } else {
      text.textContent = "未检测到语音";
      warn.style.display = "";
      if (dot) dot.className = "voice-dot err";
    }
  }

  function testVoice(lang) {
    if (!('speechSynthesis' in window)) {
      alert('当前浏览器不支持 Web Speech API');
      return;
    }
    speechSynthesis.cancel();
    const u = new SpeechSynthesisUtterance(lang === 'zh' ? '你好，这是中文语音测试。' : 'Hello, this is an English voice test.');
    u.lang = lang === 'zh' ? 'zh-CN' : 'en-US';
    const voices = speechSynthesis.getVoices();
    const match = voices.find(v => v.lang && v.lang.toLowerCase().startsWith(lang));
    if (match) u.voice = match;
    u.rate = 0.9;
    speechSynthesis.speak(u);
  }

  // ---- 初始化 ----
  document.addEventListener("DOMContentLoaded", () => {
    loadSettings();
    detectVoices();
    chrome.runtime.sendMessage({ type: "get-status" }, (resp) => {
      if (chrome.runtime.lastError || !resp) {
        updateHostStatus(false);
        return;
      }
      updateHostStatus(resp.connected);
    });
  });

  $("save").addEventListener("click", saveSettings);
  $("reset").addEventListener("click", resetSettings);
  $("applyRecommended").addEventListener("click", applyRecommended);
  $("engine").addEventListener("change", updateEngineFields);
  $("testEnVoice").addEventListener("click", () => testVoice('en'));
  $("testZhVoice").addEventListener("click", () => testVoice('zh'));

  // ---- 快捷键：打开 chrome://extensions/shortcuts ----
  $("openShortcuts").addEventListener("click", () => {
    chrome.tabs.create({ url: "chrome://extensions/shortcuts" });
  });

  // 显示当前快捷键
  function showCurrentShortcuts() {
    if (chrome.commands && chrome.commands.getAll) {
      chrome.commands.getAll((commands) => {
        for (const cmd of commands) {
          if (cmd.name === "capture-translate") {
            $("captureShortcut").value = cmd.shortcut || "未设置";
          } else if (cmd.name === "cycle-engine") {
            $("translateShortcut").value = cmd.shortcut || "未设置";
          }
        }
      });
    }
  }
  showCurrentShortcuts();

  // 测试翻译引擎：用当前表单值实测一次
  $("testEngine").addEventListener("click", () => {
    const el = $("engineTestResult");
    const settings = collectSettings();
    el.textContent = "测试中…";
    el.style.color = "var(--muted)";
    chrome.runtime.sendMessage({
      type: "translate",
      src: "Hello, world. This is a test.",
      srcLang: settings.srcLang === "auto" ? "en" : settings.srcLang,
      tgtLang: settings.tgtLang,
      settings: settings
    }, (resp) => {
      if (chrome.runtime.lastError || !resp || !resp.ok) {
        const err = ((resp && resp.error) || chrome.runtime.lastError?.message || "无响应") || "";
        if (/downloadable|语言包.*下载|下载.*语言包/i.test(err)) {
          el.textContent = "语言包正在下载，首次使用需稍候…";
          el.style.color = "var(--warn)";
        } else {
          el.textContent = "失败：" + err;
          el.style.color = "var(--err)";
        }
        return;
      }
      el.textContent = "成功：" + resp.tgt + (resp.label ? "（引擎：" + resp.label + "）" : "");
      el.style.color = "var(--ok)";
    });
  });
})();
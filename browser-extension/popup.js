// WinOCR 划词助手 - Popup
// 显示连接状态；提供手动翻译与采集入口。

const $ = (id) => document.getElementById(id);

const statusDot = $("dot");
const statusText = $("statusText");
const srcText = $("srcText");
const srcLang = $("srcLang");
const tgtLang = $("tgtLang");
const translateBtn = $("translateBtn");
const result = $("result");
const resultText = $("resultText");
const copyResult = $("copyResult");
const hint = $("hint");
const collectWord = $("collectWord");
const collectBtn = $("collectBtn");
const collectHint = $("collectHint");
const engineSel = $("engine");
const wordBadge = $("wordBadge");
const wordList = $("wordList");

let currentTgt = "";
const DEFAULTS = { srcLang: "auto", tgtLang: "zh", engine: "host" };

// 上次采集结果（存 session，popup 关闭重开仍可见；只保留本次会话，避免长期占空间）
const LAST_COLLECT_KEY = "winocr_last_collect";

function setStatus(connected) {
  statusDot.className = "dot" + (connected ? " on" : " off");
  statusText.textContent = connected ? "已连接" : "未连接";
}

function showHint(el, text, isErr) {
  el.textContent = text;
  el.style.color = isErr ? "#ef4444" : "#94a3b8";
}

chrome.runtime.sendMessage({ type: "get-status" }, (resp) => {
  if (chrome.runtime.lastError || !resp) {
    setStatus(false);
    return;
  }
  setStatus(resp.connected);
});

// 读取设置，初始化语言下拉框
chrome.storage.sync.get(DEFAULTS, (items) => {
  if (chrome.runtime.lastError) return;
  if (items.srcLang) srcLang.value = items.srcLang;
  if (items.tgtLang) tgtLang.value = items.tgtLang;
  if (items.engine) engineSel.value = items.engine;
});

// 切换引擎：写回存储，立即对划词/OCR/快捷键翻译生效（与设置页、Alt+Shift+R 共用同一值）
engineSel.addEventListener("change", () => {
  chrome.storage.sync.set({ engine: engineSel.value });
});

// 打开设置页
document.getElementById("openOptions").addEventListener("click", (e) => {
  e.preventDefault();
  chrome.runtime.openOptionsPage();
});

// 生词本：新标签页打开只读镜像面板
document.getElementById("openWords").addEventListener("click", (e) => {
  e.preventDefault();
  chrome.tabs.create({ url: chrome.runtime.getURL("words.html") });
  window.close();
});

translateBtn.addEventListener("click", () => {
  const text = srcText.value.trim();
  if (!text) { showHint(hint, "请输入要翻译的文本", true); return; }
  translateBtn.disabled = true;
  translateBtn.textContent = "翻译中…";
  showHint(hint, "");
  chrome.runtime.sendMessage({
    type: "translate",
    src: text,
    srcLang: srcLang.value,
    tgtLang: tgtLang.value
  }, (resp) => {
    translateBtn.disabled = false;
    translateBtn.textContent = "翻译";
    if (chrome.runtime.lastError || !resp || !resp.ok) {
      showHint(hint, "翻译失败：" + ((resp && resp.error) || "桌面端未连接"), true);
      result.classList.remove("show");
      return;
    }
    currentTgt = resp.tgt;
    resultText.textContent = resp.tgt;
    result.classList.add("show");
    const engineLine = document.getElementById("engineLine");
    const engineName = document.getElementById("engineName");
    if (engineLine && engineName) {
      engineName.textContent = resp.label || resp.engine || "";
      engineLine.style.display = resp.label || resp.engine ? "block" : "none";
    }
    showHint(hint, "");
    // 方便采集：预填生词
    collectWord.value = text;
  });
});

copyResult.addEventListener("click", () => {
  if (currentTgt) {
    navigator.clipboard.writeText(currentTgt).then(() => {
      copyResult.textContent = "已复制 ✓";
      setTimeout(() => { copyResult.textContent = "复制"; }, 1500);
    });
  }
});

collectBtn.addEventListener("click", () => {
  const word = collectWord.value.trim() || srcText.value.trim();
  if (!word) { showHint(collectHint, "没有要采集的生词", true); return; }
  collectBtn.disabled = true;
  chrome.runtime.sendMessage({
    type: "collect",
    src: word,
    tgt: currentTgt || "",
    tags: [],
    srcLang: srcLang.value,
    tgtLang: tgtLang.value
  }, (resp) => {
    collectBtn.disabled = false;
    if (chrome.runtime.lastError || !resp || !resp.ok) {
      showHint(collectHint, "采集失败：" + ((resp && resp.error) || "桌面端未连接"), true);
      renderCollectedWords(null, resp && resp.cached);
      return;
    }
    if (resp.cached) {
      showHint(collectHint, "已缓存到本地 ✓（桌面端未运行，下次连接自动补传）");
    } else {
      showHint(collectHint, "已采集到生词本 ✓（在桌面端 WinOCR「采录」页查看）");
    }
    renderCollectedWords(resp.data, resp.cached);
  });
});

// ---------- 右上角：已采集英文单词 ----------
// host 端 ingest_text 只收纯 ASCII 英文（中文/数字/符号一律不收），这里展示实际入库的词条。
function renderCollectedWords(data, cached) {
  const items = (data && Array.isArray(data.items)) ? data.items : [];
  const count = (data && typeof data.count === "number") ? data.count : items.length;

  // 徽章
  wordBadge.className = "word-badge";
  if (!items.length) {
    wordBadge.classList.add("show", "empty");
    wordBadge.textContent = count > 0 ? count + " 词" : "无英文词";
    wordBadge.title = count > 0 ? "共 " + count + " 个词" : "这段文本里没有可采集的英文单词（只收英文）";
    wordList.className = "word-list";
    wordList.innerHTML = "";
  } else {
    wordBadge.classList.add("show");
    if (cached) wordBadge.classList.add("cached");
    wordBadge.textContent = (cached ? "已缓存 " : "") + count + " 词";
    wordBadge.title = "点击展开/收起已采集的英文单词";
    // 词条列表（最多展示 60 个，超出提示省略）
    const MAX = 60;
    const shown = items.slice(0, MAX).map((it) => {
      const w = String(it && it.word || "").replace(/[<>&"]/g, "");
      const lv = it && it.level ? String(it.level).replace(/[<>&"]/g, "") : "";
      return '<span class="word-chip">' + w + (lv ? '<span class="lv">' + lv + '</span>' : "") + '</span>';
    }).join("");
    wordList.innerHTML = shown + (items.length > MAX
      ? '<div class="word-more">… 另有 ' + (items.length - MAX) + ' 个词</div>'
      : "");
    wordList.className = "word-list show";
  }

  // 存本次结果，popup 重开时恢复
  try {
    if (items.length) {
      chrome.storage.local.set({ [LAST_COLLECT_KEY]: { items: items.slice(0, 200), count: count, cached: !!cached, ts: Date.now() } });
    } else {
      chrome.storage.local.remove(LAST_COLLECT_KEY);
    }
  } catch (_) {}
}

// 点击徽章展开/收起
wordBadge.addEventListener("click", () => {
  if (wordBadge.classList.contains("empty")) return;
  wordList.classList.toggle("show");
});

// popup 打开时恢复上次采集结果
try {
  chrome.storage.local.get({ [LAST_COLLECT_KEY]: null }, (r) => {
    const last = r && r[LAST_COLLECT_KEY];
    if (last && last.items && last.items.length) renderCollectedWords({ items: last.items, count: last.count }, last.cached);
  });
} catch (_) {}

// 实时刷新：从划词气泡 / OCR 气泡 / 右键菜单采集时（background 统一写入 storage），
// 若 popup 正开着，右上角徽章与词条列表立即更新
try {
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area !== "local" || !changes[LAST_COLLECT_KEY]) return;
    const v = changes[LAST_COLLECT_KEY].newValue;
    if (v && v.items && v.items.length) {
      renderCollectedWords({ items: v.items, count: v.count }, v.cached);
    }
  });
} catch (_) {}

// 支持 Ctrl+Enter 翻译
srcText.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
    e.preventDefault();
    translateBtn.click();
  }
});
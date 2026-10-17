// WinOCR 划词助手 - TTS (Web Speech API)
// 浏览器端朗读译文/原文。桌面端未连接时也可用（浏览器本地语音）。
// 说明：Service Worker 中 Web Speech API 不可用，此模块由 content script 或 popup 引入。

(() => {
  if (window.__winocrTtsLoaded) return;
  window.__winocrTtsLoaded = true;

  let voices = [];
  let currentUtterance = null;

  function loadVoices() {
    voices = speechSynthesis.getVoices();
  }
  loadVoices();
  if (speechSynthesis.onvoiceschanged !== undefined) {
    speechSynthesis.onvoiceschanged = loadVoices;
  }

  function pickVoice(lang) {
    const prefix = lang ? lang.split("-")[0] : "en";
    // 优先匹配语言前缀，其次中文/英文默认
    const candidates = voices.filter((v) => v.lang && v.lang.startsWith(prefix));
    if (candidates.length) return candidates[0];
    const zh = voices.find((v) => v.lang && v.lang.startsWith("zh"));
    if (zh) return zh;
    const en = voices.find((v) => v.lang && v.lang.startsWith("en"));
    if (en) return en;
    return voices[0] || null;
  }

  function speak(text, lang) {
    if (!("speechSynthesis" in window)) {
      console.warn("[winocr] Web Speech API 不可用");
      return false;
    }
    speechSynthesis.cancel();
    const u = new SpeechSynthesisUtterance(text);
    const v = pickVoice(lang);
    if (v) u.voice = v;
    u.lang = lang || (v && v.lang) || "en-US";
    u.rate = 1.0;
    u.pitch = 1.0;
    currentUtterance = u;
    speechSynthesis.speak(u);
    return true;
  }

  function stop() {
    if ("speechSynthesis" in window) speechSynthesis.cancel();
  }

  window.__winocrTts = { speak, stop, loadVoices };
})();
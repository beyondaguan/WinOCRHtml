// doc-translate.js — 文档翻译页（S7 / 3.13）：纯文本类（粘贴 / .txt / .md）分块翻译 + 双语对照 + 导出 .md
(function () {
  'use strict';
  const TGTS = ['zh', 'en', 'ja', 'ko', 'fr', 'de', 'es', 'ru', 'pt', 'it', 'vi', 'th', 'id', 'ar'];
  const TGT_NAMES = { zh: '中文', en: 'English', ja: '日本語', ko: '한국어', fr: 'Français', de: 'Deutsch', es: 'Español', ru: 'Русский', pt: 'Português', it: 'Italiano', vi: 'Tiếng Việt', th: 'ไทย', id: 'Indonesia', ar: 'العربية' };
  const SRCS = ['auto', 'zh', 'en', 'ja', 'ko', 'fr', 'de', 'es', 'ru'];
  const SRC_NAMES = { auto: '自动', zh: '中文', en: 'English', ja: '日本語', ko: '한국어', fr: 'Français', de: 'Deutsch', es: 'Español', ru: 'Русский' };

  const $ = (id) => document.getElementById(id);
  const tgtSel = $('tgt'), srcSel = $('src'), fileInput = $('file'), goBtn = $('go'), exportBtn = $('export'), statusEl = $('status'), srcArea = $('srcText'), outEl = $('out');

  TGTS.forEach((c) => { const o = document.createElement('option'); o.value = c; o.textContent = TGT_NAMES[c] || c; tgtSel.appendChild(o); });
  SRCS.forEach((c) => { if (c === 'auto') return; const o = document.createElement('option'); o.value = c; o.textContent = SRC_NAMES[c] || c; srcSel.appendChild(o); });
  tgtSel.value = 'zh';

  let lastResult = null;   // [{ src, dst }]

  // 按句边界分块（与扩展整页翻译同策略），避免单请求过长
  function chunk(text, maxLen) {
    maxLen = maxLen || 1200;
    const segs = String(text).split(/(?<=[。.!?！？\n])\s*|\n+/);
    const chunks = []; let buf = '';
    for (const seg of segs) {
      if (!seg) continue;
      if (buf && buf.length + seg.length > maxLen) { chunks.push(buf); buf = ''; }
      buf += seg;
    }
    if (buf) chunks.push(buf);
    return chunks.length ? chunks : [text];
  }

  async function doTranslate() {
    const text = (srcArea.value || '').trim();
    if (!text) { statusEl.textContent = '请先粘贴或载入文本'; statusEl.className = 'hint'; return; }
    const tgt = tgtSel.value, src = srcSel.value;
    goBtn.disabled = true; exportBtn.disabled = true; outEl.innerHTML = '';
    statusEl.textContent = '翻译中…'; statusEl.className = '';
    try {
      const s = await WINOCR.getSettings();
      const blocks = chunk(text);
      lastResult = [];
      for (let i = 0; i < blocks.length; i++) {
        statusEl.textContent = '翻译中… (' + (i + 1) + '/' + blocks.length + ')';
        const t = await WINOCR.translate(blocks[i], {
          engine: s.engine, sfKey: s.sfKey, sfUrl: s.sfUrl, sfModel: s.sfModel,
          mymemoryEmail: s.mymemoryEmail, ollamaUrl: s.ollamaUrl, ollamaModel: s.ollamaModel,
          srcLang: src, tgtLang: tgt
        });
        const dst = t || blocks[i];
        lastResult.push({ src: blocks[i], dst: dst });
        const h = document.createElement('h4'); h.textContent = '段落 ' + (i + 1);
        const p = document.createElement('div'); p.textContent = '原文：' + blocks[i] + '\n译文：' + dst;
        outEl.appendChild(h); outEl.appendChild(p);
      }
      statusEl.textContent = '完成，共 ' + lastResult.length + ' 段'; statusEl.className = 'ok';
      exportBtn.disabled = false;
    } catch (e) {
      statusEl.textContent = '失败：' + ((e && e.message) || e); statusEl.className = 'hint';
    } finally {
      goBtn.disabled = false;
    }
  }

  function doExport() {
    if (!lastResult) return;
    let md = '# 文档翻译\n\n';
    lastResult.forEach((r, i) => { md += '## 段落 ' + (i + 1) + '\n\n**原文**\n\n' + r.src + '\n\n**译文**\n\n' + r.dst + '\n\n'; });
    const blob = new Blob([md], { type: 'text/markdown;charset=utf-8' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob); a.download = 'winocr-doc-translate.md';
    document.body.appendChild(a); a.click(); a.remove();
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  }

  fileInput.addEventListener('change', () => {
    const f = fileInput.files && fileInput.files[0];
    if (!f) return;
    const reader = new FileReader();
    reader.onload = () => { srcArea.value = String(reader.result || ''); };
    reader.readAsText(f, 'utf-8');
  });
  goBtn.addEventListener('click', doTranslate);
  exportBtn.addEventListener('click', doExport);
})();

// js/modules/export.js — 导出服务（文件夹/ZIP/Obsidian/乐享）
// 所有翻译记录导出逻辑集中在此模块。

// ---------------- Markdown 生成工具 ----------------
function pad(n) { return String(n).padStart(2, '0'); }

function fmtDateTime(iso) {
  const d = new Date(iso);
  return pad(d.getMonth() + 1) + '-' + pad(d.getDate()) + ' ' + pad(d.getHours()) + ':' + pad(d.getMinutes());
}

function fmtDate(iso) {
  const d = new Date(iso);
  return d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate());
}

export function groupByDate(records) {
  const m = {};
  records.forEach((r) => { const k = fmtDate(r.ts); (m[k] = m[k] || []).push(r); });
  return m;
}

export function recordsToMarkdown(records) {
  let out = '';
  records.slice().sort((a, b) => (a.ts < b.ts ? -1 : 1)).forEach((r) => {
    out += '## ' + fmtDateTime(r.ts);
    if (r.source) out += ' · [' + (r.source.length > 40 ? r.source.slice(0, 40) + '…' : r.source) + '](' + r.source + ')';
    out += '\n\n';
    if (r.type === 'image' && r.image) out += '![' + (r.ocr || '截图') + '](' + r.image + ')\n\n';
    out += '**原文**: ' + (r.original || r.ocr || '') + '\n\n';
    out += '**译文**: ' + (r.translation || '') + '\n\n';
    if (r.terms && r.terms.length) out += '**术语**: ' + r.terms.map((t) => '[[' + t + ']]').join(' / ') + '\n\n';
  });
  return out;
}

export function buildDailyMarkdown(records) {
  const groups = groupByDate(records);
  let md = '';
  Object.keys(groups).sort().forEach((date) => {
    md += '---\n\n# WinOCR ' + date + '\n\n' + recordsToMarkdown(groups[date]);
  });
  return md;
}

// ---------------- 零依赖 ZIP（store，无压缩） ----------------
function crc32(buf) {
  let c = ~0;
  for (let i = 0; i < buf.length; i++) { c ^= buf[i]; for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xEDB88320 & -(c & 1)); }
  return ~c >>> 0;
}

export function makeZip(files) {
  const chunks = []; let offset = 0; const central = [];
  const enc = (s) => new TextEncoder().encode(s);
  files.forEach((f) => {
    const name = enc(f.name); const data = f.data; const crc = crc32(data); const size = data.length;
    const local = new Uint8Array(30 + name.length); const dv = new DataView(local.buffer);
    dv.setUint32(0, 0x04034b50, true); dv.setUint16(4, 20, true); dv.setUint16(6, 0, true);
    dv.setUint16(8, 0, true); dv.setUint16(10, 0, true); dv.setUint32(14, crc, true);
    dv.setUint32(18, size, true); dv.setUint32(22, size, true); dv.setUint16(26, name.length, true);
    dv.setUint16(28, 0, true); local.set(name, 30);
    chunks.push(local); chunks.push(data);
    const cen = new Uint8Array(46 + name.length); const cd = new DataView(cen.buffer);
    cd.setUint32(0, 0x02014b50, true); cd.setUint16(4, 20, true); cd.setUint16(6, 20, true);
    cd.setUint16(8, 0, true); cd.setUint16(10, 0, true); cd.setUint16(12, 0, true);
    cd.setUint32(16, crc, true); cd.setUint32(20, size, true); cd.setUint32(24, size, true);
    cd.setUint16(28, name.length, true); cd.setUint16(30, 0, true); cd.setUint16(32, 0, true);
    cd.setUint16(34, 0, true); cd.setUint16(36, 0, true); cd.setUint32(38, offset, true); cd.setUint32(42, 0, true);
    cen.set(name, 46); central.push(cen); offset += local.length + data.length;
  });
  const cdStart = offset; let cdSize = 0; central.forEach((c) => { chunks.push(c); cdSize += c.length; });
  const end = new Uint8Array(22); const ed = new DataView(end.buffer);
  ed.setUint32(0, 0x06054b50, true); ed.setUint16(8, files.length, true); ed.setUint16(10, files.length, true);
  ed.setUint32(12, cdSize, true); ed.setUint32(16, cdStart, true); ed.setUint16(20, 0, true);
  chunks.push(end);
  return new Blob(chunks, { type: 'application/zip' });
}

// ---------------- 导出：文件夹（File System Access） ----------------
export async function exportToFolder(records) {
  if (!globalThis.showDirectoryPicker) throw new Error('当前浏览器不支持 File System Access，请改用 ZIP 或升级 Edge/Chrome');
  const dir = await globalThis.showDirectoryPicker({ mode: 'readwrite' });
  const groups = groupByDate(records); let n = 0;
  for (const date of Object.keys(groups).sort()) {
    const f = await dir.getFileHandle('WinOCR-' + date + '.md', { create: true });
    const w = await f.createWritable();
    await w.write('# WinOCR ' + date + '\n\n' + recordsToMarkdown(groups[date]));
    await w.close(); n++;
  }
  return n;
}

// ---------------- 导出：ZIP 归档 ----------------
export async function exportToZip(records) {
  const groups = groupByDate(records); const files = [];
  for (const date of Object.keys(groups).sort()) {
    files.push({ name: 'WinOCR-' + date + '.md', data: new TextEncoder().encode('# WinOCR ' + date + '\n\n' + recordsToMarkdown(groups[date])) });
  }
  const blob = makeZip(files);
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = 'WinOCR-' + new Date().toISOString().slice(0, 10) + '.zip';
  document.body.appendChild(a); a.click(); a.remove();
  return files.length;
}

// ---------------- 导出：Obsidian Local REST API ----------------
export async function exportToObsidian(records, settings) {
  const base = (settings.obsidianUrl || 'http://127.0.0.1:27123').replace(/\/+$/, '');
  const key = settings.obsidianKey || '';
  const groups = groupByDate(records); let n = 0;
  for (const date of Object.keys(groups).sort()) {
    const md = '# WinOCR ' + date + '\n\n' + recordsToMarkdown(groups[date]);
    const r = await fetch(base + '/vault/WinOCR/' + ('WinOCR-' + date + '.md'), {
      method: 'PUT',
      headers: { 'Authorization': 'Bearer ' + key, 'Content-Type': 'text/markdown' },
      body: md
    });
    if (r.ok) n++; else throw new Error('Obsidian 返回 ' + r.status);
  }
  return n;
}

// ---------------- 导出：乐享 ----------------
export async function exportToLexiang(records, settings) {
  const ep = settings.lexiangEndpoint;
  if (!ep) throw new Error('请在设置中配置乐享 API 端点');
  const body = {
    title: 'WinOCR 导出 ' + new Date().toISOString().slice(0, 10),
    format: 'markdown',
    content: buildDailyMarkdown(records)
  };
  const r = await fetch(ep, {
    method: 'POST',
    headers: { 'Authorization': 'Bearer ' + (settings.lexiangToken || ''), 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
  if (!r.ok) throw new Error('乐享返回 ' + r.status);
  return true;
}

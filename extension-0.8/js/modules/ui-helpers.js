// js/modules/ui-helpers.js — UI 工具函数
// 所有 UI 相关辅助函数集中在此模块。

/**
 * HTML 转义，防止 XSS
 * @param {string} t - 原始文本
 * @returns {string} 转义后的安全 HTML 字符串
 */
export function escapeHtml(t) {
  return (t || '').replace(/[&<>"]/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;'
  }[c]));
}

/**
 * File 对象转 DataURL
 * @param {File} f - 文件对象
 * @returns {Promise<string>} DataURL 字符串
 */
export function fileToDataUrl(f) {
  return new Promise((res, rej) => {
    const r = new FileReader();
    r.onload = () => res(r.result);
    r.onerror = rej;
    r.readAsDataURL(f);
  });
}

/**
 * 显示 Toast 提示（自动消失）
 * @param {string} text - 提示文本
 * @param {boolean} isErr - 是否为错误样式
 * @param {number} durationMs - 显示时长（毫秒）
 */
export function showToast(text, isErr = false, durationMs = 2600) {
  let el = document.getElementById('toast');
  if (!el) {
    el = document.createElement('div');
    el.id = 'toast';
    el.style.cssText = 'position:fixed;bottom:20px;left:50%;transform:translateX(-50%);' +
      'background:#333;color:#fff;padding:10px 20px;border-radius:6px;font-size:14px;' +
      'opacity:0;transition:opacity .3s;z-index:99999;pointer-events:none;';
    document.body.appendChild(el);
  }
  el.textContent = text;
  el.style.background = isErr ? '#c0392b' : '#333';
  el.style.opacity = '1';
  clearTimeout(el.__timer);
  el.__timer = setTimeout(() => { el.style.opacity = '0'; }, durationMs);
}

/**
 * 设置加载状态
 * @param {HTMLElement} el - 目标元素
 * @param {string} text - 加载提示文本
 */
export function setLoading(el, text = '加载中...') {
  if (!el) return;
  el.classList.remove('is-empty');
  el.innerHTML = '<span class="ph">' + escapeHtml(text) + '</span>';
}

/**
 * 显示翻译结果
 * @param {HTMLElement} el - 目标元素
 * @param {string} original - 原文
 * @param {string} translation - 译文
 * @param {string} [err] - 错误信息
 */
export function showResult(el, original, translation, err) {
  if (!el) return;
  el.classList.remove('is-empty');
  if (err) {
    el.innerHTML = '<div class="err">' + escapeHtml(err) + '</div>';
    return;
  }
  el.innerHTML =
    '<div><div class="lab">原文</div><div class="o">' + escapeHtml(original) + '</div></div>' +
    '<div><div class="t">' + escapeHtml(translation) + '</div></div>';
}

/**
 * 格式化快捷键显示
 * @param {string} key - 快捷键字符串（如 "ctrl+shift+m"）
 * @returns {string} 格式化后的显示文本
 */
export function prettyKey(key) {
  if (!key) return '';
  return key.split('+').map((p) => {
    const map = { ctrl: 'Ctrl', alt: 'Alt', shift: 'Shift', meta: 'Meta' };
    const lower = p.trim().toLowerCase();
    return map[lower] || p.trim();
  }).join(' + ');
}

/**
 * 填充下拉选择框
 * @param {HTMLSelectElement} el - select 元素
 * @param {Array<{id: string, note?: string}>} models - 选项列表
 * @param {string} current - 当前选中值
 */
export function fillSelect(el, models, current) {
  if (!el) return;
  el.innerHTML = '';
  models.forEach((m) => {
    const o = document.createElement('option');
    o.value = m.id;
    o.textContent = m.note ? (m.id + ' — ' + m.note) : m.id;
    el.appendChild(o);
  });
  if (current && !models.some((m) => m.id === current)) {
    const o = document.createElement('option');
    o.value = current;
    o.textContent = current + ' — 自定义（不在免费清单内）';
    el.appendChild(o);
  }
  el.value = current || (models[0] && models[0].id) || '';
}

// WinOCR 生词本 —— 只读镜像面板（B 方案：桌面端为唯一真源）
// 通过 background 转发 list_collect 动作到 Native Messaging host。
(() => {
  "use strict";

  let allItems = [];

  const $ = (id) => document.getElementById(id);
  const listEl = $("list");
  const errBox = $("errBox");
  const countEl = $("count");

  function esc(s) {
    return String(s ?? "")
      .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  function fmtTs(ts) {
    if (!ts) return "";
    try {
      const d = new Date(Number(ts) < 1e12 ? Number(ts) * 1000 : Number(ts));
      if (isNaN(d.getTime())) return "";
      return d.toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" });
    } catch (_) { return ""; }
  }

  function showError(msg) {
    errBox.textContent = msg;
    errBox.classList.add("show");
  }

  function hideError() {
    errBox.classList.remove("show");
  }

  async function fetchItems() {
    hideError();
    listEl.innerHTML = '<div class="empty">加载中…</div>';
    try {
      const r = await chrome.runtime.sendMessage({ type: "list-collect", limit: 2000 });
      if (!r || !r.ok) {
        showError("拉取失败：" + (r && r.error ? r.error : "host 未连接或响应异常"));
        listEl.innerHTML = '<div class="empty">无法连接桌面端 WinOCR<div class="sub">请确认主程序 / winocr-host 正在运行</div></div>';
        return;
      }
      allItems = (r.data && r.data.items) || [];
      countEl.textContent = `共 ${allItems.length} 条（只读镜像）`;
      buildFilters();
      render();
    } catch (e) {
      showError("请求异常：" + (e && e.message || e));
      listEl.innerHTML = '<div class="empty">加载失败</div>';
    }
  }

  function buildFilters() {
    const levels = [...new Set(allItems.map((i) => i.level).filter(Boolean))].sort();
    const projs = [...new Set(allItems.map((i) => i.project).filter(Boolean))].sort();
    const fill = (sel, vals, label) => {
      const cur = sel.value;
      sel.innerHTML = `<option value="">${label}</option>` + vals.map((v) => `<option value="${esc(v)}">${esc(v)}</option>`).join("");
      if (vals.includes(cur)) sel.value = cur;
    };
    fill($("levelSel"), levels, "全部级别");
    fill($("projSel"), projs, "全部项目");
  }

  function render() {
    const q = $("q").value.trim().toLowerCase();
    const lv = $("levelSel").value;
    const pj = $("projSel").value;
    const items = allItems.filter((i) => {
      if (lv && i.level !== lv) return false;
      if (pj && i.project !== pj) return false;
      if (q) {
        const hay = [i.word, i.translation, i.context].map((s) => String(s || "").toLowerCase());
        if (!hay.some((h) => h.includes(q))) return false;
      }
      return true;
    });

    if (!items.length) {
      listEl.innerHTML = '<div class="empty">没有匹配的生词<div class="sub">划词 / 截图采集后会写入桌面端生词本</div></div>';
      return;
    }
    listEl.innerHTML = items.map((i) => `
      <div class="item">
        <div class="word">${esc(i.word)}${i.level ? `<span class="lv">${esc(i.level)}</span>` : ""}</div>
        <div class="mid">
          ${esc(i.translation || "—")}
          ${i.context ? `<div class="ctx" title="${esc(i.context)}">${esc(i.context.length > 90 ? i.context.slice(0, 90) + "…" : i.context)}</div>` : ""}
        </div>
        <div class="meta">
          <div class="proj">${esc(i.project || "")}</div>
          <div>${esc(fmtTs(i.ts))}</div>
        </div>
      </div>`).join("");
  }

  $("q").addEventListener("input", render);
  $("levelSel").addEventListener("change", render);
  $("projSel").addEventListener("change", render);
  $("refreshBtn").addEventListener("click", fetchItems);

  fetchItems();
})();

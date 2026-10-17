// ============================================================
// WinOCR 划词助手 · Offscreen Document 消息路由
//
// 在独立隐藏文档中运行本地 OCR（onnxruntime-web），避免
// MV3 service worker 被挂起导致模型丢失。background 通过
// chrome.runtime.sendMessage 与这里通信。
//
// 消息协议（background → offscreen）：
//   { type: 'ocr-init' }                    预加载模型
//   { type: 'ocr-translate', img: base64 }  识别截图文本
// ============================================================
(function () {
  'use strict';

  async function handleOcrInit() {
    await WinOCROCR.loadModels();
    return { ok: true, ready: WinOCROCR.isReady() };
  }

  async function handleOcrTranslate(imgBase64) {
    if (!imgBase64) throw new Error('截图数据为空');
    // base64 → Blob → ImageBitmap → ImageData
    const bin = atob(imgBase64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    const blob = new Blob([bytes], { type: 'image/jpeg' });
    const bitmap = await createImageBitmap(blob);
    try {
      const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
      const ctx = canvas.getContext('2d', { willReadFrequently: true });
      ctx.drawImage(bitmap, 0, 0);
      const imageData = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
      const text = await WinOCROCR.ocrImageData(imageData);
      return { ok: true, text: text || '' };
    } finally {
      bitmap.close();
    }
  }

  chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (!msg || typeof msg !== 'object') return;
    if (msg.type === 'ocr-init') {
      handleOcrInit().then(sendResponse).catch((e) => sendResponse({ ok: false, error: String(e && e.message || e) }));
      return true; // async
    }
    if (msg.type === 'ocr-translate') {
      handleOcrTranslate(msg.img)
        .then(sendResponse)
        .catch((e) => sendResponse({ ok: false, error: String(e && e.message || e) }));
      return true;
    }
  });

  // 启动时预加载模型
  WinOCROCR.loadModels().then(() => {
    console.log('[WinOCR OCR] 本地 OCR 引擎已就绪（offscreen）');
  }).catch((e) => {
    console.error('[WinOCR OCR] 本地 OCR 引擎加载失败：', e);
  });
})();
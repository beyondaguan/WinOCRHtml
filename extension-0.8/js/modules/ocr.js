// js/modules/ocr.js — OCR 引擎（云端 SF + PDF 提取）
// 所有 OCR 和图片/PDF 文本提取集中在此模块。

import { normalizeOcrModel } from './storage.js';
import { sfChat } from './translate.js';

// ---------------- PDF.js CDN 加载 ----------------
const PDFJS_CDN = 'https://cdnjs.cloudflare.com/ajax/libs/pdf.js/3.11.174/pdf.min.js';
const PDFJS_WORKER_CDN = 'https://cdnjs.cloudflare.com/ajax/libs/pdf.js/3.11.174/pdf.worker.min.js';
let pdfjsLibPromise = null;

function loadPdfJs() {
  if (pdfjsLibPromise) return pdfjsLibPromise;
  pdfjsLibPromise = new Promise((resolve, reject) => {
    if (typeof window !== 'undefined' && window.pdfjsLib) return resolve(window.pdfjsLib);
    const s = document.createElement('script');
    s.src = PDFJS_CDN;
    s.onload = () => {
      if (window.pdfjsLib) {
        window.pdfjsLib.GlobalWorkerOptions.workerSrc = PDFJS_WORKER_CDN;
        resolve(window.pdfjsLib);
      } else reject(new Error('PDF.js 加载失败'));
    };
    s.onerror = () => reject(new Error('无法加载 PDF.js（CDN 不可达，可能是离线环境）'));
    document.head.appendChild(s);
  });
  return pdfjsLibPromise;
}

// ---------------- PDF 文本提取 ----------------
export async function extractPdfText(file, onProgress) {
  const lib = await loadPdfJs();
  const buf = await file.arrayBuffer();
  const pdf = await lib.getDocument({ data: buf }).promise;
  const pages = [];
  for (let i = 1; i <= pdf.numPages; i++) {
    const page = await pdf.getPage(i);
    const content = await page.getTextContent();
    const text = content.items.map((it) => it.str).join(' ').trim();
    pages.push({ page: i, text: text });
    if (onProgress) onProgress(i, pdf.numPages);
  }
  return pages;
}

// ---------------- PDF 页面渲染为图片（供 OCR / 图像型 PDF） ----------------
export async function renderPdfPageToImage(file, pageNo, scale) {
  const lib = await loadPdfJs();
  const buf = await file.arrayBuffer();
  const pdf = await lib.getDocument({ data: buf }).promise;
  const page = await pdf.getPage(pageNo);
  const viewport = page.getViewport({ scale: scale || 2 });
  const canvas = document.createElement('canvas');
  canvas.width = viewport.width;
  canvas.height = viewport.height;
  const ctx = canvas.getContext('2d');
  await page.render({ canvasContext: ctx, viewport: viewport }).promise;
  return canvas.toDataURL('image/png');
}

// ---------------- SF 视觉模型 OCR ----------------
export async function ocrSF(dataUrl, opts) {
  opts = opts || {};
  const body = {
    model: normalizeOcrModel(opts.sfOcrModel),
    messages: [{
      role: 'user',
      content: [
        { type: 'text', text: 'OCR all text in this image. Output only the recognized text, preserving line breaks.' },
        { type: 'image_url', image_url: { url: dataUrl } }
      ]
    }],
    max_tokens: 4096
  };
  try {
    return await sfChat(opts, body);
  } catch (e) {
    if (/does not exist|not exist|20012/i.test(String(e && e.message))) {
      throw new Error('OCR 模型名不存在（' + normalizeOcrModel(opts.sfOcrModel) +
        '）。硅基流动上要写全称：PaddlePaddle/PaddleOCR-VL-1.5 或 deepseek-ai/DeepSeek-OCR（扩展「选项」里可改）');
    }
    throw e;
  }
}

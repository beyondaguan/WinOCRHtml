// ============================================================
// WinOCR 划词助手 · 本地 OCR 引擎（PP-OCRv6 tiny，onnxruntime-web）
//
// 完全在浏览器内运行，不经过桌面端主程序 / Native Messaging host。
// 模型资源：
//   models/ocr/det_tiny.onnx     文本检测（DBNet）
//   models/ocr/rec_tiny.onnx      文本识别（CTC）
//   models/ocr/ppocr_dict.txt    识别字典（6904 字符）
// 运行时：
//   vendor/ort.all.min.js        ONNX Runtime Web（UMD，全局 ort）
//   vendor/ort-wasm-simd-threaded.jsep.{mjs,wasm}
//
// 本文件在 Offscreen Document 中加载（offscreen.html），通过
// window.WinOCROCR 暴露接口，由 offscreen.js 做消息路由。
// ============================================================
(function () {
  'use strict';

  // ---------- 常量 ----------
  const DET_MODEL_URL = 'models/ocr/det_tiny.onnx';
  const REC_MODEL_URL = 'models/ocr/rec_tiny.onnx';
  const DICT_URL = 'models/ocr/ppocr_dict.txt';

  // PP-OCRv6 预处理/后处理参数（与官方 inference.yml 一致）
  const DET_LIMIT_SIDE = 960;      // 检测最长边限制
  const DET_THRESH = 0.2;          // DB 二值化阈值
  const DET_BOX_THRESH = 0.4;      // 框置信度阈值
  const DET_UNCLIP_RATIO = 1.4;    // 框扩张比例
  const DET_MAX_CANDIDATES = 1000; // 最多候选框
  const REC_HEIGHT = 48;           // 识别输入高度
  const REC_MEAN = [0.5, 0.5, 0.5];
  const REC_STD = [0.5, 0.5, 0.5];
  const DET_MEAN = [0.485, 0.456, 0.406];
  const DET_STD = [0.229, 0.224, 0.225];

  let ort = null;          // onnxruntime-web 全局对象
  let detSession = null;
  let recSession = null;
  let dict = null;         // string[]，index -> 字符
  let modelsLoaded = false;
  let loadingPromise = null;

  // ---------- 工具函数 ----------
  function loadScript(src) {
    return new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = src;
      s.onload = () => resolve();
      s.onerror = () => reject(new Error('加载 ' + src + ' 失败'));
      (document.head || document.documentElement).appendChild(s);
    });
  }

  function loadText(url) {
    return fetch(url).then((r) => {
      if (!r.ok) throw new Error('HTTP ' + r.status + ' ' + url);
      return r.text();
    });
  }

  // ---------- 初始化 ----------
  async function ensureOrt() {
    if (ort) return ort;
    if (typeof window.ort !== 'undefined') {
      ort = window.ort;
    } else {
      await loadScript('vendor/ort.all.min.js');
      if (typeof window.ort === 'undefined') {
        throw new Error('onnxruntime-web 加载失败：未找到全局 ort');
      }
      ort = window.ort;
    }
    // 指向扩展内 wasm 资源目录（相对 offscreen.html 的 URL）
    ort.env.wasm.wasmPaths = chrome.runtime.getURL('vendor/');
    ort.env.wasm.numThreads = Math.min(4, navigator.hardwareConcurrency || 4);
    return ort;
  }

  async function loadDict() {
    const text = await loadText(DICT_URL);
    // 字典每行一个字符；末尾可能有多余空行
    const lines = text.split(/\r?\n/);
    // PP-OCR 字典第一个字符是 blank 占位（实际为 '! 等符号'，模型输出 0=blank）
    // 字典行数 6904，模型输出 6906 = 6904 字符 + blank(0) + 特殊 token
    dict = lines.filter((l) => l.length > 0);
    // 补足到与模型输出类别数一致（如不足则补空）
    return dict;
  }

  async function loadModels() {
    if (modelsLoaded) return;
    if (loadingPromise) return loadingPromise;
    loadingPromise = (async () => {
      await ensureOrt();
      await loadDict();
      const [det, rec] = await Promise.all([
        ort.InferenceSession.create(DET_MODEL_URL, {
          executionProviders: ['wasm'],
          graphOptimizationLevel: 'all'
        }),
        ort.InferenceSession.create(REC_MODEL_URL, {
          executionProviders: ['wasm'],
          graphOptimizationLevel: 'all'
        })
      ]);
      detSession = det;
      recSession = rec;
      modelsLoaded = true;
      console.log('[WinOCR OCR] 本地 OCR 引擎就绪：det=' + det.inputNames.join(',') + ' rec=' + rec.inputNames.join(','));
    })();
    return loadingPromise;
  }

  // ---------- 图像预处理 ----------
  // 输入：ImageData（RGBA），输出 det 模型输入 tensor
  function preprocessDet(imageData) {
    const w = imageData.width;
    const h = imageData.height;
    // 保持长宽比缩放到最长边 <= DET_LIMIT_SIDE，且宽高为 32 的倍数
    let ratio = 1;
    let nw = w;
    let nh = h;
    if (Math.max(w, h) > DET_LIMIT_SIDE) {
      ratio = DET_LIMIT_SIDE / Math.max(w, h);
      nw = Math.round(w * ratio);
      nh = Math.round(h * ratio);
    }
    nw = Math.max(32, Math.round(nw / 32) * 32);
    nh = Math.max(32, Math.round(nh / 32) * 32);
    ratio = nw / w;

    // 用 canvas 缩放图像
    const canvas = document.createElement('canvas');
    canvas.width = nw;
    canvas.height = nh;
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    ctx.drawImage(imageData, 0, 0, w, h, 0, 0, nw, nh);
    const scaled = ctx.getImageData(0, 0, nw, nh);
    const px = scaled.data;

    // CHW float32，RGB，归一化 (x/255 - mean) / std
    const data = new Float32Array(3 * nh * nw);
    const area = nh * nw;
    for (let i = 0; i < area; i++) {
      const r = px[i * 4] / 255;
      const g = px[i * 4 + 1] / 255;
      const b = px[i * 4 + 2] / 255;
      data[i] = (r - DET_MEAN[0]) / DET_STD[0];
      data[area + i] = (g - DET_MEAN[1]) / DET_STD[1];
      data[2 * area + i] = (b - DET_MEAN[2]) / DET_STD[2];
    }
    return {
      tensor: new ort.Tensor('float32', data, [1, 3, nh, nw]),
      ratio, origW: w, origH: h, nw, nh
    };
  }

  // ---------- DB 后处理：概率图 → 文本框 ----------
  function dbPostProcess(probMap, meta) {
    // probMap: Float32Array H*W（sigmoid 后 0..1）
    const { nw, nh, ratio, origW, origH } = meta;
    // 二值化
    const bin = new Uint8Array(nw * nh);
    for (let i = 0; i < nw * nh; i++) {
      bin[i] = probMap[i] > DET_THRESH ? 1 : 0;
    }
    // 连通域标记（8 邻域，简单 flood fill）
    const visited = new Uint8Array(nw * nh);
    const boxes = [];
    const stack = [];
    const dirs = [[-1, -1], [-1, 0], [-1, 1], [0, -1], [0, 1], [1, -1], [1, 0], [1, 1]];
    for (let y = 0; y < nh; y++) {
      for (let x = 0; x < nw; x++) {
        const idx = y * nw + x;
        if (!bin[idx] || visited[idx]) continue;
        // flood fill
        let minX = x, maxX = x, minY = y, maxY = y, count = 0, sumProb = 0;
        stack.length = 0;
        stack.push(idx);
        visited[idx] = 1;
        while (stack.length) {
          const cur = stack.pop();
          const cx = cur % nw;
          const cy = Math.floor(cur / nw);
          count++;
          sumProb += probMap[cur];
          if (cx < minX) minX = cx;
          if (cx > maxX) maxX = cx;
          if (cy < minY) minY = cy;
          if (cy > maxY) maxY = cy;
          for (const [dx, dy] of dirs) {
            const nx = cx + dx;
            const ny = cy + dy;
            if (nx < 0 || ny < 0 || nx >= nw || ny >= nh) continue;
            const ni = ny * nw + nx;
            if (bin[ni] && !visited[ni]) {
              visited[ni] = 1;
              stack.push(ni);
            }
          }
        }
        const avgProb = count > 0 ? sumProb / count : 0;
        if (avgProb < DET_BOX_THRESH) continue;
        // 转为原图坐标（最小外接水平矩形）
        const x0 = Math.round(minX / ratio);
        const y0 = Math.round(minY / ratio);
        const w = Math.round((maxX - minX + 1) / ratio);
        const h = Math.round((maxY - minY + 1) / ratio);
        // 过滤太小的框
        if (w < 3 || h < 3) continue;
        // 限制在原图范围内
        const bx0 = Math.max(0, x0);
        const by0 = Math.max(0, y0);
        const bw = Math.min(origW - bx0, w);
        const bh = Math.min(origH - by0, h);
        if (bw < 3 || bh < 3) continue;
        boxes.push({ x: bx0, y: by0, w: bw, h: bh, score: avgProb });
      }
    }
    // 按 y 坐标排序（从上到下），同一行按 x 排序
    boxes.sort((a, b) => a.y - b.y || a.x - b.x);
    return boxes;
  }

  // ---------- 识别预处理：文本行图像 → rec 输入 tensor ----------
  function preprocessRecLine(imageData, box) {
    // 从原图裁剪文本行区域（ImageData 已在原图分辨率）
    const { x, y, w, h } = box;
    const canvas = document.createElement('canvas');
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    ctx.drawImage(imageData, x, y, w, h, 0, 0, w, h);
    const linePx = ctx.getImageData(0, 0, w, h);

    // 缩放：高度固定 REC_HEIGHT，宽度按比例，且为 8 的倍数（最小 8）
    let rw = Math.max(8, Math.round(w * REC_HEIGHT / h / 8) * 8);
    const rp = new Float32Array(3 * REC_HEIGHT * rw);
    // 双线性缩放（用 canvas）
    const rc = document.createElement('canvas');
    rc.width = rw;
    rc.height = REC_HEIGHT;
    const rctx = rc.getContext('2d', { willReadFrequently: true });
    rctx.drawImage(linePx, 0, 0, w, h, 0, 0, rw, REC_HEIGHT);
    const scaled = rctx.getImageData(0, 0, rw, REC_HEIGHT);
    const spx = scaled.data;
    const area = rw * REC_HEIGHT;
    for (let i = 0; i < area; i++) {
      const r = spx[i * 4] / 255;
      const g = spx[i * 4 + 1] / 255;
      const b = spx[i * 4 + 2] / 255;
      rp[i] = (r - REC_MEAN[0]) / REC_STD[0];
      rp[area + i] = (g - REC_MEAN[1]) / REC_STD[1];
      rp[2 * area + i] = (b - REC_MEAN[2]) / REC_STD[2];
    }
    return { tensor: new ort.Tensor('float32', rp, [1, 3, REC_HEIGHT, rw]) };
  }

  // ---------- CTC greedy decode ----------
  function ctcDecode(output, seqLen, dictArr) {
    // output: Float32Array seqLen * numClasses，取每步 argmax，去重 blank 和连续重复
    const numClasses = dictArr.length + 2; // 模型输出类别数（0=blank, 1..dictLen, last=特殊）
    let result = '';
    let prevIdx = -1;
    for (let t = 0; t < seqLen; t++) {
      let best = 0;
      let bestVal = -Infinity;
      const offset = t * numClasses;
      for (let c = 0; c < numClasses; c++) {
        const v = output[offset + c];
        if (v > bestVal) { bestVal = v; best = c; }
      }
      if (best === 0) { prevIdx = -1; continue; } // blank
      if (best === prevIdx) continue; // 连续重复
      prevIdx = best;
      const ch = dictArr[best - 1];
      if (ch) result += ch;
    }
    return result;
  }

  // ---------- 主入口：识别 ImageData ----------
  async function ocrImageData(imageData) {
    await loadModels();
    // 1. 检测
    const detMeta = preprocessDet(imageData);
    const detResult = await detSession.run({ [detSession.inputNames[0]]: detMeta.tensor });
    const detOutputName = detSession.outputNames[0];
    const prob = detResult[detOutputName].data; // Float32Array
    // 模型输出可能是 sigmoid 前 logit（有正有负）或 sigmoid 后概率（0~1）。
    // 检测值域：若有负值则做 sigmoid，否则直接用。
    let probMap;
    let hasNegative = false;
    for (let i = 0; i < prob.length; i++) {
      if (prob[i] < 0) { hasNegative = true; break; }
    }
    if (hasNegative) {
      probMap = new Float32Array(prob.length);
      for (let i = 0; i < prob.length; i++) {
        probMap[i] = 1 / (1 + Math.exp(-prob[i]));
      }
    } else {
      probMap = prob;
    }
    const boxes = dbPostProcess(probMap, detMeta);
    if (!boxes.length) return '';

    // 2. 逐行识别
    const lines = [];
    for (const box of boxes) {
      const recInput = preprocessRecLine(imageData, box);
      const recResult = await recSession.run({ [recSession.inputNames[0]]: recInput.tensor });
      const recOutput = recResult[recSession.outputNames[0]];
      const outData = recOutput.data;
      const outShape = recOutput.dims || recOutput.shape;
      // 输出 shape: [1, seq, classes] 或 [1, seq*classes] 或 [1,1,classes]
      let seqLen;
      if (outShape && outShape.length >= 2) {
        seqLen = outShape[outShape.length - 2] === 1 && outShape.length >= 3 ? outShape[outShape.length - 2] * outShape[outShape.length - 1] / (dict.length + 2) : outShape[outShape.length - 2];
        seqLen = Math.round(seqLen);
      } else {
        seqLen = Math.round(outData.length / (dict.length + 2));
      }
      const text = ctcDecode(outData, seqLen, dict);
      if (text) lines.push(text);
    }
    return lines.join('\n');
  }

  // ---------- 导出 ----------
  window.WinOCROCR = {
    loadModels,
    ocrImageData,
    isReady: () => modelsLoaded
  };
})();
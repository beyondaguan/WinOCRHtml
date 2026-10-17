// js/modules/study.js — 生词本 + SM-2 复习引擎
// 生词采集、复习队列、掌握度计算集中在此模块。

import { getVocabList, setVocabList, getHistory, DEFAULT_PROJECT } from './storage.js';

// ---------------- SM-2 算法 ----------------
export function sm2Calc(quality, easeFactor, interval, repetitions) {
  let newEF = easeFactor + (0.1 - (5 - quality) * (0.08 + (5 - quality) * 0.02));
  if (newEF < 1.3) newEF = 1.3;
  let newInterval;
  if (quality < 3) {
    newInterval = 1;
    repetitions = 0;
  } else {
    repetitions += 1;
    if (repetitions === 1) newInterval = 1;
    else if (repetitions === 2) newInterval = 6;
    else newInterval = Math.round(interval * newEF);
  }
  return {
    easeFactor: newEF,
    interval: newInterval,
    repetitions: repetitions,
    nextReview: Date.now() + newInterval * 24 * 60 * 60 * 1000
  };
}

// ---------------- 掌握度衰减 ----------------
export function decayMastery(mastery, daysSinceReview) {
  const decayRate = 0.05;
  const decayed = mastery - decayRate * daysSinceReview;
  return Math.max(0.1, decayed);
}

// ---------------- 生词 CRUD ----------------
export async function addVocab(word, translation, source, context) {
  const vocab = await getVocabList();
  const existing = vocab.find((v) => v.word === word);
  if (existing) {
    existing.reviewCount = (existing.reviewCount || 0) + 1;
    existing.lastSeen = Date.now();
    await setVocabList(vocab);
    return existing;
  }
  const item = {
    id: 'v' + Date.now() + '_' + Math.random().toString(36).slice(2, 8),
    word: word,
    translation: translation || '',
    source: source || '',
    context: context || '',
    project: DEFAULT_PROJECT,
    mastery: 0.3,
    easeFactor: 2.5,
    interval: 1,
    repetitions: 0,
    reviewCount: 0,
    nextReview: Date.now(),
    createdAt: Date.now(),
    lastSeen: Date.now()
  };
  vocab.push(item);
  await setVocabList(vocab);
  return item;
}

export async function removeVocab(id) {
  const vocab = await getVocabList();
  const filtered = vocab.filter((v) => v.id !== id);
  await setVocabList(filtered);
  return true;
}

// ---------------- 自动从翻译记录采集生词 ----------------
export async function autoCollectFromHistory(maxItems) {
  const hist = await getHistory();
  let count = 0;
  for (const rec of hist) {
    if (count >= (maxItems || 50)) break;
    const text = rec.original || '';
    if (!text) continue;
    const words = text.split(/[\s,;.!?;:'"()]+/).filter((w) => w.length >= 2 && /^[a-zA-Z\u4e00-\u9fa5]+$/.test(w));
    for (const word of words.slice(0, 10)) {
      await addVocab(word, rec.translation || '', rec.source || '', text);
      count++;
    }
  }
  return count;
}

// ---------------- 获取待复习列表 ----------------
export async function getReviewQueue(limit) {
  const vocab = await getVocabList();
  const now = Date.now();
  const updated = vocab.map((v) => {
    const daysSince = (now - (v.lastReview || v.createdAt)) / (24 * 60 * 60 * 1000);
    v.mastery = decayMastery(v.mastery, daysSince);
    return v;
  });
  await setVocabList(updated);
  return updated
    .filter((v) => v.nextReview <= now)
    .sort((a, b) => a.nextReview - b.nextReview)
    .slice(0, limit || 20);
}

// ---------------- 提交复习结果 ----------------
export async function submitReview(id, quality) {
  const vocab = await getVocabList();
  const item = vocab.find((v) => v.id === id);
  if (!item) return null;
  const calc = sm2Calc(quality, item.easeFactor, item.interval, item.repetitions);
  item.easeFactor = calc.easeFactor;
  item.interval = calc.interval;
  item.repetitions = calc.repetitions;
  item.nextReview = calc.nextReview;
  item.lastReview = Date.now();
  item.reviewCount = (item.reviewCount || 0) + 1;
  const masteryDelta = (quality - 3) * 0.15;
  item.mastery = Math.min(1, Math.max(0.1, item.mastery + masteryDelta));
  await setVocabList(vocab);
  return item;
}

// ---------------- 学习统计 ----------------
export async function getStudyStats() {
  const vocab = await getVocabList();
  const now = Date.now();
  const today = new Date().toISOString().slice(0, 10);
  const reviewedToday = vocab.filter((v) => {
    const d = new Date(v.lastReview || 0).toISOString().slice(0, 10);
    return d === today;
  });
  return {
    total: vocab.length,
    mastered: vocab.filter((v) => v.mastery >= 0.95).length,
    learning: vocab.filter((v) => v.mastery >= 0.3 && v.mastery < 0.95).length,
    newWords: vocab.filter((v) => v.mastery < 0.3).length,
    reviewedToday: reviewedToday.length,
    dueToday: vocab.filter((v) => {
      const d = new Date(v.nextReview).toISOString().slice(0, 10);
      return d === today;
    }).length,
    avgMastery: vocab.length ? (vocab.reduce((s, v) => s + v.mastery, 0) / vocab.length).toFixed(2) : 0
  };
}

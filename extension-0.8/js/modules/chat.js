// js/modules/chat.js — AI 对话管理
// 多轮对话创建/保存/删除/重命名/导出集中在此模块。

import { getChatListFull, setChatListFull } from './storage.js';

// ---------------- 对话 CRUD ----------------
export async function createChat(title, engine, model) {
  const list = await getChatListFull();
  const chat = {
    id: 'c' + Date.now() + '_' + Math.random().toString(36).slice(2, 8),
    title: title || '新对话',
    engine: engine || 'sf',
    model: model || '',
    messages: [],
    createdAt: Date.now(),
    updatedAt: Date.now()
  };
  list.push(chat);
  await setChatListFull(list);
  return chat;
}

export async function saveChat(id, messages, title, engine, model) {
  const list = await getChatListFull();
  const chat = list.find((c) => c.id === id);
  if (!chat) return false;
  chat.messages = messages;
  chat.updatedAt = Date.now();
  if (title) chat.title = title;
  if (engine) chat.engine = engine;
  if (model) chat.model = model;
  await setChatListFull(list);
  return true;
}

export async function deleteChat(id) {
  const list = await getChatListFull();
  await setChatListFull(list.filter((c) => c.id !== id));
  return true;
}

export async function renameChat(id, title) {
  const list = await getChatListFull();
  const chat = list.find((c) => c.id === id);
  if (!chat) return false;
  chat.title = title || chat.title;
  chat.updatedAt = Date.now();
  await setChatListFull(list);
  return true;
}

// ---------------- 对话导出 ----------------
export function exportChatToMarkdown(chat) {
  const lines = ['# ' + (chat.title || '未命名对话'), ''];
  lines.push('> 引擎：' + (chat.engine || 'sf') + ' · 模型：' + (chat.model || '未知'));
  lines.push('> 导出时间：' + new Date().toLocaleString(), '');
  (chat.messages || []).forEach((m) => {
    const role = m.role === 'user' ? '**你**' : m.role === 'assistant' ? '**AI**' : '**系统**';
    lines.push(role + '：', '', m.content || '', '', '---', '');
  });
  return lines.join('\n');
}

export function exportChatToJSON(chat) {
  return JSON.stringify({
    title: chat.title,
    engine: chat.engine,
    model: chat.model,
    exportedAt: new Date().toISOString(),
    messages: chat.messages
  }, null, 2);
}

export function exportChatToText(chat) {
  const lines = ['=== ' + (chat.title || '未命名对话') + ' ==='];
  lines.push('引擎：' + (chat.engine || 'sf') + ' · 模型：' + (chat.model || '未知'));
  lines.push('导出时间：' + new Date().toLocaleString(), '');
  (chat.messages || []).forEach((m) => {
    const role = m.role === 'user' ? '你' : m.role === 'assistant' ? 'AI' : '系统';
    lines.push('[' + role + '] ' + (m.content || ''), '');
  });
  return lines.join('\n');
}

// ---------------- 对话下载（浏览器端） ----------------
export function downloadChat(chat, format) {
  let content, mime, ext;
  if (format === 'json') {
    content = exportChatToJSON(chat);
    mime = 'application/json';
    ext = 'json';
  } else if (format === 'text') {
    content = exportChatToText(chat);
    mime = 'text/plain';
    ext = 'txt';
  } else {
    content = exportChatToMarkdown(chat);
    mime = 'text/markdown';
    ext = 'md';
  }
  const blob = new Blob([content], { type: mime + ';charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = (chat.title || 'chat').replace(/[\\/:*?"<>|]/g, '_') + '.' + ext;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// ---------------- 获取对话列表（通过 storage 模块的 getChatList） ----------------
export { getChatList, getChatListFull } from './storage.js';

// plugins.js — 插件系统占位页（S6 / 3.10，P2）。当前仅展示空状态与设计说明；真实加载器为后续迭代。
(function () {
  'use strict';
  // 占位：未来从 chrome.storage 读取已安装插件列表并渲染。
  // 钩子注册点（规划）：WINOCR.PluginHost.register(hookName, handler) 由宿主/Rust 侧 action 钩子驱动。
  try {
    if (WINOCR && !WINOCR.PluginHost) {
      WINOCR.PluginHost = {
        hooks: {},
        register: function (hook, fn) { (this.hooks[hook] = this.hooks[hook] || []).push(fn); },
        fire: function (hook, payload) {
          const list = this.hooks[hook] || [];
          let p = payload;
          for (const fn of list) { try { p = fn(p) || p; } catch (e) {} }
          return p;
        }
      };
    }
  } catch (e) {}
})();

# WinOCR 主窗口 · 高保真原型

页面数：2（含 P0 流程总览）· 主题：brand

## 速查
- 打开 index.html；顶部舞台条可切 桌面/平板/手机 与 4 套主题
- 跳转：`data-goto="页面id"`；返回：`data-back`
- 弹窗：`data-modal-open="#id"`；抽屉：`data-drawer-open="#id"`；底部面板：`data-sheet-open="#id"`；关闭：`data-close`
- 下拉菜单：`data-menu-open="#id"`；轻提示：`data-toast="文案"`
- 表单校验：`<form data-validate>` + `data-required / data-type / data-min`
- 数字滚动：`data-count-to="1280"`；进度：`data-progress="72"`；倒计时：`data-countdown="3600"`
- 图标：`data-icon="名称"`（内置 60+ 线性图标，清单见 references/component-spec.md）
- 完整组件示例见 skill 的 assets/example.html

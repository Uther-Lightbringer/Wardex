# 功能规格：主菜单、待办页与全局杂项

> 相关文档：[../architecture.md](../architecture.md) · [../design-principles.md](../design-principles.md) · [../implementation-plan.md](../implementation-plan.md) · [../ui-design.md](../ui-design.md) · [../assets.md](../assets.md) · [../data-formats.md](../data-formats.md) · [chat.md](./chat.md) · [sessions-and-config.md](./sessions-and-config.md)

本文档定义主菜单页、待办页、打开项目对话框及全局机制的功能行为。读者是执行实现的 AI 模型；界面文本为纯中文。旧代码参照均为 `C:/workspace/WarDex` 下的 `文件:行号`。

## 1. 页面状态机（全局导航）

参照 `ui/Main.qml`（1092 行）。

- 五个页面状态：`main | config | sessionSelect | chat | todo`，同一时间只显示一页。
- 窗口固定有左右两条"铁轨"边框（`frame_edge_left/right.png`），页面内容区嵌入铁轨之间（见 [../ui-design.md](../ui-design.md)）。
- 页面切换动画：内容壳三段式下落入场（`ShellFrame.qml`，总时长 **750ms**：prepareEnter → playEnterDrop → snapContentIn）。
- 音画时序：切页播放 `popUp`/`popDown` 音效；**`popUp`（可闻 1280ms）未放完时不允许开始下拉动画**，队列等待（Main.qml 的 popUpGapMs 逻辑，必须保留）。
- 页面用缓存 Loader 预热（新版：Vue 组件 `<keep-alive>` 等价）。
- 切页期间 `uiGate.busy = true`，禁用所有按钮输入（全局输入闸门，见 §6）。

## 2. 主菜单页（main）

参照 `ui/Main.qml`、`ui/SteelPanel.qml`、`ui/RecentProjectsPanel.qml`（201 行）。

- 视觉：铆钉钢板面板（frame_tall/frame_short + `chain_link.png` 垂直平铺吊链，链条可伸出窗口顶），内衬半透明玻璃 `#A60b0d12`。
- 中央菜单按钮（WarButton，标准宽 276，中文文案）：打开项目 / 最近项目 / 配置 / 待办 等（以旧版实际按钮为准）。
- 左栏**最近项目面板**（`frame_popup_small.png` 九宫格）：
  - 数据源 `projects.json`（recent 上限 **8**，新者在前；见 [../data-formats.md](../data-formats.md) §6）。
  - 点击项目 → 以该目录为工作区创建新会话并进入聊天页。
  - 右键菜单：重命名别名 / 移除。别名存独立 `aliases` map，掉出最近列表不丢。
  - 行高亮用 `GlueScreen-Button-KeyboardHighlight.png`，`mix-blend-mode: screen`，透明度 0.55。

## 3. 打开项目对话框（FolderBrowserDialog）

参照 `ui/FolderBrowserDialog.qml`（428 行）。

- WC3 风格模态框（WarDialog 外观），组成：盘符下拉（WarDropdown）+ 当前路径显示 + 文件夹列表（`icon-folder.png`/`icon-folder-up.png`）+ 底部按钮区。
- 功能：双击/回车进入子目录；上级目录项；**内联新建文件夹**（行内编辑命名）；键盘导航（上下选择、回车进入、退格上级）。
- 仅展示目录（FolderBrowserModel 纯目录模型 + 盘符列表）。
- 确认后：写入 `projects.json` recent（cleanPath 绝对路径、大小写不敏感去重），以该目录创建会话进入聊天页。

## 4. 待办页（todo）

参照 `ui/TodoPage.qml`（314 行）、`src/TodoStore.h/.cpp`；新版统一待办/提醒模型见 [../data-formats.md](../data-formats.md) §8.1 与 [chat.md](chat.md) §6.6。

- 全量看板，持久化 `todos.json`。**待办区按范围分组**：全局 → 项目级（按项目目录名分组）→ 会话级（按会话名分组）；已完成区在下方（划线 + 灰化）。
- 操作：新增（输入框回车，可选手范围：全局/项目/会话——无当前会话/项目时对应范围禁用；可勾选"到期"并填分钟数）、勾选完成/取消完成、删除单条、**清除已完成**。到期待办显示 `已到期`（红色），到点弹系统通知 + 应用内弹窗（见 chat.md §6.6），勾掉才算完成。
- 与聊天/会话数据仅通过会话 id/项目目录关联，不含任何 ACP 交互。

## 5. 背景与启动参数

- 背景系统见 [../assets.md](../assets.md) §外部背景配置约定 与 [../ui-design.md](../ui-design.md) §背景系统：`background.json`（image|model）、缺省 `LodolonFall.jpg`、gif/webp 动图。model 背景（Three.js）可后置。
- 启动参数（开发自测用，新版酌情保留）：
  - `--page <main|config|sessionSelect|chat|todo>`：启动直达页面（旧版注入 `startPage`，main.cpp:138-158）。
  - `--geometry WxH`：调窗口尺寸（main.cpp:160-245）。
  - `--shot <path>`：1.5s 预热后抓帧保存退出（截图自测模式）。
  - `--perf` 或数据目录 `debug.flag`：开启性能 HUD（内存/CPU），新版可选。

## 6. 全局机制

- **uiGate 输入闸门**：页面切换/模态期间全局禁用按钮，防连点穿透（src/UiGate.h；新版：Pinia 全局 `uiBusy` + 控件统一绑定）。
- **字号缩放**：`userPrefs.fontScale` 0.85~1.30，全局生效（聊天气泡、列表、输入区），持久化 `user_prefs.json`（[../data-formats.md](../data-formats.md) §7）。
- **用户头像/昵称**：user_prefs 配置，聊天气泡用户侧使用；头像 128×128 中心裁方落盘 `user_avatar.png`。
- **光标**：全局 `cursor.png`，可交互元素 `cursor_green.png`（[../ui-design.md](../ui-design.md) §光标）。
- **音效**：click/popUp/popDown 三事件全局限流播放（[../ui-design.md](../ui-design.md) §音效）。
- **内存压力横幅**：旧版 PerfMonitor 内存看门狗超阈值锁存 `memoryPressure`（带回滞），聊天页顶部显示横幅；新版可选实现（Rust 侧读进程内存 + Tauri 事件）。

## 7. 崩溃与日志（开发期保障）

- 旧版 `AppLog`（`logs/wardex-*.log` 分阶段启动计时打点）+ `CrashHandler`（`crashes/` minidump）。
- 新版对应：Rust 侧 `log` crate 写数据目录 `logs/`，启动分阶段计时；panic hook 写 `crashes/crash-*.txt`。优先级低，但调试三类 CLI 兼容性时**日志是刚需**，建议阶段 1 就带上（见 [../implementation-plan.md](../implementation-plan.md)）。

## 实现检查清单

- [ ] 五页状态机 + 750ms 三段下落动画 + popUp/popDown 音画时序（1280ms 闸门）
- [ ] 主菜单钢板 + 吊链平铺 + 玻璃内衬 + 菜单按钮导航正确
- [ ] 最近项目：≤8、点击开新会话、右键别名/移除、aliases 持久化
- [ ] 打开项目对话框：盘符下拉、进入/上级、内联新建文件夹、键盘导航
- [ ] 待办页：两区、划线灰化、清除已完成、todos.json 持久化
- [ ] uiGate 切页期间全局禁输入
- [ ] fontScale 0.85~1.30 全局生效并持久化
- [ ] --page/--geometry 启动参数（开发自测）
- [ ] 日志：启动分阶段计时写 logs/，panic hook 写 crashes/
- [ ] （可选）--perf HUD 与内存压力横幅

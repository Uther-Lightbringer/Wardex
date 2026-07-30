# WarDex (Tauri 重写版) 设计文档集

本目录是 **WarDex 从 C++/Qt6 到 Tauri 2 (Rust + Vue 3) 重写**的完整设计文档，供执行实现的 AI 模型（或人类开发者）使用：**无需任何对话上下文，按本文档集即可独立完成实现**。

## 项目一句话

WarDex 是一个 Windows 桌面端 AI 编程助手 GUI：魔兽争霸 III 风格自绘界面，通过 ACP（Agent Client Protocol，stdio JSON-RPC）连接 `kimi acp` / `claude-code-acp` / `codex-acp` 等 Agent CLI，提供多会话聊天、权限确认、图片发送等功能。界面文本为纯中文。

## 重写动机

旧版存在系统性性能/内存问题（逐分片 O(N²) 全量重建、消息 2~3 倍重复存储、会话无淘汰，详见旧仓库 `memory-audit-streaming.md`）。重写按新架构从设计上规避这些问题，**同时保持磁盘数据格式与 ACP 协议行为完全兼容**。

## 阅读顺序

| 顺序 | 文档 | 内容 |
|---|---|---|
| 1 | [architecture.md](./architecture.md) | 整体架构、模块划分、前后端通信模型 |
| 2 | [design-principles.md](./design-principles.md) | 设计原则与红线（性能/兼容/代码约定） |
| 3 | [implementation-plan.md](./implementation-plan.md) | 分阶段实施计划与验收标准 |
| 4 | [acp-protocol.md](./acp-protocol.md) | ACP 协议客户端完整规格（后端核心） |
| 5 | [providers-and-cli.md](./providers-and-cli.md) | 四类 provider 差异、Agent 配置、CLI 探测 |
| 6 | [data-formats.md](./data-formats.md) | 磁盘数据格式逐字段规格（兼容性命门） |
| 7 | [features/chat.md](./features/chat.md) | 聊天页完整功能规格 |
| 8 | [features/sessions-and-config.md](./features/sessions-and-config.md) | 会话选择页 + Agent 配置页 |
| 9 | [features/main-menu-and-misc.md](./features/main-menu-and-misc.md) | 主菜单、待办页、全局杂项 |
| 10 | [ui-design.md](./ui-design.md) | WC3 视觉体系（九宫格参数/动画/光标/音效） |
| 11 | [assets.md](./assets.md) | 53 个素材文件清单与用法 |
| 12 | [performance.md](./performance.md) | 性能预算与稳定性设计（缓存三要素/预算表/五道防线） |
| 13 | [panels.md](./panels.md) | 信息面板坞与铁框视觉语言（可扩展右栏/拖拽调高/布局记忆） |

仓库根目录另有 [../PLAN.md](../PLAN.md)（最初批准的迁移计划，已被本目录细化，冲突时以本目录为准）。

## 旧代码参照约定

- 旧项目（只读参照，**绝不要修改**）：`C:/workspace/WarDex`
- 文档中 `文件:行号` 引用均指旧仓库路径，如 `src/AcpClient.cpp:208-220`
- 新项目：`C:/workspace/Wardex-rust`（即本仓库），骨架已就位：
  - `src-tauri/src/acp/`、`src-tauri/src/chat/`、`src-tauri/src/store/` —— Rust 后端模块（头部注释已写明迁移要点）
  - `src/pages/`、`src/components/war/`、`src/stores/` —— Vue 前端目录
  - `public/assets/` —— 53 个素材文件（路径与旧版 `assets/` 一致）

## 文档维护约定

- 实现过程中发现文档与代码行为不一致时，**以让二者一致为准**：改代码或改文档，并说明原因。
- 每份文档结尾的"实现检查清单"供执行时逐项打勾；完成一项勾一项（`[x]`）。
- 新增功能/素材/配置项时，同步更新对应文档。

# 整体架构

> 相关文档：[design-principles.md](./design-principles.md) · [implementation-plan.md](./implementation-plan.md) · [acp-protocol.md](./acp-protocol.md) · [data-formats.md](./data-formats.md) · [features/chat.md](./features/chat.md) · [performance.md](./performance.md) · [panels.md](./panels.md)

## 1. 技术栈与形态

- **Tauri 2**：Rust 后端 + WebView2 前端，Windows only
- **前端**：Vue 3 + TypeScript + Vite + Pinia（无 UI 框架，全部控件自绘，见 [ui-design.md](./ui-design.md)）
- **后端**：Rust（tokio 异步），无第三方 ACP 库，协议层手写（见 [acp-protocol.md](./acp-protocol.md)）
- 窗口 1280×720（最小 960×600），标题 WarDex
- 分发：NSIS 安装器 + 便携 zip（替代旧版 Qt 运行时 zip，体积小一个数量级）

## 2. 目录结构

```
Wardex-rust/
├── docs/                  # 本文档集
├── public/assets/         # 53 个运行时素材（见 assets.md）
├── src/                   # Vue 前端
│   ├── pages/             # MainMenu / Config / SessionSelect / Chat / Todo（五页状态机）
│   ├── components/war/    # WC3 自绘控件库（WarFrame/WarButton/WarDialog/...）
│   ├── stores/            # Pinia：session 列表、消息渲染状态、agent 配置、uiGate 等
│   └── lib/               # IPC 封装、音效、光标、工具函数
└── src-tauri/src/
    ├── acp/               # ACP 协议客户端（spawn、JSON-RPC、反向 RPC）
    ├── chat/              # 会话 runtime：流式合并、重试、续写、进程管理、subagent 跟踪
    ├── store/             # 持久化：sessions/agents/projects/prefs/todos/prompts/media
    ├── inspect/           # 信息面板数据源：git.rs / files.rs / 将来 db.rs ...（见 panels.md）
    ├── probe.rs           # CLI 探测
    └── lib.rs             # Tauri 命令注册、事件转发、启动流程
```

## 3. 前后端职责划分（核心原则）

**前端只维护渲染状态，消息/会话/Agent 配置的权威数据全部在 Rust 侧。** 这是与旧版最大的架构差异：旧版 QML 通过 QVariantList 每 50ms 深拷贝全量 segments 进 JS 引擎（性能灾难 P1 的根源）；新版前端从不持有全量消息的"权威副本"。

- **命令（前端 → Rust）**：`invoke()`，如 `create_session`、`send_prompt`、`answer_permission`、`list_sessions`、`save_agent`、`search_messages`、`read_file_range` 等。
- **事件（Rust → 前端）**：`emit()` 推送，前端订阅：
  - `acp://chunk`（流式文本/thinking 增量，50ms 合并节奏）
  - `acp://tool`（tool_call / tool_call_update 归一化后的结构）
  - `acp://turn`（回合状态：done/error/interrupted、stopReason）
  - `acp://permission`（权限请求，等待 `answer_permission` 应答）
  - `acp://retry`（限流重试倒计时）、`acp://subagent`（子代理状态）
  - `store://sessions`（会话列表变更）等

## 4. 后端模块关系

```
lib.rs (Tauri commands/events)
  ├── chat::ChatManager ── 管理 HashMap<sessionId, Runtime>
  │     ├── Runtime: acp::AcpClient + 合并定时器 + 队列 + 重试/续写状态
  │     └── store::SessionStore（消息落盘/读取、LRU 驻留）
  ├── store::* ── AgentStore / ProjectStore / UserPrefs / TodoStore / PromptStore
  ├── probe::CliProbe
  └── acp::AcpClient ── 每 runtime 一个子进程，NDJSON stdio
```

关键行为（细节见各专题文档）：

- **流式合并**：chunk 先入 pending 缓冲，50ms 单发定时器合并后经事件推前端；积压 >64KB 升 250ms。
- **进程上限**：`kMaxParallelAcp = 3`，超限停最久未活动的 idle 进程（全 busy 允许临时超限）；会话恢复靠 `session/load`。
- **会话驻留**：消息模型 LRU 淘汰（旧版无淘汰是内存暴涨 P3 根源），磁盘 JSONL 为唯一持久真相。
- **限流重试**：429/quota 检测，20→40→80s backoff 上限 300s，重试 3 次；依赖 ACP 层错误事件顺序（见 [acp-protocol.md](./acp-protocol.md) §7）。
- **断线续写**：进程崩溃且有部分输出时合成续写 prompt（附尾部 500 字符）接同一气泡。

## 5. 数据流示例（发送一条消息）

1. 前端 `invoke('send_prompt', {sessionId, text, attachments})`
2. ChatManager：busy 则入队（上限 10；含附件不可入队），否则走 3
3. 用户消息落盘（messages.jsonl 追加）→ 事件通知前端插入气泡
4. AcpClient `session/prompt` → CLI；流式 chunk 经合并 → `acp://chunk` → 前端**增量 append** 到气泡末尾文本节点
5. tool_call 事件 → `acp://tool` → 前端插入 tool 段
6. 权限请求 → `acp://permission` → 前端弹框 → `invoke('answer_permission')` → ACP 层回 result
7. 回合结束 → `rewriteMessagesFile` 全量重写该会话 → 前端收到 `acp://turn` 后将气泡文本段一次性 markdown 渲染

## 6. 内存设计（对应旧版 P1~P4，详见 design-principles.md）

| 旧版问题 | 新版设计 |
|---|---|
| P1 每 flush O(N²) 全量重建 | 前端增量 DOM append；markdown 回合结束渲染一次 |
| P2 消息 2~3 倍重复存储 | segments 单一数据源；运行期 buffer 只留尾部 |
| P3 会话/runtime 无淘汰 | 消息模型 LRU + ACP 进程上限 3 |
| P4 工具 payload 全量 + O(n²) 解析 | 内存截断 64KB（全文只落盘）；只解析最后一个累积块 |

## 7. 兼容性边界

- **磁盘数据**：逐字段兼容旧版 `%AppData%/WarDex`（[data-formats.md](./data-formats.md)）；开发期用 `WarDex-tauri-dev` 隔离，发布版复用 `WarDex`，用户历史零迁移。
- **ACP 行为**：三类 CLI（kimi/claude-code/codex-acp）差异收敛在 provider registry（[providers-and-cli.md](./providers-and-cli.md)）；Windows 特例（.cmd shim、env 删除语义等）逐条保留。
- **素材与视觉**：贴图/音效直接复用，九宫格参数从 QML 搬运到 CSS border-image（[ui-design.md](./ui-design.md)）。

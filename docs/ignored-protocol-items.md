# WarDex ACP 协议忽略项清单

> **实施状态（2026-08 更新）**：本文档中除 `terminal/*`、附件入队、非图片附件
> 降级外的项目**已全部实现**（见各项 ✅ 标注）。文档保留作为实现记录与剩余项
> 清单。当前 Rust 侧的分派表现见 `docs/acp-protocol.md` 第 6 节的最新表格。

整理自 `docs/acp-protocol.md`（旧 C++ 实现 `AcpClient.cpp` 的行为记录）。
当前实现的共同原则：只有聊天主链路（消息 chunk、tool call、权限请求）被接线，
其余协议面一律丢弃或容错跳过——即"UI/功能没做，协议层就不接"的最小实现策略。
本文档按接入优先级分类列出全部忽略项，作为后续实现的接入点清单。

---

## 1. `session/update` 通知层被忽略的 kind

分派点：`AcpClient.cpp:516`（见 `acp-protocol.md` 第 6 节）。

| kind | 含义 | 现状 | 实现建议 |
|---|---|---|---|
| `available_commands_update` | agent 动态下发的斜杠命令列表（`availableCommands: [{name, description, input?}]`） | ✅ 已实现 | 存 store + `acp://commands` 事件；Composer 敲 `/` 弹补全菜单，选中作为普通文本发送（`src/components/chat/Composer.vue`） |
| `plan` | agent 的任务计划更新（plan entries，带状态） | ✅ 已实现 | 构造 `{toolCallId:"plan"}` 段 upsert 到最后 assistant 行；前端渲染计划卡片（`src/components/chat/ChatBubble.vue`），不进过程行 |
| `config_option_update` | 会话配置项变更（mode 之外的 picker 值变化） | ✅ 已实现（早于本文档） | 修补 config_options 发 `AcpEvent::ConfigOptions` |
| `usage_update` | 回合外 token 用量 | ✅ 已实现 | `TurnUsage` 容错解析；prompt result 无 usage 时回退使用（archive 补源之前） |
| `session_info_update` | 会话元信息（title） | ✅ 已实现 | 非空 title → `rename_session` + `store://sessions` |
| `current_mode_update` | mode 变更 | ✅ 已实现 | 发 ModeChanged；chat 层修补 mode picker 并重发 configOptions |
| `user_message_chunk` | 回放的用户消息 | ✅ 已实现（日志） | 只在回放有意义（回放已丢）；非回放期记日志忽略 |
| 其他未知 kind | 兜底丢弃 | ✅ 已加日志 | `log::info!` 记录 kind 名 + 键集，便于观察线上 agent 行为 |

已接线的对照：`agent_thought_chunk`、`agent_message_chunk`、`tool_call`、`tool_call_update`。

## 2. session/load 回放丢弃

- 位置：`AcpClient.cpp:489-491`（`acp-protocol.md:190-192, 392`）。
- 行为：`replaying == true`（session/load 在途）期间收到的 `session/update`
  直接 return——**`available_commands_update` 除外**：它是状态非历史，回放期
  只存不发，session_ready 时补发（✅ 已实现）。
- 原因：WarDex 自己持久化了聊天历史，回放只为恢复 agent 侧上下文。
- plan 仍按历史处理随回放丢弃（它是历史回合的产物，与消息 chunk 同类）。

## 3. 反向请求与通知层

| 项 | 位置 | 行为 | 实现建议 |
|---|---|---|---|
| 未知方法通知（无 id） | `AcpClient.cpp:463-465`（`acp-protocol.md:59, 380`） | 仍忽略 | ✅ 已补 `log::info!`（method 名） |
| 未知方法反向请求（有 id） | 同上 | 回 `-32601 "Method not found: <method>"` | ✅ 保留行为并补了 `log::info!` |
| `terminal/*` | initialize 声明 `terminal: false` | 收到走 -32601（已加日志） | ❌ 未实现：若要支持 agent 内嵌终端，需声明能力并实现 `terminal/*` 命名空间 |
| `fs/read_many_files` 等其他 fs 方法 | 只实现了 `fs/read_text_file`、`fs/write_text_file`（`acp-protocol.md` 5.x） | -32601 | 视 agent 需求增补 |
| 匹配不上在途 id 的响应 | `AcpClient.cpp:411`（`acp-protocol.md:66`） | 仍忽略 | ✅ 已改 `log::warn!`（agent 乱序/重复应答可诊断） |
| stdout 非 JSON 行 | `AcpClient.cpp:290-300, 309-318`（`acp-protocol.md:42-45`） | 记日志丢行，不断连 | 现状合理（容忍 banner/日志噪声），保留 |
| `authenticate` | initialize 的 `authMethods` | ✅ 已实现 | prompt 被 `-32002` 拒绝时用第一个 authMethod 自动 `authenticate` 一次并重发原 prompt；仍失败走原错误路径 |

## 4. 请求/响应内容层

| 项 | 位置 | 行为 | 实现建议 |
|---|---|---|---|
| `session/set_config_option` 响应内容 | `AcpClient.cpp:398-402`（`acp-protocol.md:298`） | ✅ 已实现：result 的 `configOptions` 会读取并发 `AcpEvent::ConfigOptions` | 已完成 |
| `session/cancel` 语义 | `acp-protocol.md:277-279` | 发出通知后不保证 agent 服从 | 已有 2500ms 强杀兜底（第 9.6 节），现状可保留 |
| `initialize` 响应的能力字段 | `acp-protocol.md:472-473` | 只读了 `loadSession`、`promptCapabilities.image`（+ `authMethods`） | 其余能力位（`mcpCapabilities`、promptCapabilities 其他媒体类型）按需透传 |

## 5. 消息与队列层

- **附件消息不可入队**：busy 时直接拒绝（`acp-protocol.md:581`）。
  如需支持，扩展排队结构以携带附件 block。
- **非图片附件降级**：非图片（或 agent 不支持图片）时只在文本尾部追加
  `"[附件] <绝对路径>"`（`acp-protocol.md:584`）。
  如 agent 的 `promptCapabilities` 支持其他 media type，应改为对应的 content block。
- **AskUserQuestion 窄化**：`multi_select` 按单选处理（`acp-protocol.md:350`）；
  kimi 0.29.x 适配器多问题只发第一问（agent 侧限制，客户端已前向兼容）。

## 6. 实现顺序（已执行完毕）

1. ✅ **可观测性先行**：未知 update kind / 未知方法 / 孤儿响应均已加日志。
2. ✅ **`available_commands_update`** → Composer `/` 补全菜单。
3. ✅ **`plan`** → 聊天内嵌计划卡片。
4. ✅ **`config_option_update` + set_config_option 响应读取** → 配置同步闭环。
5. ✅ **回放期状态类 update 的保留策略**（available_commands 回放期只存不发，
   session_ready 补发；plan 随回放丢弃）。
6. ❌ 剩余：`terminal/*`（内嵌终端）、扩展附件类型、附件入队。
7. ✅ 补充实现：`authenticate`（-32002 自动认证重试）。

> 实施时注意：Rust 侧的分派点在 `src-tauri/src/acp/`（参照 `acp-protocol.md`
> 第 10 节建议的 `acp/types.rs` / `acp/events.rs` 结构），tool_call 归一化
> 建议在 `types.rs` 一处完成，事件通过 `AcpEvent` 枚举上抛。

---

## 附录：ACP 协议完整操作清单

对照 [ACP 官方规范](https://agentclientprotocol.com/protocol/overview) 与
`acp-protocol.md` 整理，并标注 WarDex 实现现状（✅ 已实现 / ❌ 未实现 / ⚠️ 部分）。

### A. Agent 侧方法（Client → Agent）

**Baseline（必须实现）**

| 方法 | 类型 | 说明 | WarDex |
|---|---|---|---|
| `initialize` | 请求 | 握手：协商协议版本、双向声明 capabilities | ✅ |
| `authenticate` | 请求 | agent 要求时进行认证（-32002 时自动认证一次并重发原请求） | ✅ |
| `session/new` | 请求 | 创建新会话，params 带 `cwd`、`mcpServers` | ✅ |
| `session/prompt` | 请求 | 发送用户消息，响应带 `stopReason` 结束回合 | ✅ |

**Optional（能力声明后才可用）**

| 方法 | 说明 | WarDex |
|---|---|---|
| `session/load` | 恢复已有会话（agent 需声明 `loadSession: true`），回放全部历史 update | ✅ |
| `logout` | 注销 | ❌ |
| `session/set_mode` | 切换会话 mode（官方版方法） | ❌（用扩展版替代） |
| `session/set_config_option` | 新版扩展：mode 和其他 picker 统一走此方法的不同 `configId`（`acp-protocol.md:300`） | ✅ |

**通知**

| 方法 | 说明 | WarDex |
|---|---|---|
| `session/cancel` | 取消当前回合（agent 可忽略，WarDex 有 2500ms 强杀兜底，第 9.6 节） | ✅ |

### B. Client 侧方法（Agent → Client，反向 RPC）

**Baseline**

| 方法 | 说明 | WarDex |
|---|---|---|
| `session/request_permission` | 请求用户批准工具操作，应答 `outcome` = selected/cancelled | ✅ |

**Optional（initialize 声明能力后 agent 才会调用）**

| 方法 | 说明 | WarDex |
|---|---|---|
| `fs/read_text_file` | 读文件（支持 line/limit 1-based 裁剪） | ✅ |
| `fs/write_text_file` | 写文件（自动建父目录、截断写） | ✅ |
| `terminal/create` | 创建终端并执行命令（需 `terminal: true`） | ❌（声明了 `terminal: false`） |
| `terminal/output` | 读终端输出和退出状态 | ❌ |
| `terminal/release` | 释放终端 | ❌ |
| `terminal/wait_for_exit` | 等待命令退出 | ❌ |
| `terminal/kill` | 杀命令但不释放终端 | ❌ |

**通知**

| 方法 | 说明 | WarDex |
|---|---|---|
| `session/update` | 流式进度通知，按 `sessionUpdate` kind 分派（见 C） | ✅ 全部 kind 已分派 |

### C. `session/update` 的 kind 全集

| kind | 说明 | WarDex |
|---|---|---|
| `user_message_chunk` | 回放的用户消息 | ✅（日志后忽略，只在回放有意义） |
| `agent_message_chunk` | 助手回复流 | ✅ |
| `agent_thought_chunk` | 思考流 | ✅ |
| `tool_call` | 工具调用 | ✅ |
| `tool_call_update` | 工具调用状态更新 | ✅ |
| `plan` | 任务计划更新 | ✅（计划卡片，第 1 节） |
| `available_commands_update` | 斜杠命令列表 | ✅（`/` 补全菜单，第 1 节） |
| `current_mode_update` | mode 变更 | ✅ |
| `config_option_update` | 配置项变更（扩展） | ✅ |
| `usage_update` | token 用量（较新） | ✅ |
| `session_info_update` | 会话标题等元信息（较新） | ✅ |

### D. 扩展机制

协议自带三种扩展方式：

- `_meta` 字段携带自定义数据；
- `_` 前缀的自定义方法；
- initialize 时声明自定义 capabilities。

kimi 的 `AskUserQuestion`（`acp-protocol.md:350` 附近）即此类扩展命名空间。

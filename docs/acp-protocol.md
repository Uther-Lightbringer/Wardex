# ACP 协议客户端规格

> 本文档是 WarDex Tauri 重写中 ACP（Agent Client Protocol）客户端的完整实现规格。
> 所有行为均以旧 C++/Qt 代码库（`C:/workspace/WarDex`）为准，文中以 `文件:行号` 标注出处
> （如 `AcpClient.cpp:110` 指 `src/AcpClient.cpp` 第 110 行）。
> 读者无需访问旧仓库即可照此实现；标注仅供交叉核对。

## 相关文档

- [架构总览](./architecture.md)
- [设计原则](./design-principles.md)
- [实施计划](./implementation-plan.md)
- [数据格式](./data-formats.md)
- [聊天功能](./features/chat.md)
- [Provider 与 CLI 探测](./providers-and-cli.md)

## 目标模块布局（建议）

```
src-tauri/src/acp/
├── mod.rs          // 模块入口，re-export
├── transport.rs    // 子进程 spawn + NDJSON 帧读写（对应旧 AcpClient 的传输层）
├── client.rs       // AcpClient 协议状态机（initialize/session/prompt/反向 RPC）
├── types.rs        // serde 数据结构（请求/响应/通知/update 负载）
└── events.rs       // 发往 chat 层的事件枚举（对应旧 Qt signals）
```

chat 层（流式合并、限流重试、断线续写、进程上限、subagent 跟踪、消息队列、附件规则）
属于 `src-tauri/src/chat/`，本文档第 9 节给出其与 ACP 客户端的交互契约，详细设计见
[聊天功能](./features/chat.md)。

---

## 1. 传输层：stdio NDJSON

参照：`AcpClient.cpp:242-318`（writeJson / onReadyRead / handleLine）。

### 1.1 帧格式

- 每条 JSON-RPC 消息是**一行紧凑 JSON**（无多余空白）+ `\n`，写入子进程 stdin
  （`AcpClient.cpp:246-247`）。
- 读方向：累积 stdout 到缓冲区，按 `\n` 切行；每行 `trim` 后为空则跳过；
  JSON 解析失败或不是 object 则**记日志并丢弃该行**，不中断连接
  （`AcpClient.cpp:290-300, 309-318`）。agent CLI 可能在 stdout 打印 banner/日志噪声，
  必须容忍。
- stderr 走独立通道，只读出来记日志（截断前 500 字符），不参与协议
  （`AcpClient.cpp:65, 79-83`）。

### 1.2 消息分派

收到一行 JSON object 后（`AcpClient.cpp:320-466`）：

- 含 `id` 且**不含** `method` → 是对我方请求的响应，按 id 匹配挂起的请求。
- 含 `method` → agent 发来的请求或通知：
  - `session/update` → 通知，处理（见第 6 节）；
  - `session/request_permission` / `fs/read_text_file` / `fs/write_text_file` 且含 `id`
    → 反向请求，需应答（见第 5 节）；
  - 其他方法：若含 `id`，回复 JSON-RPC 错误 `-32601`，message 为
    `"Method not found: <method>"`（`AcpClient.cpp:463-465`）；不含 id 的通知直接忽略。

### 1.3 请求 id

- 我方请求 id 为整数，从 1 开始自增（`AcpClient.h:83` `m_nextId = 1`）。
- 协议层需记住 5 个"在途请求 id"：`initialize`、`session/new`、`session/load`、
  `session/prompt`、`session/set_config_option`（`AcpClient.h:84-88`），响应按 id 归位；
  未匹配任何在途 id 的响应静默忽略（`AcpClient.cpp:411`）。

### 1.4 消息骨架（types.rs 中建议用 serde 手写构造，保持字段顺序无关、缺省可省）

```jsonc
// 请求        { "jsonrpc": "2.0", "id": 1, "method": "...", "params": {...} }   // AcpClient.cpp:250-258
// 通知        { "jsonrpc": "2.0", "method": "...", "params": {...} }             // AcpClient.cpp:260-267
// 成功响应    { "jsonrpc": "2.0", "id": 1, "result": {...} }                     // AcpClient.cpp:269-276
// 错误响应    { "jsonrpc": "2.0", "id": 1, "error": { "code": -32000, "message": "..." } }  // AcpClient.cpp:278-288
```

---

## 2. 子进程 spawn

参照：`AcpClient.cpp:49-117`（`AcpClient::start`）。

### 2.1 启动参数

`start(cliPath, acpArgs, env, cwd, preferredMode, resumeSessionId)`：

1. 先 `stop()` 掉旧进程（`AcpClient.cpp:54`）。
2. `cliPath` trim 后为空 → 直接发 `startFailed("未配置 CLI 命令，请在配置页填写")`，不 spawn
   （`AcpClient.cpp:55-58`）。
3. `cwd` 非空时设为子进程工作目录（`AcpClient.cpp:66-67`）；它同时是后面
   `session/new|load` 的 `cwd` 参数来源。

### 2.2 环境变量：覆盖与 null 删除语义

参照：`AcpClient.cpp:69-76`。

- 以**完整系统环境**为底，再套用 `env` 覆盖表：
  - 值非 null → 设置/覆盖该变量；
  - **值为 null → 从子进程环境中删除该变量**（provider `clearEnvs` 防嵌套机制，
    见 providers-and-cli.md）。
- Rust 实现注意：`std::process::Command::env_remove` 对应删除语义；必须先
  `env_clear` 再灌入系统环境再套用覆盖表，或在系统环境 map 上操作后整体 `envs`。

### 2.3 Windows .cmd/.bat 特例（必须保留）

参照：`AcpClient.cpp:93-108`。

npm 全局安装的 CLI（`claude-code-acp`、`codex-acp`）是 `.cmd` shim，Windows
`CreateProcess` 无法直接 exec。启动前处理：

1. `cliPath` 不是绝对路径 → 先在 PATH（含 PATHEXT）中解析为绝对路径
   （旧代码用 `QStandardPaths::findExecutable`）。
2. 解析结果以 `.cmd` 或 `.bat` 结尾（大小写不敏感）→ 改写为：
   `program = "cmd.exe"`，`args = ["/c", <解析后的绝对路径>, ...acpArgs]`。
3. 否则用解析后的路径（若解析成功）作为 program。

Rust 侧注意：`tokio::process::Command` 走 `CreateProcess`，同样不能直接起 `.cmd`；
解析 PATH 可用 `which` crate 或自实现（需试 `.exe/.cmd/.bat` 后缀）。

### 2.4 启动超时与失败

- spawn 后等待进程进入 running 状态，**超时 8000ms**（`AcpClient.cpp:110`
  `waitForStarted(8000)`）。超时或 spawn 失败 → 清理进程并发 `startFailed`
  （错误串为空时用 `"无法启动 ACP 进程 «<program>»"`）。
- 进程在 initialize 完成前发生 spawn 级错误 → 同样发 `startFailed`
  （`AcpClient.cpp:86-89`）。
- 进程退出（任何时刻）→ 清 `turnBusy`、`initialized`，发 `processExited(exitCode)`
  （`AcpClient.cpp:302-307`）。chat 层据此做断线续写/中断标记（见第 9.4 节）。

### 2.5 stop 语义

参照：`AcpClient.cpp:28-47`。

`stop()`：kill 子进程并等待最多 1500ms；清空读缓冲、sessionId、所有在途 id、
`initialized/turnBusy/canLoadSession/imageSupported/replaying` 全部复位。

---

## 3. 启动握手序列

参照：`AcpClient.cpp:119-135, 356-397, 468-485`。

顺序：`spawn` → `initialize` → （`session/load` | `session/new`） → 应用 mode → `started`。

### 3.1 `initialize`

进程启动成功后立即发送（id 记为 `m_initId`）：

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "clientInfo": { "name": "WarDex", "version": "0.2" },
    "clientCapabilities": {
      "fs": { "readTextFile": true, "writeTextFile": true },
      "terminal": false
    }
  }
}
```

- `clientInfo.name`/`version` 固定为 `WarDex` / `0.2`（`AcpClient.cpp:127-129`；
  新项目如需改版本号，注意 AgentStore 的 testAgent 握手也写死了同样的值，
  见 providers-and-cli.md 第 4.3 节，两处要同步）。
- 响应 result 中读取两个能力（`AcpClient.cpp:359-364`）：
  - `result.agentCapabilities.loadSession` (bool) → `canLoadSession`；
  - `result.agentCapabilities.promptCapabilities.image` (bool) → `imageSupported`，
    决定 `session/prompt` 是否允许附带 image block。
- 响应是 error → `startFailed(error.message || "ACP 初始化失败")`
  （`AcpClient.cpp:335-336`）。

### 3.2 `session/load`（恢复 agent 侧会话）

条件：`canLoadSession == true` 且调用方给了 `resumeSessionId`（`AcpClient.cpp:365`）。

```json
{
  "method": "session/load",
  "params": {
    "sessionId": "<resumeSessionId>",
    "cwd": "<工作目录，空则用当前进程 cwd>",
    "mcpServers": [ /* 原样透传，见 3.4 */ ]
  }
}
```

- 发出后置 `replaying = true`（`AcpClient.cpp:367`）：session/load 会把整个历史以
  `session/update` 通知**回放**一遍，而 WarDex 自己持久化了历史，回放期间收到的所有
  `session/update` 必须**全部丢弃**（`AcpClient.cpp:489-491`）。
- 成功：以 `resumeSessionId` 为当前 sessionId，清 `replaying`，进入 sessionReady
  （`AcpClient.cpp:380-386`）。注意 result 里没有新 sessionId，沿用请求里的。
- **失败回退**：响应为 error（本地存的 sessionId 在 agent 侧已过期/未知）→ 清
  `replaying`，**自动改发 `session/new`**，整个 start 不算失败
  （`AcpClient.cpp:328-334`）。这是必须保留的行为。

### 3.3 `session/new`

不满足 load 条件、或 load 失败回退时发送（`AcpClient.cpp:468-475`）：

```json
{
  "method": "session/new",
  "params": {
    "cwd": "<工作目录，空则用当前进程 cwd>",
    "mcpServers": []
  }
}
```

- 成功：`result.sessionId` 为当前 sessionId；**为空字符串则 `startFailed("session/new 未返回 sessionId")`**（`AcpClient.cpp:388-394`）。
- 响应 error → `startFailed(error.message || "ACP 初始化失败")`（`AcpClient.cpp:335-336`）。

### 3.4 mcpServers 透传

`session/new` 和 `session/load` 的 params 都带 `mcpServers` 数组，内容由上层
（agent 配置的 `mcpServers` JSON 文本解析而来）**原样透传**，协议层不校验条目结构
（`AcpClient.h:33-37, 101-103`；解析与降级逻辑在 `ChatController.cpp:877-894`）。
缺省为空数组 `[]`。

### 3.5 sessionReady：应用 mode 并发 started

参照：`AcpClient.cpp:477-485`。

会话建立后：

- `pendingMode` 非空且不等于 `"default"` → 自动发一次 `session/set_config_option`
  （见 4.4）；
- 否则发事件 `modeChanged("default")`。
- 随后发 `started(sessionId)` 事件。chat 层在收到 `started` 后才发挂起的 prompt
  （`ChatController.cpp:325-339`）。

---

## 4. 会话期请求

### 4.1 `session/prompt`

参照：`AcpClient.cpp:152-197`。

发送前守卫：sessionId 为空**或** `turnBusy == true` → 不发请求，直接发
`protocolError("ACP 会话未就绪或仍在生成")` 事件（`AcpClient.cpp:168-171`）。
通过后 `turnBusy = true`。

```json
{
  "method": "session/prompt",
  "params": {
    "sessionId": "...",
    "prompt": [
      { "type": "text", "text": "<用户文本>" },
      { "type": "image", "mimeType": "image/png", "data": "<base64>" }
    ]
  }
}
```

- text 为空字符串时**不生成 text block**（纯图片消息合法）（`AcpClient.cpp:175-180`）。
- image block：仅当 `imageSupported == true` 才附加，否则**静默跳过**所有图片
  （`AcpClient.cpp:181-192`）。
- 图片读本地文件转 base64；读失败的文件静默跳过。mimeType 按扩展名映射
  （`AcpClient.cpp:152-164`）：`jpg|jpeg → image/jpeg`，`webp → image/webp`，
  `gif → image/gif`，`bmp → image/bmp`，**其余一律 `image/png`**。
- 成功响应：`result.stopReason`（字符串，如 `end_turn`/`cancelled`/`max_tokens` 等），
  缺省按 `"end_turn"` 处理；随后 `turnBusy = false` 并发 `turnFinished(stopReason)`
  （`AcpClient.cpp:404-409`）。
- 错误响应：见第 7 节的严格顺序。

### 4.2 `session/cancel`（通知）

```json
{ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": "..." } }
```

sessionId 为空时不发（`AcpClient.cpp:199-206`）。是**通知**，无响应。
agent 收到后应以 `session/prompt` 的响应（`stopReason: "cancelled"`）结束回合；
agent 可能忽略它——chat 层有 2500ms 强杀兜底（第 9.6 节）。

### 4.3 mode 值与 `session/set_config_option`

参照：`AcpClient.cpp:138-150, 477-485`。

```json
{
  "method": "session/set_config_option",
  "params": { "sessionId": "...", "configId": "mode", "value": "<modeId>" }
}
```

- `configId` 固定为字符串 `"mode"`。
- WarDex 侧 mode id 取值：`default | plan | auto | yolo`；发给 agent 前要先经
  provider 的 `modeMap` 翻译（如 claude: `auto→acceptEdits`、`yolo→bypassPermissions`），
  翻译发生在 chat 层（`ChatController.cpp:908-916`），协议层收到什么发什么。
- 会话未建立时调用 setMode：只缓存为 `pendingMode`，等 sessionReady 后自动应用
  （`AcpClient.cpp:140-142`）。
- 响应忽略内容（result 里可能带 `configOptions`，不读），发 `modeChanged(pendingMode)`
  事件（`AcpClient.cpp:398-402`）。
- mode 变更时机：每次 `session/prompt` 之前 chat 层都会先 `setMode` 保持同步
  （`ChatController.cpp:1066`）；UI 切 mode 时只对当前活动进程立即下发
  （`ChatController.cpp:28-38`）。

---

## 5. 反向 RPC（agent → client）

参照：`AcpClient.cpp:208-240, 424-465`。

### 5.1 `session/request_permission`

agent 请求用户批准一次工具调用。params 原样转发给 UI（含 `toolCall`、`options` 等），
由 UI 弹出确认。应答（`AcpClient.cpp:208-220`）：

```jsonc
// 用户选了一个选项
{ "id": <reqId>, "result": { "outcome": { "outcome": "selected", "optionId": "<optionId>" } } }
// 用户取消
{ "id": <reqId>, "result": { "outcome": { "outcome": "cancelled" } } }
```

注意结构是 `result.outcome.outcome`（嵌套两层 outcome），别写成平铺。
chat 层为 UI 会给 params 补一个 `_uiTitle` 字段（toolCall.title → kind → "工具调用"）
（`ChatController.cpp:385-403`）。

#### AskUserQuestion 的 `q{n}_*` 命名空间（kimi acp 适配器）

kimi 的 ACP 适配器把 AskUserQuestion 工具桥接进 `request_permission`（ACP 没有专用
question 方法），optionId 带命名空间以便回传不歧义（kimi 0.29.1 抓包实证）：

```jsonc
// options: 每个真实选项一项 + 一项 Skip
{ "optionId": "q0_opt_0", "name": "Red",  "kind": "allow_once"  }
{ "optionId": "q0_skip",  "name": "Skip", "kind": "reject_once" }
// toolCall.title 固定 "AskUserQuestion"，问题文本在 toolCall.content[].content.text
```

- 命名规则：`q{问题序号}_opt_{选项序号}` / `q{问题序号}_skip`；问题文本也可能经
  `toolCall.rawInput.questions[]` 携带（含 `multi_select`/`multiSelect` 标志）。
- Rust 侧 `acp/types.rs::parse_question_request` 把 options 按问题分组，随
  `acp://permission` 事件的 `questions` 字段下发给前端分组渲染；不匹配命名空间的
  option（普通批准流）分组为空，行为不变。
- **应答窄化**：ACP 一次响应只带一个 optionId，所以每问按单选应答——与 kimi 适配器
  自身的窄化一致（`multi_select` 问题同样按单选）。
- **已知 agent 侧限制**：kimi 0.29.x 的适配器对一次多问题调用只发第一问（其余在
  agent 侧丢弃，不上线）；claude-code-acp 直接禁用 AskUserQuestion；codex-acp 不发
  此命名空间。客户端的解析/分组渲染是前向兼容：线上一出现多组即全部可展示。

### 5.2 `fs/read_text_file`

params：`{ "sessionId", "path", "line"?, "limit"?, ... }`。

- 以 UTF-8 文本读 `path`；打不开 → 回 JSON-RPC 错误 **code `-32000`**，message
  `"无法读取: <path>"`（`AcpClient.cpp:428-434, 222-227`）。
- `line`/`limit` 可选的行裁剪（`AcpClient.cpp:436-443`）：
  - 按 `\n` 切行；
  - `line > 0` 时起点为第 `line` 行（**1-based**，即 `start = line - 1`），否则从 0 开始；
  - `limit > 0` 时最多取 `limit` 行，否则取到文件尾；
  - 裁剪后用 `\n` 重新 join。
- 成功应答：`{ "id": <reqId>, "result": { "content": "<文本>" } }`。

### 5.3 `fs/write_text_file`

params：`{ "sessionId", "path", "content", ... }`。

- 先创建所有缺失的父目录（mkpath），再以**截断写**方式写入 UTF-8 内容
  （`AcpClient.cpp:448-459`）。
- 写失败 → 错误 code `-32000`，message `"无法写入: <path>"`。
- 成功应答：`{ "id": <reqId>, "result": {} }`（空对象）。

### 5.4 未知方法

带 `id` 的未知反向请求 → 回 `-32601 "Method not found: <method>"`
（`AcpClient.cpp:463-465`）。不带 id 的未知通知直接忽略。
（若 agent 请求 `terminal/*`，因为我们 initialize 声明了 `terminal: false`，
正常 agent 不会发；收到也走 -32601。）

---

## 6. `session/update` 通知处理

参照：`AcpClient.cpp:487-517`。

params 结构：`{ "sessionId": "...", "update": { "sessionUpdate": "<kind>", ... } }`。

**回放丢弃**：`replaying == true`（session/load 在途）时所有 update 直接 return
（`AcpClient.cpp:489-491`）。

按 `update.sessionUpdate` 分派：

| kind | 处理 |
|---|---|
| `agent_thought_chunk` | 取 `update.content.text`，发 `thoughtChunk(text)` |
| `agent_message_chunk` | 取 `update.content.text`，发 `messageChunk(text)` |
| `tool_call` | 归一化后发 `toolCall(map)` |
| `tool_call_update` | 归一化后发 `toolCallUpdate(map)` |
| `available_commands_update` / `plan` / `config_option_update` / 其他 | **忽略**（`AcpClient.cpp:516`） |

**tool_call 归一化**（`AcpClient.cpp:505-514`）：把整个 `update` 对象转成 map；
若顶层没有 `toolCallId` 但有 `toolCall` 字段（部分 adapter 嵌套一层），则改用
`update.toolCall` 子对象。

chat 层在此基础上还有第二级归一化 `toolFromUpdate`（`ChatController.cpp:466-483`）：
缺 `name` 时用 `title` 填充，再缺则用 `kind` 填充。建议在 Rust 侧把两级归一化合并到
`acp/types.rs` 一处完成。

事件负载建议（acp/events.rs）：

```rust
pub enum AcpEvent {
    Started { session_id: String },
    StartFailed { error: String },
    ModeChanged { mode: String },
    ThoughtChunk { text: String },
    MessageChunk { text: String },
    ToolCall { call: serde_json::Value },        // 已归一化的 map
    ToolCallUpdate { update: serde_json::Value },
    PermissionRequested { request_id: i64, params: serde_json::Value },
    TurnFinished { stop_reason: String },
    ProtocolError { error: String },
    ProcessExited { code: i32 },
}
```

---

## 7. prompt 错误路径（严格顺序，限流检测依赖它）

参照：`AcpClient.cpp:325-353`；chat 层依赖：`ChatController.cpp:50-64, 404-444`。

`session/prompt` 的响应为 error 时，**必须按以下顺序发三个事件**：

1. `protocolError(error.message)` —— 先发，让 chat 层拿到原始错误文本；
2. `messageChunk("回合失败：" + error.message)`（message 非空时）—— 把错误显示在
   气泡里，否则用户只能去翻 stderr 日志（如 claude-code-acp 的 403 鉴权失败）；
3. `turnFinished("error")` —— 最后发，关闭回合。

同时 `turnBusy = false`、清 `m_promptId`。

**为什么顺序不能乱**：chat 层的限流自动重试（Phase 4）在收到 `turnFinished("error")`
时检查"本次回合的最后一个 protocolError 文本"是否匹配限流特征（`ChatController.cpp:53-64`
`isRateLimitError`，大小写不敏感地包含 `429` / `rate limit` / `ratelimit` /
`too many requests` / `quota` / `resource exhausted`）。如果 `turnFinished` 先于
`protocolError` 到达，检测会读到上一个回合的陈旧错误文本，误判或漏判。

其他请求的 error 响应路径：`session/load` → 回退 session/new（3.2 节）；
`initialize` / `session/new` → `startFailed`；其余在途 id → 只发 `protocolError`
（`AcpClient.cpp:335-352`）。

---

## 8. AcpClient 状态字段一览

对应 `AcpClient.h:81-103`，Rust 侧 `client.rs` 的 `AcpClient` 结构需要：

| 字段 | 类型 | 说明 |
|---|---|---|
| `next_id` | i64 | 请求 id 计数器，从 1 开始 |
| `init_id / new_session_id / load_session_id / prompt_id / set_mode_id` | Option\<i64\> | 在途请求 id |
| `session_id` | String | 当前 agent 侧会话 id |
| `cwd` | String | 工作目录（session/new|load 参数来源） |
| `pending_mode` | String | 会话建立前缓存的 mode |
| `resume_session_id` | String | start 时传入的待恢复会话 id |
| `initialized` | bool | initialize 已完成 |
| `turn_busy` | bool | 有 prompt 在途（拒绝重入） |
| `can_load_session` | bool | agentCapabilities.loadSession |
| `image_supported` | bool | agentCapabilities.promptCapabilities.image |
| `replaying` | bool | session/load 回放中，丢弃所有 update |
| `mcp_servers` | Vec\<Value\> | 透传给 session/new|load |
| `buf` | Vec\<u8\> | stdout 行缓冲 |

---

## 9. chat 层与 ACP 的交互契约（摘要）

以下行为在旧代码 `ChatController.cpp` 中，重写时属于 `src-tauri/src/chat/`。这里只列与
ACP 客户端直接相关的契约，保证两边接口对齐。

### 9.1 流式分片合并 flush

参照：`ChatController.cpp:356-372, 1184-1208`。

- `thoughtChunk`/`messageChunk` 先进 `pendingThinking`/`pendingContent` 缓冲，
  由 **50ms** 单次定时器合并落盘；待 flush 数据 > 64KB 时定时器间隔拉长到 **250ms**。
- `toolCall`/`toolCallUpdate`/`turnFinished` 到达前**先强制 flush**，保证文本与工具调用
  在段序列中的到达顺序不乱（`ChatController.cpp:374, 380, 405`）。
- 运行期完整 buffer 只保留尾部 2000 字符（`kStreamBufferKeep`，
  `ChatController.h:168`），全文由存储层逐段持有；尾部用于断线续写（取 `right(500)`）
  和判空。

### 9.2 限流自动重试（Phase 4）

参照：`ChatController.cpp:404-419, 1243-1340`；常量 `ChatController.h:161-165`。

- 触发：`turnFinished("error")` 且非用户主动停止、且 `lastTurnError` 命中
  `isRateLimitError`（见第 7 节）、且 `retryPrompt` 非空、且已重试次数 < 3。
- 策略：最多 **3 次**自动重发，指数退避 **20s → 40s → 80s**，单次上限 **300s**；
  1s 定时器倒计时。
- **只有纯文本 prompt 可重发**：`retryPrompt` 在发图回合置空（base64 不保留，
  重读文件可能内容已变）（`ChatController.cpp:1028-1036`）。非图片附件已内联进
  文本，不受影响。
- 重发时**不新增历史行**：用户行已持久化，重试的回复流回同一个 assistant 气泡。
- 新用户消息、guide、手动取消都会**取代**（supersede）挂起的重试：气泡结算为失败
  `"回合失败：请求被限流，已取消自动重试"`，回合按普通错误路径关闭
  （`ChatController.cpp:931-934, 1310-1333`）。
- 进程死亡会连带取消挂起的重试（`ChatController.cpp:448-451`）。

### 9.3 断线续写（进程崩溃自动 continue）

参照：`ChatController.cpp:445-463, 1210-1231`；常量 `kMaxContinueRetries = 2`
（`ChatController.h:160`）。

回合进行中 `processExited` 且非用户停止、assistant buffer 非空、续写次数 < 2 →
重启 ACP 进程，握手后自动发**合成续写 prompt**（不入本地历史）：

```
上一条回复因连接中断被截断。请紧接着已输出的内容继续，不要重复已输出的部分，不要重新开头，不要解释。已输出内容的结尾片段：
…<assistant buffer 尾部 500 字符>
```

重启走 `session/load` 恢复原会话；即便回退成 `session/new` 丢了上下文，尾部片段也能
锚定模型。不满足条件 → 标记"（已中断）"并关闭回合。

### 9.4 进程并发上限

参照：`ChatController.cpp:293-317`；常量 `kMaxParallelAcp = 3`（`ChatController.h:50`）。

每次要启动新 ACP 进程前执行：运行中进程数 ≥ 3 时，停掉**最久未活动（LRU）的空闲**
进程（busy 的绝不动）；全是 busy 则允许临时超限。被停的会话下次发言时经
`session/load` 恢复。

### 9.5 switchAgent

参照：`ChatController.cpp:748-808`。

- 进行中的回合按强制取消处理（标记已中断），清 pendingPrompt——绝不能让它落进
  新 agent 的进程。
- **同 provider 切换**：保留 agent 侧 sessionId，新进程用 `session/load` 恢复
  agent 侧历史。
- **跨 provider 切换**：清掉存储的 acpSessionId（拿去 load 必然失败），走 `session/new`。
  本地 WarDex 历史两种情况下都不动。
- 切换结果持久化到会话 meta（`setSessionAgentId`），重开会话时生效。

### 9.6 取消与 guide 插队

参照：`ChatController.cpp:1070-1180`。

- `cancel()`：若有限流重试在倒计时 → 只取消重试，**不发** `session/cancel`；
  有挂起权限请求 → 先以 `cancelled` 应答；然后 `userStop = true` + `session/cancel`；
  **2500ms** 后 agent 仍未结束 → 直接 stop 进程（两边 turn 状态已不可信，
  AcpClient 的 `turnBusy` 还卡着，必须重启连接）。
- `guideAt(i)`（队列插队）：先取消权限请求（否则旧 dialog 会卡在新回合上），
  再 `session/cancel`，**800ms** 后旧回合还活着就杀进程再发新 prompt。

### 9.7 消息队列与附件规则

参照：`ChatController.cpp:920-1004`；常量 `kMaxQueueSize = 10`（`ChatController.h:47`）。

- busy 时纯文本消息入队，上限 10 条，回合结束后 FIFO 逐个 drain。
- **附件消息不可入队**：busy 时直接拒绝（错误提示"生成中不支持附件入队，请等待当前回复完成"）。
- 附件分流（`isImagePath`：png/jpg/jpeg/webp/gif/bmp）：
  - 是图片且 agent `imageSupported` → 作为 image block 发送；
  - 否则（非图片，或 agent 不支持图片）→ 在文本尾部追加一行 `"[附件] <绝对路径>"`，
    agent 在自己的 cwd 里用工具读文件。
- 文本为空但有图片时，发送占位文本 `"（图片）"`。

### 9.8 subagent 跟踪

参照：`ChatController.cpp:485-651`。

ACP 没有专门的 subagent 事件，CLI 把 subagent 工作报为普通工具调用（已对
kimi acp 0.29.1 验证）：

- 工具名（归一化后的 `name`，大小写不敏感）属于
  `agent | agentswarm | task | spawn_agent` 时新建跟踪条目（初始 status `pending`）；
  `tool_call_update` 不带名字，只能靠 `toolCallId` 匹配已有条目。
- 输入参数随 `tool_call_update` 流式到来：`content` 数组里 `type == "content"` 块的
  `content.text` 是**累积快照**（kimi），但别的 adapter 可能发增量——先尝试解析
  最后一个块的 JSON，失败再尝试全部拼接的 JSON（`parseToolInput`，
  `ChatController.cpp:509-527`）。
- 从解析出的 input JSON 提取：`description` → 标题；无 description 用 `prompt`
  截断 48 字符加 `…`；`items` 数组 → `children` 数量与 `childNames`。
- 完成时（status `completed|failed`）从 `rawOutput` 提取摘要：
  - 以 `<agent_swarm_result>` 开头 → 数 `outcome="..."` 出现次数与其中
    `completed` 次数，生成 `"完成 <ok>/<total>"`；
  - 否则找 `actual_subagent_type:` 行，显示其值。
- 回合结束时仍 `pending|in_progress` 的条目强制置为 `completed`（或中断时
  `interrupted`）（`finishSubagents`）。

---

## 实现检查清单

传输层（`acp/transport.rs`）：

- [ ] stdout NDJSON 按 `\n` 切行，trim、空行跳过、坏 JSON 记日志后丢弃（`AcpClient.cpp:290-318`）
- [ ] stdin 写紧凑 JSON + `\n`（`AcpClient.cpp:242-248`）
- [ ] stderr 独立通道只记日志（截断 500 字符）（`AcpClient.cpp:79-83`）
- [ ] env 覆盖表：null 值 = 删除变量，底为完整系统环境（`AcpClient.cpp:69-76`）
- [ ] Windows `.cmd`/`.bat` 解析后包 `cmd.exe /c`（`AcpClient.cpp:93-108`）
- [ ] spawn 启动等待 8000ms 超时 → `startFailed`（`AcpClient.cpp:110-117`）
- [ ] `stop()`：kill + 1500ms 等待，全部状态复位（`AcpClient.cpp:28-47`）

协议状态机（`acp/client.rs`）：

- [ ] 请求 id 从 1 自增；5 个在途 id 跟踪；未知响应 id 静默忽略
- [ ] `initialize`：protocolVersion 1、clientInfo WarDex/0.2、capabilities fs rw + terminal false（`AcpClient.cpp:119-135`）
- [ ] 读取 `agentCapabilities.loadSession` 与 `promptCapabilities.image`（`AcpClient.cpp:359-364`）
- [ ] `session/load`：replaying 期间丢弃所有 session/update；error 自动回退 `session/new`（`AcpClient.cpp:328-334, 365-386`）
- [ ] `session/new`：空 sessionId → `startFailed`（`AcpClient.cpp:388-394`）
- [ ] `mcpServers` 原样透传，缺省 `[]`（`AcpClient.cpp:373, 473`）
- [ ] sessionReady：非 default mode 自动下发 set_config_option，随后发 `started`（`AcpClient.cpp:477-485`）
- [ ] `session/prompt`：turnBusy 重入守卫；text 空不加 text block；image 按能力静默跳过；mime 映射含"默认 png"（`AcpClient.cpp:152-197`）
- [ ] `session/cancel` 为通知（`AcpClient.cpp:199-206`）
- [ ] `session/set_config_option`：configId 固定 `"mode"`；无会话时缓存 pendingMode（`AcpClient.cpp:138-150`）
- [ ] prompt 错误路径严格按 protocolError → messageChunk("回合失败：…") → turnFinished("error") 顺序（`AcpClient.cpp:337-349`）

反向 RPC：

- [ ] `session/request_permission` 应答结构 `result.outcome.outcome` = selected/cancelled（`AcpClient.cpp:208-220`）
- [ ] `fs/read_text_file`：UTF-8 读、line/limit 1-based 行裁剪、失败 `-32000`（`AcpClient.cpp:428-446`）
- [ ] `fs/write_text_file`：建父目录、截断写、失败 `-32000`（`AcpClient.cpp:448-460`）
- [ ] 未知带 id 方法回 `-32601 "Method not found: <method>"`（`AcpClient.cpp:463-465`）

session/update：

- [ ] 四类 kind 处理 + 忽略其余（`AcpClient.cpp:495-516`）
- [ ] tool_call 归一化（toolCall 子对象拆包；缺 name 用 title/kind 补）（`AcpClient.cpp:505-514`、`ChatController.cpp:466-483`）

chat 层契约（`src-tauri/src/chat/`，详见 features/chat.md）：

- [ ] 50ms/250ms 合并 flush；tool/turnFinished 前强制 flush；buffer 尾部保留 2000
- [ ] 限流重试：isRateLimitError 六个特征串、3 次、20→40→80s 封顶 300s、图片回合不重试
- [ ] 断线续写：2 次上限、合成 prompt 带尾部 500 字符、不入历史
- [ ] 进程上限 3、LRU 杀空闲、busy 不动
- [ ] switchAgent：同 provider 保留 acpSessionId、跨 provider 清空
- [ ] cancel 2500ms 强杀兜底；guideAt 800ms 兜底且先取消权限请求
- [ ] 队列上限 10；附件不入队；非图片附件内联 `"[附件] <路径>"`
- [ ] subagent 跟踪：四个工具名、parseToolInput 双策略、swarm outcome 统计

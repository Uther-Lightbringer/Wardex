# WarDex 磁盘数据格式与持久化层完整规格

> 本文档定义 WarDex（Tauri 重写版）必须**逐字段兼容**的磁盘数据格式。用户的历史数据位于
> `%AppData%/WarDex/`，新版直接读写，**零迁移**。
>
> 文中所有旧代码参照（`文件:行号`）均指向只读参照仓库 `C:/workspace/WarDex/` 下的路径
> （如 `src/SessionStore.cpp:702` 即 `C:/workspace/WarDex/src/SessionStore.cpp` 第 702 行）。
> 实现时以本文档为准；与旧代码有出入处按旧代码行为修正本文档。

## 相关文档

- [./architecture.md](./architecture.md) —— 整体架构（Tauri 命令/事件模型、前后端分工）
- [./design-principles.md](./design-principles.md) —— 设计原则（内存策略、流式渲染约束）
- [./implementation-plan.md](./implementation-plan.md) —— 分阶段实施计划
- [./acp-protocol.md](./acp-protocol.md) —— ACP（Agent Client Protocol）协议细节
- [./providers-and-cli.md](./providers-and-cli.md) —— Provider 差异与 CLI 探测
- [./features/chat.md](./features/chat.md) —— 聊天功能规格（本文档的调用方）

## 0. 通用约定

以下约定贯穿所有 JSON 文件，实现时极易踩坑，逐条遵守：

- **编码**：全部 UTF-8 无 BOM。读取时对非法 UTF-8 容错（旧代码 `QJsonDocument::fromJson` 解析失败即当空对象/数组处理，不报错、不崩溃）。
- **时间戳**：`createdAt` / `updatedAt` / `lastOpenedAt` / `doneAt` 等一律为 **Unix 毫秒（ms since epoch）的 JSON number**。旧代码写入时是整数形态（如 `1753700000000`），但读取一律走 `toDouble()` 再转 `qint64`（如 `src/SessionStore.cpp:1746`）。新版 Serde 模型**必须把这类字段定义为 `f64` 或 `serde_json::Number` 再转 `i64`**，不能假设是整数，也不能写成科学计数法（旧 Qt 解析器对科学计数法兼容，但为保持文件一致性，写盘用整数形态的 number）。
- **键序**：Qt 的 `QJsonObject` 按键名字典序存储，所以旧文件写出的键是**字母序**的。Rust 侧用 `BTreeMap`（或 `serde_json::Map` 默认的 `preserve_order` 关闭行为）可自然复现，便于新旧文件 diff 一致。这不是兼容性硬要求，但建议遵守。
- **缩进**：单文件 JSON（meta.json、index.json、projects.json、user_prefs.json、todos.json、prompts.json、agents/<id>.json）用 `QJsonDocument::Indented` 写出（4 空格缩进）。JSONL 用 `Compact` 单行 + `\n`。
- **未知字段**：读取方忽略未知字段；写入方保留自己认识的字段全集即可（旧代码本身是读全量 map 改几个键再整体写回，如 `src/SessionStore.cpp:588-600`，天然保留未知字段。新版若为 struct 化序列化，需用 `#[serde(flatten)] extra: Map<String, Value>` 兜住未知键，避免跨版本互相覆盖丢字段——尤其是 meta.json，见 §3）。
- **缺字段**：所有读取点都有缺省值（见各表"缺省"列）；文件不存在 = 全部缺省，不视为错误。
- **UUID**：会话/消息/Agent/Todo/Prompt 的 id 均为 `QUuid::createUuid().toString(WithoutBraces)` 格式：小写 36 字符带连字符（如 `3f2c9a1e-7b4d-4e8f-9c0a-1d2e3f4a5b6c`）。
- **写盘时机**：所有 save 都是"打开 → truncate → 整体写"（无原子 rename，无文件锁）。崩溃可能留下截断文件，读取方必须容错（返回缺省）。

## 1. 数据根目录与目录树全景

### 1.1 根目录定位（参照 `src/AppPaths.cpp:11-27`）

- 旧版规则：基础路径为 `QStandardPaths::AppDataLocation`（即 `%AppData%`）；运行 exe 名为 `wardex-dev`（大小写不敏感）时用 `%AppData%/WarDex-dev`，否则用 `%AppData%/WarDex`。**靠 exe 名判别，无编译开关**。
- **新版规则（与旧版刻意不同）**：
  - 开发期（`tauri dev` / debug build）→ `%AppData%/WarDex-tauri-dev`，与旧 dev/release 数据三方隔离。
  - 发布版 → `%AppData%/WarDex`，**直接复用用户现有数据**。
  - 判别方式建议用 `cfg!(debug_assertions)` 或构建特性 flag，**不要**沿用小旧的 exe 名判别（新 exe 不叫 wardex-dev）。判别处收敛在 `store::paths::root()` 一个函数里。

### 1.2 目录树

```
%AppData%/WarDex/                     # 数据根（dev: WarDex-dev / WarDex-tauri-dev）
├── agents/
│   ├── index.json                    # Agent 列表顺序 + 默认 Agent 指针      §5.1
│   └── <agentId>.json                # 每个 Agent 一个文件                   §5.2
├── sessions/
│   └── <sessionUuid>/
│       ├── meta.json                 # 会话元数据（4 空格缩进 JSON）         §3
│       ├── messages.jsonl            # 消息流（每行一条 compact JSON）       §4
│       └── workspace/                # 遗留：无项目会话的应用托管工作目录    §3.2
├── media/
│   └── <yyyy-MM-dd>/
│       └── <sessionId|"no-session">/
│           └── paste-yyyyMMdd-HHmmss-zzz.png|.jpg   # 剪贴板图片缓存        §9
├── logs/
│   ├── wardex-yyyyMMdd-HHmmss-<pid>.log   # 每次启动一个运行日志
│   └── wardex-latest.txt                  # 文本指针：当前日志文件完整路径
├── crashes/                          # 崩溃转储（旧版 dbghelp minidump）
├── projects.json                     # 最近项目 + 别名                       §6
├── user_prefs.json                   # 用户偏好                              §7
├── user_avatar.png                   # 用户自定义头像（128×128 PNG）          §7.2
├── todos.json                        # Todo 列表                             §8.1
└── prompts.json                      # Prompt 模板（首启种子）                §8.2
```

说明：

- `logs/`、`crashes/` 由旧版 `AppLog`/`CrashHandler` 使用，格式不属于会话数据兼容范围；新版可沿用同名目录写自己的日志（文件名沿用 `wardex-yyyyMMdd-HHmmss-<pid>.log` 与 `wardex-latest.txt` 指针文件，参照 `src/AppPaths.cpp:92-106`），也可替换为自己的方案。`ensureLayout()`（`src/AppPaths.cpp:113-121`）在启动时 mkpath 全部目录并执行 media 清理。
- `sessions/<uuid>/workspace/` 只在该会话**没有绑定项目目录**（`projectDir` 为空）时创建并作为工作目录（`src/SessionStore.cpp:434-437`）。内容完全是 Agent CLI 运行时产生的，WarDex 自己不写任何文件进去。
- 任何时刻都可能存在手工增删的文件/目录；所有扫描逻辑必须忽略不认识的名字。

## 2. sessions/ 索引设计：无索引文件

**没有 sessions 级索引文件**。会话列表 = 启动时扫 `sessions/` 下每个子目录、读各自 `meta.json` 组装（`src/SessionStore.cpp:1676-1707`）：

1. `sessions/` 下所有目录名即为 sessionId 候选；
2. `meta.json` 读不出（不存在/JSON 损坏）→ 该目录跳过，不进列表（`readMeta` 返回空 map，`src/SessionStore.cpp:1723-1729`）；
3. 列表按 `updatedAt` **降序**排序（`src/SessionStore.cpp:1702-1704`）。

启动时还有一个清理动作 `discardEmptySessions()`（`src/SessionStore.cpp:400-412`）：对每个未打开的会话目录，**当 meta 可读且 `messageCount == 0` 时整个目录递归删除**（新建后从未发言的残留）。meta 读不出的目录**保留不动**。仅启动时执行一次。

## 3. sessions/<uuid>/meta.json 逐字段规格

创建入口 `src/SessionStore.cpp:439-456`；写盘 `writeMeta()` `src/SessionStore.cpp:1709-1721`（先 mkpath 会话目录，4 空格缩进整体重写）；读取 `readMeta()` `src/SessionStore.cpp:1723-1729`。

| 字段 | 类型 | 含义 | 缺省（读取时） |
|---|---|---|---|
| `id` | string | 会话 UUID，与目录名一致 | 必填；写盘时为空则拒绝（`src/SessionStore.cpp:1711-1713`） |
| `title` | string | 会话标题。新建固定 `"新会话"`；首条 user 消息写入后自动改为 `content.left(24)`（超长加 `"…"`，`src/SessionStore.cpp:745-751`）；手动改名 trim 后截 48 字符（`src/SessionStore.cpp:585`） | `""` |
| `status` | string | 会话状态，目前只见 `"active"` | `""` |
| `createdAt` | number | 创建时间 ms | `0` |
| `updatedAt` | number | 最后消息写入时间 ms。**改名/置顶/换 Agent 不更新**（`src/SessionStore.cpp:583-641, 665-698`）；仅消息写盘时刷新（`src/SessionStore.cpp:822`） | `0` |
| `messageCount` | number | 消息行数（含 pending 占位 assistant 行），每次写消息后 = 内存模型行数（`src/SessionStore.cpp:821`） | `0` |
| `summary` | string | 列表预览：最近一次写消息的正文 `left(80)`，超长加 `"…"`（`src/SessionStore.cpp:817-820`） | `""` |
| `agentId` | string | 绑定的 Agent UUID（agents/<id>.json 的文件名） | `""` |
| `agentName` | string | Agent 显示名快照（换 Agent 时同步更新，`src/SessionStore.cpp:665-698`） | `""` |
| `provider` | string | `"kimi" \| "claude" \| "codex" \| "custom"` 等，快照 | `""` |
| `model` | string | Agent 模型名快照（仅快照，运行期以 Agent 当前配置为准） | `""` |
| `baseUrl` | string | 快照 | `""` |
| `cliPath` | string | 快照 | `""` |
| `workDir` | string | 工作目录。有项目时 = `projectDir`；无项目时 = `sessions/<uuid>/workspace` 绝对路径（`src/SessionStore.cpp:434-435`） | `""` |
| `projectDir` | string | 绑定的项目目录（canonical 形式，见 §6.1）；空串 = 临时会话 | `""` |
| `pinned` | bool | 置顶标记。后加字段，**老文件没有**，读取缺省 false（`src/SessionStore.cpp:1699`） | `false` |
| `acpSessionId` | string | ACP 层的 session id，用于 `session/load` 恢复（`src/SessionStore.cpp:1084-1101`）。后加字段，可缺 | `""` |

工作区路径解析优先级（`workspacePathFor`，`src/SessionStore.cpp:1121-1134`）：
`projectDir` 非空 → 用它；否则 `workDir` 非空 → 用它；否则回退 `sessions/<id>/workspace`。

示例（真实形态的字段全集；`pinned`/`acpSessionId` 仅在有值时出现）：

```json
{
    "acpSessionId": "019823ab-cdef-7012-8456-abcdef012345",
    "agentId": "8a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
    "agentName": "Kimi 主 Agent",
    "baseUrl": "",
    "cliPath": "C:\\Users\\me\\AppData\\Roaming\\npm\\kimi.cmd",
    "createdAt": 1753600000000,
    "id": "3f2c9a1e-7b4d-4e8f-9c0a-1d2e3f4a5b6c",
    "messageCount": 6,
    "model": "kimi-for-coding",
    "pinned": true,
    "projectDir": "C:/workspace/WarDex",
    "provider": "kimi",
    "status": "active",
    "summary": "帮我把流式渲染的 O(N²) 重建改掉，先给方案…",
    "title": "帮我把流式渲染的 O(N²) 重建改…",
    "updatedAt": 1753690000000,
    "workDir": "C:/workspace/WarDex"
}
```

注意 JSON 里 Windows 路径的反斜杠转义；**内存中统一用正斜杠 canonical 形式**（Qt `QDir::cleanPath` 输出正斜杠，见 §6.1），上例 `cliPath` 的 `C:\\...` 是历史手填值的残留，读取方不要对路径分隔符做假设。

### 3.1 meta 更新规则（写时序）

- `writeMeta()` 永远是**整体重写**（truncate + 全量字段），没有增量写。
- 每条消息写盘后都会调 `updateMetaAfterWrite()`（`src/SessionStore.cpp:811-829`）：刷新 `summary`/`messageCount`/`updatedAt`（+ 可能的 titleHint），然后 writeMeta。
- 改名 `renameSession`、置顶 `setSessionPinned`、换 Agent `setSessionAgentId` 只改对应字段并 writeMeta，**不碰 `updatedAt`**，列表不重排（`src/SessionStore.cpp:583-641, 665-698`）。

### 3.2 无项目会话的 workspace 目录

`createSession`（`src/SessionStore.cpp:420-470`）：`projectDir` 为空时创建 `sessions/<uuid>/workspace/` 并把它写进 `workDir`；非空时校验目录必须存在（`QFileInfo::isDir()`），不存在则报错 `"项目目录不存在: <dir>"` 并放弃创建。会话目录、`meta.json`、**空的 `messages.jsonl`**（创建即 truncate 一个 0 字节文件，`src/SessionStore.cpp:461-464`）三者在创建时一次落定。

## 4. sessions/<uuid>/messages.jsonl 逐字段规格

每行一条 compact JSON 对象 + `\n`，空行跳过，损坏行当空对象处理（`src/SessionStore.cpp:1736-1741`）。行序 = 时间序（append-only，见 §4.4）。

### 4.1 消息行字段

加载逻辑 `loadMessagesInto()`（`src/SessionStore.cpp:1731-1781`），写入逻辑见 §4.4。

| 字段 | 类型 | 含义 | 缺省 |
|---|---|---|---|
| `id` | string | 消息 UUID | `""` |
| `role` | string | `"user" \| "assistant"`（无其它取值） | `""` |
| `content` | string | 正文全文。流式中逐步累积；pending 占位行为 `"…"`（`src/ChatController.cpp:1021`） | `""` |
| `createdAt` | number | ms 时间戳 | `0` |
| `provider` | string | provider 快照 | `""` |
| `status` | string | 见 §4.2 枚举 | `"done"`（**缺字段时默认 done**，`src/SessionStore.cpp:1748`） |
| `thinking` | string | 思考过程全文（与 thinking segments 同源冗余） | `""` |
| `toolCalls` | array | 工具调用对象数组（见 §4.3），与 tool segments 同源冗余 | `[]` |
| `segments` | array | **按到达顺序**的段落流（见 §4.3）。**可能整个键缺失**（见下） | `[]` |
| `attachments` | array | 用户消息附件：本地文件绝对路径字符串数组（`src/ChatController.cpp:983-989` 收集，原样落盘） | `[]` |

**`segments` 键缺失是常态而非异常**：`appendMessageTo()` 追加行时**不写 `segments` 键**（只写 id/role/content/createdAt/provider/status/thinking/toolCalls/attachments，`src/SessionStore.cpp:719-728`）；只有经过 `rewriteMessagesFile()` 全量重写的行才有 `segments`（`src/SessionStore.cpp:766-777`）。加载时若 `segments` 为空数组/缺失，必须按**遗留合成规则**重建（`src/SessionStore.cpp:1756-1776`），顺序固定为 thinking → text → tools：

1. `thinking` trim 后非空 → 合成 `{kind:"thinking", text: <thinking>}`；
2. `content` 非空且 ≠ `"…"` → 合成 `{kind:"text", text: <content>}`；
3. `toolCalls` 每项 → 浅拷贝加 `kind:"tool"`。

另一个遗留清洗（加载和搜索都做）：`content` 长度 >1 且以 `"…"` 开头 → 删掉首字符（`src/SessionStore.cpp:1752-1755` 与 `1593-1594`。历史 bug：占位符没被清掉就 append 正文）。

### 4.2 status 枚举

消息级 `status`：

| 值 | 含义 |
|---|---|
| `pending` | assistant 占位行，回复尚未开始流入（发送时写入，`src/ChatController.cpp:1021-1022`）。崩溃残留可长期存在；渲染为占位，transcript/复制跳过（`src/SessionStore.cpp:1509-1510`） |
| `streaming` | 流式进行中（`src/SessionStore.cpp:885, 924, 992`）。正常回合结束会被 flush 覆盖；崩溃后可能残留，**加载方要把它当 interrupted 展示**（旧版不特殊处理，新版建议兜底） |
| `done` | 正常完成（默认） |
| `error` | 回合失败（`src/ChatController.cpp:423-424`） |
| `interrupted` | 用户取消 / 进程中断（`src/ChatController.cpp:421-422`）。空内容中断时正文被写成 `"（已中断）"`（`src/ChatController.cpp:427-428`）；done 但正文仍是 `"…"` 时改写成 `"（空回复）"`（`src/SessionStore.cpp:1030-1033`） |

user 行恒为 `done`（append 默认值，`src/SessionStore.h:86`）。

### 4.3 segments 与 toolCalls 结构

`segments` 是 `{kind: "thinking"|"text"|"tool", ...}` 对象的**时间顺序数组**（`src/SessionStore.cpp:131-133`）：

- `{ "kind": "thinking", "text": "<累积思考文本>" }` —— 连续 thinking chunk 合并延伸末段（`src/SessionStore.cpp:913-922`）
- `{ "kind": "text", "text": "<累积正文>" }` —— 连续 text chunk 合并延伸末段（`appendTextSegment`，`src/SessionStore.cpp:785-798`）
- `{ "kind": "tool", "toolCallId": "...", ... }` —— 工具调用段 = ACP `tool_call`/`tool_call_update` payload 的**归一化透传拷贝**加 `kind:"tool"`，按 `toolCallId` 原位合并更新（非 null 字段覆盖，`src/SessionStore.cpp:970-990`）

tool 段 / `toolCalls` 元素的键集（来自 ACP update + `toolFromUpdate` 归一化，`src/ChatController.cpp:466-483`）：

| 键 | 类型 | 说明 |
|---|---|---|
| `toolCallId` | string | 必填，合并主键（空则整条丢弃，`src/SessionStore.cpp:949-951`） |
| `name` | string | 归一化产物：原 `title` → 否则 `kind`（ACP 的 tool kind） |
| `title` | string | ACP 原始标题（可缺） |
| `kind` | string | 段类型 `"tool"`；在 toolCall 原始语义里是 ACP tool kind（`"read"｜"edit"｜"execute"｜"other"…`），落盘后二者共存于同一 map，**段级 kind 恒为 "tool"**（`src/SessionStore.cpp:981, 988` 会在合并后强制写回） |
| `status` | string | `"pending"｜"in_progress"｜"completed"｜"failed"` |
| `content` | array | ACP content blocks（`[{type:"content", content:{type:"text", text:"..."}}, ...]`），累积快照 |
| `rawInput` / `rawOutput` | any | ACP 原始输入/输出，可能很大（新版内存截断策略见 §11，**磁盘上保留全量**） |
| `locations` | array | ACP 文件位置信息（可缺） |
| 其它 | any | ACP 未来字段直接透传，读取方忽略 |

示例 messages.jsonl（2 行；实际为一行一条，此处格式化展示）：

```jsonl
{"attachments":["C:\\Users\\me\\AppData\\Roaming\\WarDex\\media\\2026-07-27\\3f2c9a1e\\paste-20260727-101530-123.png"],"content":"看下这张图里的布局问题","createdAt":1753689000000,"id":"a1…","provider":"kimi","role":"user","status":"done","thinking":"","toolCalls":[]}
{"attachments":[],"content":"我先读一下文件。\n\n问题在 Repeater 每次 flush 全量重建。","createdAt":1753689005000,"id":"b2…","provider":"kimi","role":"assistant","segments":[{"kind":"thinking","text":"用户给了张截图，先看相关代码…"},{"kind":"text","text":"我先读一下文件。"},{"kind":"tool","name":"Read","status":"completed","title":"Read","toolCallId":"call_01HQ…"},{"kind":"text","text":"\n\n问题在 Repeater 每次 flush 全量重建。"}],"status":"done","thinking":"用户给了张截图，先看相关代码…","toolCalls":[{"name":"Read","status":"completed","title":"Read","toolCallId":"call_01HQ…"}]}
```

### 4.4 读写策略：append vs 全量重写

两条写路径，**时机严格区分**（这是磁盘格式正确性的核心）：

1. **追加写（append）**：仅 `appendMessageTo()` 用（`src/SessionStore.cpp:730-737`）——新消息行（user 行、assistant 占位行）以 `Append` 模式追加一行 compact JSON。**不含 `segments` 键**。
2. **全量重写（`rewriteMessagesFile()`）**：`src/SessionStore.cpp:758-782`——truncate 后把内存模型**每一行**（含 `segments`）整体写回。触发时机：
   - 回合结束 `flushLastAssistantTo()`（`src/SessionStore.cpp:1035`）——正常路径，**唯一会把 segments 落盘的常规时机**；
   - `updateLastAssistantTo()`（错误替换、限流重试提示、`（已中断）` 写入，`src/SessionStore.cpp:860`）；
   - `setLastAssistantFieldsTo()`（`src/SessionStore.cpp:1014`）。
3. **流式分片不落盘**：`appendLastAssistantContentTo` / `appendLastAssistantThinkingTo` / `upsertLastAssistantToolTo` 只改内存模型，一次磁盘 I/O 都没有（`src/SessionStore.cpp:866-997`；审计报告确认 `memory-audit-streaming.md` §"磁盘 I/O"）。**进程在流式中途被杀时，磁盘上该 assistant 行仍是追加时的占位形态（content="…", status="pending"）——这是兼容数据，不是损坏。**

推论给新版：

- 不要因为"内存策略变了"就改成流式落盘；保持"分片纯内存、flush 整体重写"的写放大模型，否则中断语义（占位行残留）会变。
- 全量重写是 O(历史长度)，每回合至多几次，可接受；但要保证重写时**段序与内存一致**（按模型行序逐行写）。

## 5. agents/ 持久化

### 5.1 agents/index.json

写盘 `saveIndex()`（`src/AgentStore.cpp:491-503`），加载 `loadFromDisk()`（`src/AgentStore.cpp:440-489`）。

| 字段 | 类型 | 含义 |
|---|---|---|
| `defaultAgentId` | string | 默认 Agent 的 id；空串 = 无 |
| `agents` | array&lt;string&gt; | Agent id 列表，**顺序即 UI 列表顺序** |

```json
{
    "agents": [
        "8a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
        "5e6f7a8b-9c0d-4e1f-2a3b-4c5d6e7f8a9b"
    ],
    "defaultAgentId": "8a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"
}
```

加载时的两条兼容规则（`src/AgentStore.cpp:456-488`）：

- **孤儿文件拾取**：`agents/` 下除 `index.json` 外的所有 `*.json` 都被当作 Agent 文件，id = 文件名去 `.json`；不在 index 列表里的 id 追加到列表尾部。
- **`isDefault` 以 index 为准**：Agent 文件里虽然也有 `isDefault` 字段，但加载时强制 `isDefault = (id == defaultAgentId)`（`src/AgentStore.cpp:475`）；`defaultAgentId` 为空且存在 Agent 时，自动选第一个 `providerSupportsChat()` 的（当前等价于 provider == "kimi"，见 providers-and-cli.md）。

### 5.2 agents/<id>.json

序列化 `toJson()`/`fromJson()`（`src/AgentStore.cpp:515-553`）。

| 字段 | 类型 | 含义 | 缺省 |
|---|---|---|---|
| `id` | string | Agent UUID = 文件名 | 文件名（`a.id.isEmpty()` 时用文件名回填，`src/AgentStore.cpp:473-474`） |
| `name` | string | 显示名 | `""`（新建默认 `"新 Agent"`，`src/AgentStore.cpp:110`） |
| `provider` | string | `"kimi" \| "claude" \| "codex" \| "custom"`，小写 | `"kimi"` |
| `isDefault` | bool | 见上：以 index.json 为准，此字段是冗余快照 | `false` |
| `enabled` | bool | 停用后不参与会话 | `true` |
| `model` | string | 模型名（新建默认 `"moonshot-v1-auto"`，`src/AgentStore.cpp:112`） | `""` |
| `baseUrl` | string | 自定义 API base | `""` |
| `cliPath` | string | CLI 绝对路径；空 = 自动探测/用 provider 默认命令 | `"kimi"`（**注意：文件缺字段时旧代码回填 "kimi"**，`src/AgentStore.cpp:525`；新建内存值是空串） |
| `apiKey` | string | **明文** API key | `""` |
| `extraArgs` | string | 追加 CLI 参数（空格分隔，`QProcess::splitCommand` 解析） | `""` |
| `mcpServers` | string | **JSON 数组文本**（不是嵌套对象！原样透传给 ACP session/new，`src/AgentStore.h:86-88`） | `""` |
| `avatarPath` | string | 自定义头像绝对路径；空 = 内置默认 | `""` |
| `createdAt` / `updatedAt` | number | ms 时间戳 | `0` |

```json
{
    "apiKey": "sk-xxxxxxxxxxxxxxxx",
    "avatarPath": "",
    "baseUrl": "",
    "cliPath": "C:\\Users\\me\\AppData\\Roaming\\npm\\kimi.cmd",
    "createdAt": 1753500000000,
    "enabled": true,
    "extraArgs": "",
    "id": "8a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
    "isDefault": true,
    "mcpServers": "",
    "model": "kimi-for-coding",
    "name": "Kimi 主 Agent",
    "provider": "kimi",
    "updatedAt": 1753600000000
}
```

更新语义（`updateAgent`，`src/AgentStore.cpp:136-180`）：trim 各字符串；`provider` 额外转小写；**`apiKey` 为空串或含 `*` 时保留旧值**（UI 回传掩码 key 的保护，掩码规则 `maskKey()`：`size<=8 → "********"`，否则 `left(3)+"****"+right(4)`，`src/AgentStore.cpp:586-592`）；每次 update 都重写该 Agent 文件 + index.json 并刷 `updatedAt`。删除 = 删文件 + 从 index 移除；删的是默认 Agent 时列表首个接任默认（`src/AgentStore.cpp:182-207`）。`setDefault` 会把所有 Agent 文件的 `isDefault` 重写一遍（`src/AgentStore.cpp:220-227`）。

## 6. projects.json

存储 `src/ProjectStore.cpp:188-207`（save）/ `158-186`（load）。

```json
{
    "aliases": {
        "C:/workspace/WarDex": "魔兽助手旧版"
    },
    "recent": [
        {
            "lastOpenedAt": 1753690000000,
            "path": "C:/workspace/WarDex"
        },
        {
            "lastOpenedAt": 1753600000000,
            "path": "C:/workspace/Wardex-rust"
        }
    ]
}
```

规则（全部有兼容意义）：

- `recent` 数组**最新在前，上限 8 条**（`kMaxRecent`，`src/ProjectStore.h:19`）。`touchProject` 去重后 prepend、超出截尾（`src/ProjectStore.cpp:75-95`）；加载时也只取前 8 条（`src/ProjectStore.cpp:182-184`）。条目只有 `path` + `lastOpenedAt` 两字段，**没有 alias 字段**。
- `aliases` 是**独立的 map**：canonical path → 显示别名。故意与 recent 分离存储，使项目掉出 recent 后别名仍存活（`src/ProjectStore.h:9-10`）。别名 trim 后截 24 字符；空串 = 删除该键（`src/ProjectStore.cpp:102-108`）。加载时丢弃空值别名（`src/ProjectStore.cpp:166-170`）。显示名回退：别名 → 目录 basename → 整个路径的原生分隔符形式（盘符根目录如 `C:/` 没有 basename 的情况，`src/ProjectStore.cpp:37-43, 120-128`）。

### 6.1 canonical path 规则（多处共用，务必一致）

`canonicalDir()`（`src/ProjectStore.cpp:69-73`）= `QDir::cleanPath(QDir(dir).absolutePath())`。等价语义：

1. 转绝对路径（基于进程 cwd 解析相对路径）；
2. 解析 `.`/`..`、折叠重复分隔符、**统一为正斜杠**；
3. 去掉尾部 `/`（盘符根 `C:/` 除外）。

去重/匹配一律**大小写不敏感**比较（Windows 语义）：`touchProject`/`removeProject`/`setAlias` 里都是 `Qt::CaseInsensitive`（`src/ProjectStore.cpp:82, 111, 136`）。`sessionsForProject` 的匹配同样大小写不敏感（`src/SessionStore.cpp:1153`）。**注意：aliases map 的键区分大小写插入**（`QHash` 精确匹配，`src/ProjectStore.cpp:108`）——同一目录大小写不同会产生两条别名键，这是旧行为的原样保留；新版若规范化键大小写，读取旧文件时先原样加载即可，不要主动"清理"。

## 7. user_prefs.json 与用户头像

### 7.1 user_prefs.json

存储 `src/UserPrefs.cpp:170-182`（save）/ `152-168`（load）。

| 字段 | 类型 | 含义 | 缺省 / 钳制 |
|---|---|---|---|
| `permissionMode` | string | ACP 权限模式。白名单 `"default"｜"plan"｜"auto"｜"yolo"`，白名单外一律回落 `"default"`（`src/UserPrefs.cpp:40-54`） | `"default"` |
| `userAvatarPath` | string | 自定义头像绝对路径。**加载时校验文件存在**，不存在则尝试回退到固定路径 `<root>/user_avatar.png`（`src/UserPrefs.cpp:163-167`） | `""` |
| `userName` | string | 聊天里用户侧显示名；trim 截 24 字符；**空 → 显示 `"阿尔萨斯"`**（`src/UserPrefs.cpp:56-69`） | `""` |
| `previewWidth` / `previewHeight` | number(int) | 文件预览对话框上次尺寸；`0 = 未设置`（QML 用 A4 默认），非 0 钳制到 `[320, 4096]`（`src/UserPrefs.cpp:72-75`） | `0` |
| `fontScale` | number | 全局字体缩放，钳制 **[0.85, 1.30]**，默认 1.0（`src/UserPrefs.cpp:98-111, 162`）。UI 档位 85%/100%/115%/130% | `1.0` |
| `panelLayout` | object | **新版新增**（旧版无此字段，读取时容忍缺失）。面板坞布局记忆：`{ <panelId>: { "open": bool, "height": px } }`，拖拽结束后 300ms 防抖写盘。规范见 [panels.md](./panels.md) §1.2 | `{}` |

```json
{
    "fontScale": 1.15,
    "permissionMode": "auto",
    "previewHeight": 0,
    "previewWidth": 0,
    "userAvatarPath": "C:\\Users\\me\\AppData\\Roaming\\WarDex\\user_avatar.png",
    "userName": "阿尔萨斯"
}
```

### 7.2 user_avatar.png

`setUserAvatarFromFile()`（`src/UserPrefs.cpp:113-140`）：读图 → **中心裁剪成正方形** → 缩放 **128×128** → 存 PNG 到固定路径 `<root>/user_avatar.png`，然后把该路径写进 `userAvatarPath`。`clearUserAvatar()` 删文件并清空字段（`src/UserPrefs.cpp:142-150`）。接受 `file:` URL 输入（先转本地路径）。

## 8. todos.json 与 prompts.json

### 8.1 todos.json

存储 `src/TodoStore.cpp:176-194`（save）/ `155-174`（load）。

```json
{
    "todos": [
        {
            "createdAt": 1753600000000,
            "done": false,
            "doneAt": 0,
            "id": "c7d8e9f0-a1b2-4c3d-8e4f-5a6b7c8d9e0f",
            "title": "验证旧会话能打开"
        }
    ]
}
```

- 顶层仅 `todos` 数组。条目字段：`id`(string, UUID)、`title`(string)、`done`(bool)、`createdAt`(number ms)、`doneAt`(number ms，**未完成 = 0**；toggle 回未完成时归零，`src/TodoStore.cpp:107-108`)。
- 加载校验：`id` 或 `title` 为空的条目丢弃（`src/TodoStore.cpp:171-172`）。
- 排序不落盘：UI 的 pending/done 两个视图在内存中分别按 `createdAt` desc / `doneAt` desc 排（`src/TodoStore.cpp:135-148`）；磁盘顺序 = 插入顺序。

### 8.2 prompts.json

存储 `src/PromptStore.cpp:161-178`（save）/ `111-159`（load）。

```json
{
    "prompts": [
        {
            "createdAt": 1753500000000,
            "id": "d8e9f0a1-b2c3-4d4e-9f5a-6b7c8d9e0f1a",
            "name": "代码审查",
            "text": "请审查以下代码，指出潜在的 bug、边界条件问题和可改进点，并给出具体的修改建议：\n"
        }
    ]
}
```

- 顶层仅 `prompts` 数组。条目字段：`id`、`name`、`text`、`createdAt`。加载校验：`id` 或 `text` 为空丢弃（`src/PromptStore.cpp:130-131`）。
- **首启种子机制**（`src/PromptStore.cpp:113-156`）：**仅当文件从未存在过**（`!QFileInfo::exists`）且解析结果为空时，写入 3 条内置中文模板并立即落盘；用户把模板删光后文件仍存在（空数组），**不会**再种子。三条种子的 `name`/`text` 必须逐字一致（`text` 带尾部 `\n`，设计上用于前缀拼接代码）：
  1. `代码审查` — `请审查以下代码，指出潜在的 bug、边界条件问题和可改进点，并给出具体的修改建议：\n`
  2. `解释代码` — `请逐段解释以下代码的作用、实现思路和关键细节：\n`
  3. `重构建议` — `请分析以下代码的结构，在保持行为不变的前提下给出具体的重构方案：\n`
- 新增时 `name` 为空 → 回退为 `text` 首行截 20 字符（`src/PromptStore.cpp:77-81`）。

## 9. media/ 缓存与剪贴板图片落盘

### 9.1 目录规则（`src/AppPaths.cpp:54-69`）

`media/<yyyy-MM-dd>/<sessionId>/`；sessionId 为空 → `"no-session"`；sessionId 先剔除 `/` 和 `\`（`src/AppPaths.cpp:63-67`）。日期取**本地当前日期**。

### 9.2 清理策略（`pruneMedia`，`src/AppPaths.cpp:71-85`）

- 启动时由 `ensureLayout()` 调用一次（`src/AppPaths.cpp:120`）。
- 只处理**名字能按 `yyyy-MM-dd` 解析**的子目录；`d.daysTo(today) > 14`（即目录日期早于 14 天前，**严格大于**，第 14 天当天不删）→ 整个日期目录递归删除。其它一切条目不动。
- UI 另有"清空缓存"入口 `clearMediaCache()`：直接整个 `media/` 递归删除（`src/ClipboardHelper.cpp:87-92`）。
- **没有**按会话删除联动（删会话不删其 media；代码注释明确这是"later pass"，`src/AppPaths.h:25-26`）。新版保持不联动。

### 9.3 剪贴板图片落盘规则（`saveClipboardImage`，`src/ClipboardHelper.cpp:44-85`）

输入：剪贴板位图 + sessionId。输出：落盘文件的**原生分隔符绝对路径**（写进消息 `attachments`），失败返回空串。

1. 目标目录 `mediaDirFor(sessionId)`，不存在则 mkpath。
2. 文件名 `paste-yyyyMMdd-HHmmss-zzz.png`（毫秒后缀防同秒覆盖，`src/ClipboardHelper.cpp:56-59`）。
3. **先存 PNG**；文件 ≤ **2 MB**（`kMaxImageBytes = 2*1024*1024`，`src/ClipboardHelper.cpp:15`）→ 完成。
4. 超过 2MB：若原图任一边 > **1920**（`kMaxImageSide`，`src/ClipboardHelper.cpp:16`）→ 等比缩到 1920 以内重存 PNG；仍超 2MB 继续下一步。
5. 降级 JPEG：有 alpha 先转 RGB32 拍平；按质量 **90 → 75 → 60 → 45** 依次尝试 `paste-<同一时间戳>.jpg`，第一个 ≤2MB 的胜出，删 PNG 收工。
6. 全部质量仍超 2MB：**保留最后一次 JPEG 尝试**（最小的一份），删 PNG，返回 jpg 路径；jpg 写失败才返回空。

## 10. 全文搜索规格（searchMessages）

实现参照 `src/SessionStore.cpp:1538-1658`，QML 契约注释 `src/SessionStore.h:130-145`。

- **范围**：全部会话的 `messages.jsonl`（含未打开会话，直接读文件，线程池执行）；只匹配 `role ∈ {user, assistant}` 行的 `content` 字段 + 会话 `title`。thinking / toolCalls / segments **不参与**匹配。
- **代际计数器（generation）**：每次调用 `++gen` 并立即返回新 gen；worker 在每处理一个会话前和投递结果前各检查一次 `genCounter == gen`，不等即作废（`src/SessionStore.cpp:1577, 1650-1654`）。空 query = 取消在途搜索并立刻回空结果（`src/SessionStore.cpp:1544-1549`）。新版用 `Arc<AtomicU64>` 实现同一语义。
- **匹配**：大小写不敏感的子串匹配，**在原始字符串上取 index**（不做 case folding，避免 ß→ss 之类长度变化导致 snippet 偏移错位，`src/SessionStore.cpp:1597-1600`）。
- **清洗**：与加载一致的 `"…"` 开头清洗（§4.1）；空 content / 纯 `"…"` 的 pending 占位行跳过（`src/SessionStore.cpp:1593-1596`）。
- **snippet**：命中点前后各 40 字符上下文（`kSnippetContext = 40`），首尾被截断时补 `"…"`（`src/SessionStore.cpp:1603-1608`）。
- **配额**：每会话最多 3 条命中（`kMaxHitsPerSession = 3`），**新的在前**（JSONL 是时间序，倒序取）；会话按 `updatedAt` desc 顺序遍历；总数 ≤ `maxResults`（默认 50）。
- **标题命中**：title 匹配但正文无命中时也出一条 `titleOnly: true` 的结果（snippet 用标题、`hitCount: 0`、`timestamp` 用 `updatedAt`，`src/SessionStore.cpp:1617-1628`）。
- **结果项字段**（每条）：
  | 字段 | 类型 | 说明 |
  |---|---|---|
  | `sessionId` | string | |
  | `sessionTitle` | string | |
  | `projectDir` | string | |
  | `snippet` | string | 上下文片段或标题 |
  | `timestamp` | number | 命中消息的 `createdAt`（titleOnly 时为会话 `updatedAt`） |
  | `updatedAt` | number | 会话 updatedAt |
  | `hitCount` | number | 该会话**全部**命中数（可 >3，虽然只展示 3 条） |
  | `titleOnly` | bool | |

  投递语义：完成时回调 `(generation, results)`；Rust 侧对应 `emit("search://results", {generation, results})`（事件名以 architecture.md 为准）。

## 11. 工作区文件访问与 @引用

工作区根 = §3 的 workspacePathFor 解析结果。以下功能都是**运行时读盘**，不改变磁盘格式，但其行为（忽略集、上限、错误枚举）是用户可见契约，需逐条复刻。

### 11.1 忽略集与扩展名分类（三处共用同一常量）

忽略名（`FileListModel::isIgnoredName`，`src/SessionStore.cpp:256-265`）：`.git`、`node_modules`、`build`、`dist`、`.venv`、`__pycache__`、`.qt`、`.rcc`，以及**任何以 `.git` 开头的名字**。

- 图片扩展名（`imageExtensions`，`src/SessionStore.cpp:1229-1236`）：`png jpg jpeg gif webp bmp`
- 二进制扩展名（`binaryExtensions`，`src/SessionStore.cpp:1238-1250`）：`ico zip 7z rar gz exe dll pdf mp3 mp4 wav ogg blp mpq mdx glb bin dat db so dylib`

不在两个列表里的扩展名 = 按文本候选，但会再做 **NUL 字节嗅探**（含 `\0` 即判二进制）。扩展名比较一律小写化。

### 11.2 workspaceFileList（@ 选择器数据源，`src/SessionStore.cpp:1333-1369`）

- 从工作区根做 **DFS**（手动栈，保证忽略目录不下钻）；每层内按名字大小写不敏感排序。
- 跳过忽略名、图片/二进制扩展名文件。
- 返回**相对于工作区根的路径**（正斜杠），`filter` 非空时对相对路径做大小写不敏感子串过滤。
- 上限 `maxResults`（默认 200），达到即停。

### 11.3 readFileRange（发送时 @引用展开，`src/SessionStore.cpp:1371-1453`）

- **路径逃逸拒绝**：`relPath` 为绝对路径，或 clean 后的绝对结果不在 `rootAbs` 前缀内（`abs != rootAbs && !abs.startsWith(rootAbs + '/')`，**大小写不敏感**）→ `{ok:false, error:"escape"}`（`src/SessionStore.cpp:1380-1390`）。符号链接逃逸明确不设防（本地用户自输入场景，注释 `src/SessionStore.cpp:1381-1383`）。
- 读取上限 **200 KB**（`kMaxRefBytes = 200*1024`，`src/SessionStore.cpp:1403`）；超出时 `truncated: true` 且**丢弃最后一条可能截断的行**（`src/SessionStore.cpp:1416-1418`）。
- 二进制判定：图片/二进制扩展名 或 头部含 NUL → `{ok:false, error:"binary"}`。
- 编码：先按 UTF-8 解码；出现 U+FFFD 替换符则回退系统本地编码（旧版 `QString::fromLocal8Bit`，中文 Windows 即 GBK，`src/SessionStore.cpp:1411-1414`。Rust 侧建议同款回退：UTF-8 失败 → GBK（`encoding_rs` 的 GBK））。
- 行号语义（1-based）：`from <= 0` → 整个文件（仍带行号）；`from > 0 && to <= 0` → 单行 `from`；`to < from` → 钳到 `from`；`from > totalLines` → `{ok:false, error:"range"}`；`to` 钳到 `totalLines`（`src/SessionStore.cpp:1423-1438`）。
- 行文本按 `\n` 切分，尾部 `\r` 剥掉（`src/SessionStore.cpp:1444-1447`）。
- 成功返回 `{ok:true, lines:[{n, text}...], totalLines, truncated}`；错误返回 `{ok:false, error}`，error ∈ `"escape" | "missing" | "unreadable" | "binary" | "range"`（`missing` 也用于工作区根为空，`src/SessionStore.cpp:1376-1378`）。

### 11.4 previewFile / savePreviewText（工作区文件预览，`src/SessionStore.cpp:1253-1331`）

`previewFile(path)`（入参为**绝对路径**，来自工作区树的选择，不做 root 包含检查）：

- 不是文件 → `{ok:false, size:0, reason:"missing"}`；
- 图片扩展名 → `{ok:true, size, image:true}`（**不读内容**，前端直接按文件路径渲染）；
- 打不开 → `{ok:false, size, reason:"unreadable"}`；
- 读前 **256 KB**（`kMaxPreview = 256*1024`，`src/SessionStore.cpp:1277`）；二进制扩展名或头部含 NUL → `{ok:false, size, reason:"binary"}`；
- 否则 `{ok:true, size, image:false, text, truncated}`，`truncated = (文件大小 > 256KB)`；编码同样 UTF-8 → 本地编码回退。

`savePreviewText(path, content)`：拒绝不存在的文件（error `"文件不存在"`）、图片/二进制扩展名（error `"二进制文件不可编辑"`）、前 4096 字节含 NUL（同上 error）；通过则 **UTF-8 整体覆写**（truncate）。返回 `{ok:true}` 或 `{ok:false, error}`（中文错误串，逐字保持）。

### 11.5 gitBranchFor（直读 .git/HEAD，`src/SessionStore.cpp:1455-1488`）

不 spawn 进程：

1. `<dir>/.git` 是目录 → 读 `.git/HEAD`；是文件（worktree/submodule gitfile）→ 解析 `gitdir: <path>` 行（相对路径基于 dir 解析），读其下的 `HEAD`；都不是 → 返回 `""`。
2. HEAD 内容 trim 后：`ref: refs/heads/<branch>` → 返回 `<branch>`；其它 `ref: <x>` → 返回 `<x>`；否则（detached）→ 返回前 7 字符短 SHA。

### 11.6 GitStore（提交历史）—— 无磁盘格式

`src/GitStore.cpp:84-178`：spawn `git -C <dir> -c i18n.logOutputEncoding=UTF-8 log --pretty=format:%H%x1f%an%x1f%ad%x1f%s --date=format-local:%Y-%m-%d %H:%M -n <maxCount>`，按 `\x1f` 切分列。纯运行时数据，**不落盘**；列出仅为说明持久化层不涉及它。Rust 侧可用 `std::process::Command` 照搬命令行（含零提交仓库的 exit 128 / "does not have any commits" 特判，`src/GitStore.cpp:151-158`）。

## 12. 新版内存驻留策略变化（磁盘格式不变）

以下变化**只改内存表示与渲染路径，磁盘格式与读写时序完全不变**（§1-§10 依然逐字有效）。动机与出处见旧仓库审计报告 `memory-audit-streaming.md`（P1~P4）。

1. **segments 单一数据源（对应 P2）**：旧版 `content`/`thinking`/`toolCalls` 与 segments 三处冗余存储全文。新版内存中 **segments 为唯一权威**；`content`/`thinking` 在需要时（搜索、transcript、meta.summary、兼容写盘）由 segments 拼接派生。**写盘仍写全字段**（`content`/`thinking`/`toolCalls` + `segments`），与旧文件形态一致，旧版能继续读新版写的文件。
2. **会话模型 LRU 淘汰（对应 P3）**：旧版 `m_open` 只增不减。新版内存中最多保留 N 个已加载会话消息模型（建议 N = 当前会话 + 并行 runtime 数，上限对齐 `kMaxParallelAcp = 3`，`src/ChatController.h:50`），超出即卸载，需要时从 JSONL 重读。卸载前若有未落盘流式分片须先按 §4.4 完成重写。
3. **工具 payload 64KB 截断（对应 P4）**：内存模型的 tool 段 `rawInput`/`rawOutput`/`content` 块截断到 64 KB；**磁盘保留全量**——即 flush 写盘时要用未截断的完整 payload（流式累积缓冲持有），或接受"截断后落盘"作为刻意的行为变更（会丢历史数据，**不推荐**；推荐前者：接收侧先存完整副本用于落盘，截断副本用于渲染）。`tool_call_update` 的 content 块按"最后一个累积块优先"解析，不再做全量拼接（`memory-audit-streaming.md` P4）。
4. **流式增量渲染（对应 P1）**：旧版的 50ms/250ms 自适应 flush（`src/ChatController.cpp:361, 369`）与"分片不落盘"时序保留；前端改增量 append（见 architecture.md / features/chat.md），不属本文档范围。

## 13. 新仓库目标模块布局建议

`C:/workspace/Wardex-rust/src-tauri/src/store/` 下分文件（与既有 `src-tauri/src/{acp,chat,store}` 骨架对齐）：

```
src-tauri/src/store/
├── mod.rs          # 对外 re-export；StoreRegistry（各 store 的单例持有与启动初始化顺序）
├── paths.rs        # root() / agents / sessions / media / logs / crashes 路径；dev/release 目录名判别（§1）；
│                   # ensure_layout()（mkpath 全部 + prune_media）；canonical_dir()（§6.1）
├── session.rs      # meta.json 读写、messages.jsonl append/rewrite、load 清洗与遗留合成（§3-§4）、
│                   # sessions 索引扫描 + discard_empty_sessions（§2）、LRU 模型缓存（§12.2）
├── search.rs       # 全文搜索 worker + 代际计数器（§10）
├── workspace.rs    # 忽略集/扩展名常量、workspace_file_list、read_file_range、preview_file、
│                   # save_preview_text、git_branch_for（§11）
├── agent.rs        # agents/index.json + agents/<id>.json、孤儿拾取、默认 Agent 规则（§5）
├── project.rs      # projects.json、recent ≤8、aliases（§6）
├── prefs.rs        # user_prefs.json、钳制规则、user_avatar.png 处理（§7）
├── todo.rs         # todos.json（§8.1）
├── prompt.rs       # prompts.json + 首启种子（§8.2）
└── media.rs        # media_dir_for、prune_media(14d)、剪贴板图片落盘降级链（§9）
```

Serde 建议：所有持久化 struct 用 `#[serde(default)]` + 未知字段兜底（§0）；时间戳字段用自定义 `de` 同时接受整数/浮点 number 并归一为 `i64` 毫秒。

## 14. 实现检查清单

- [ ] `paths::root()`：debug → `%AppData%/WarDex-tauri-dev`，release → `%AppData%/WarDex`（§1.1）
- [ ] `ensure_layout()`：启动时 mkpath `agents/ sessions/ media/ logs/ crashes/` 并执行 14 天 media 清理（§1.2、§9.2）
- [ ] 会话索引：启动扫 `sessions/*/meta.json`，坏 meta 跳过，`updatedAt` desc 排序；`discard_empty_sessions`（meta 可读且 `messageCount==0` 才删）（§2）
- [ ] meta.json 全字段 serde（含可选 `pinned`、`acpSessionId`），4 空格缩进、键字母序整体重写；改名/置顶/换 Agent 不刷 `updatedAt`（§3、§3.1）
- [ ] 工作区路径解析优先级 `projectDir > workDir > sessions/<id>/workspace`（§3）
- [ ] messages.jsonl：每行 compact JSON + `\n`；空行/坏行容错；时间戳按 f64 解析（§0、§4.1）
- [ ] 追加行**不写 `segments` 键**；`rewriteMessagesFile` 全量重写才写；流式分片零磁盘 I/O（§4.4）
- [ ] 加载清洗：`content` 前导 `"…"` 剥离；`status` 缺省 `"done"`；`segments` 缺失时按 thinking→text→tools 合成（§4.1）
- [ ] status 枚举完整处理：`pending | streaming | done | error | interrupted`；`"（已中断）"` / `"（空回复）"` 占位语义（§4.2）
- [ ] tool 段按 `toolCallId` 非 null 字段合并，段级 `kind` 恒为 `"tool"`（§4.3）
- [ ] user 消息 `attachments` 为绝对路径字符串数组，原样透传（§4.1）
- [ ] 标题自动命名 `left(24)+"…"`、summary `left(80)+"…"`、手动改名 trim 截 48（§3）
- [ ] agents：index.json（defaultAgentId + agents 顺序）与 `<id>.json` 双写；孤儿文件拾取；`isDefault` 以 index 为准；缺省 provider `"kimi"` / cliPath `"kimi"` / enabled `true`（§5）
- [ ] apiKey 更新时空串/含 `*` 保留旧值；掩码 `left(3)+"****"+right(4)`（§5.2）
- [ ] projects.json：recent 上限 8、最新在前；canonical path 正斜杠；去重/匹配大小写不敏感；aliases 独立 map、别名截 24、空串删键（§6）
- [ ] user_prefs.json：permissionMode 白名单回落、fontScale 钳 [0.85,1.30]、preview 尺寸 0 或钳 [320,4096]、userName 截 24 回退 `"阿尔萨斯"`、userAvatarPath 存在性校验回退（§7）
- [ ] 用户头像：中心裁方 → 128×128 PNG → `<root>/user_avatar.png`（§7.2）
- [ ] todos.json / prompts.json：字段与加载校验一致；prompts **仅文件不存在时**种子 3 条逐字模板（§8）
- [ ] media：`<yyyy-MM-dd>/<sessionId|no-session>/`；sessionId 剥 `/\`；清理只删日期目录且 `>14` 天；不做会话删除联动（§9）
- [ ] 剪贴板图片：PNG → >2MB 缩 1920 → JPEG 90/75/60/45 阶梯 → 兜底保留最后一份 JPEG；文件名带毫秒（§9.3）
- [ ] 全文搜索：仅 user/assistant 的 content + title；原文取 index；snippet ±40；每会话 ≤3 新在前；`titleOnly` 分支；代际计数器作废过期结果（§10）
- [ ] workspaceFileList：DFS + 忽略集 + 跳过图片/二进制扩展名 + 相对路径子串过滤 ≤200（§11.2）
- [ ] readFileRange：逃逸拒绝（大小写不敏感前缀）、200KB 上限丢尾行、行号钳制与 `"escape"|"missing"|"unreadable"|"binary"|"range"` 错误枚举、`\r` 剥离、UTF-8→GBK 回退（§11.3）
- [ ] previewFile 256KB / savePreviewText 4096B NUL 嗅探与中文错误串（§11.4）
- [ ] gitBranchFor：`.git` 目录与 gitfile 两种形态、detached 短 SHA 7 位（§11.5）
- [ ] 内存策略：segments 单一数据源但**写盘仍全字段**；LRU 淘汰前完成落盘；工具 payload 截断仅限内存副本（§12）

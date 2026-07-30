# Provider 差异、Agent 配置模型与 CLI 探测规格

> 本文档是 WarDex Tauri 重写中 provider 注册表、Agent 持久化配置、CLI 探测三个子系统的
> 完整实现规格。所有行为以旧 C++/Qt 代码库（`C:/workspace/WarDex`）为准，文中以
> `文件:行号` 标注出处（如 `ProviderRegistry.cpp:30` 指 `src/ProviderRegistry.cpp` 第 30 行）。
> ACP 协议细节见 [acp-protocol.md](./acp-protocol.md)。

## 相关文档

- [架构总览](./architecture.md)
- [设计原则](./design-principles.md)
- [实施计划](./implementation-plan.md)
- [数据格式](./data-formats.md)
- [聊天功能](./features/chat.md)
- [ACP 协议客户端](./acp-protocol.md)

## 目标模块布局（建议）

```
src-tauri/src/
├── providers.rs        // ProviderSpec 静态注册表（对应旧 ProviderRegistry）
├── store/
│   ├── mod.rs
│   └── agents.rs       // AgentStore：Agent CRUD + 持久化 + testAgent
└── cli_probe.rs        // CliProbe：异步 CLI 探测与 --version 验证
```

---

## 1. ProviderSpec 模型

参照：`ProviderRegistry.h:13-32`。一条记录包含把一个 CLI 当作 ACP agent 运行所需的全部
信息；**新增一个 CLI 支持 = 在注册表加一条记录**，聊天管线本身 provider 无关。

```rust
pub struct ProviderSpec {
    pub id: &'static str,               // 稳定小写 id，存进 agents/sessions 数据
    pub display_name: &'static str,     // UI 显示名
    pub default_command: &'static str,  // agent.cliPath 为空时按 PATH 查找的可执行名
    pub acp_args: Vec<&'static str>,    // 让 CLI 进入 ACP stdio 模式的参数
    pub api_key_envs: Vec<&'static str>,// 注入 agent.apiKey 的环境变量名（全部注入同一值）
    pub base_url_envs: Vec<&'static str>, // 注入 agent.baseUrl 的环境变量名
    pub clear_envs: Vec<&'static str>,  // 启动前从子进程环境删除的变量（防嵌套守卫）
    pub bearer_token_env: &'static str, // 见 1.2 特例；空串 = 无此行为
    pub official_key_prefix: &'static str, // 见 1.2 特例
    pub base_url_hint: &'static str,    // 配置页展示的 Base URL 格式说明
    pub mode_map: HashMap<&'static str, &'static str>, // WarDex mode id → provider mode id
    pub install_hint: &'static str,     // 配置页展示的安装提示
    pub chat_capable: bool,             // 固定 true（保留字段，ProviderRegistry.h:31）
}
```

### 1.1 四个内置 provider 的逐项值

参照：`ProviderRegistry.cpp:3-85`。

| 字段 | kimi | claude | codex | custom |
|---|---|---|---|---|
| `id` | `kimi` | `claude` | `codex` | `custom` |
| `displayName` | `Kimi CLI` | `Claude Code` | `Codex CLI` | `自定义 (ACP)` |
| `defaultCommand` | `kimi` | `claude-code-acp` | `codex-acp` | （空） |
| `acpArgs` | `["acp"]` | `[]`（adapter 直接讲 ACP，无子命令） | `[]` | `[]` |
| `apiKeyEnvs` | `["KIMI_API_KEY", "OPENAI_API_KEY"]` | `["ANTHROPIC_API_KEY"]` | `["OPENAI_API_KEY"]` | `["OPENAI_API_KEY"]` |
| `baseUrlEnvs` | `["KIMI_BASE_URL", "OPENAI_BASE_URL"]` | `["ANTHROPIC_BASE_URL"]` | `["OPENAI_BASE_URL"]` | `["OPENAI_BASE_URL"]` |
| `clearEnvs` | `[]` | `["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT"]` | `[]` | `[]` |
| `bearerTokenEnv` | （空） | `ANTHROPIC_AUTH_TOKEN` | （空） | （空） |
| `officialKeyPrefix` | （空） | `sk-ant-` | （空） | （空） |
| `modeMap` | `{}`（恒等） | `{auto: acceptEdits, yolo: bypassPermissions}` | `{}` | `{}` |
| `installHint` | `安装见 https://www.kimi.com/code` | `npm i -g @zed-industries/claude-code-acp；API Key 留空则使用 claude /login 的本地凭据` | `npm i -g @zed-industries/codex-acp；API Key 留空则使用 codex login 的本地凭据` | `填写 CLI 路径与进入 ACP 模式的参数（如 acp 或 --experimental-acp）` |
| `baseUrlHint` | `OpenAI 兼容端点，通常以 /v1 结尾，如 https://api.kimi.com/coding/v1；留空使用本机登录态` | `Anthropic 格式根地址（走 /v1/messages 协议），如 https://api.anthropic.com 或中转的 Anthropic 兼容地址，结尾不要带 /v1/messages` | `OpenAI 兼容端点，以 /v1 结尾，如 https://api.openai.com/v1；仅支持旧式 chat 接口的中转需在 ~/.codex/config.toml 配 wire_api="chat"` | `按该 CLI 文档要求的根地址填写（注入 OPENAI_BASE_URL）` |

补充说明：

- **claude 的 clearEnvs**（`ProviderRegistry.cpp:34-38`）：Zed 的 claude-code-acp adapter
  拒绝"在另一个 Claude Code 会话内"运行；父进程（claude 或 WarDex 自身）泄漏进来的
  这三个会话标记变量必须删除。删除语义由 ACP 传输层的"null = 删除环境变量"约定实现
  （`ChatController.cpp:862-864` → `AcpClient.cpp:71-73`）。
- **claude 的凭据回退**（`ProviderRegistry.cpp:21-22`）：不提供 `ANTHROPIC_API_KEY` 时
  adapter 复用本机 `claude /login` 的凭据。kimi/codex 同理靠本机登录态。
- **custom 是逃生舱**（`ProviderRegistry.cpp:67-80`）：command 由用户的 `cliPath` 提供，
  args 由用户的 `extraArgs` 提供（此时 extraArgs 就是全部参数），无需改代码即可接任何
  讲 ACP 的 CLI。

### 1.2 claude 的 ANTHROPIC_AUTH_TOKEN 中转 key 特例

参照：`ProviderRegistry.h:22-25`、`ProviderRegistry.cpp:42-45`、注入逻辑
`ChatController.cpp:849-856` 与 `AgentStore.cpp:266-273`。

注入规则（在 ensureAcp 和 testAgent 两处各有一份，必须一致）：

```
if apiKey 非空:
    for name in apiKeyEnvs: env[name] = apiKey
    if bearerTokenEnv 非空 且 (officialKeyPrefix 为空 或 非 apiKey.startswith(officialKeyPrefix)):
        env[bearerTokenEnv] = apiKey
```

含义：中转（relay）key 需要走 `Authorization: Bearer`（即 `ANTHROPIC_AUTH_TOKEN`），
而官方 `sk-ant-…` key **只能**留在主 header（`x-api-key`，由 `ANTHROPIC_API_KEY` 注入），
绝不能同时塞进 `ANTHROPIC_AUTH_TOKEN`。对非 claude 的 provider `bearerTokenEnv` 为空，
此分支不生效。

### 1.3 注册表 API

参照：`ProviderRegistry.cpp:87-135`。

- `spec(id)`：id `trim + toLower` 后精确匹配，找不到返回 None。
- `ids()`：四个 id 列表（注册顺序：kimi, claude, codex, custom）。
- `chatCapable(id)`：spec 存在且 `chat_capable`。
- `mapMode(id, mode)`：`modeMap.get(mode).unwrap_or(mode)`——**未映射的 mode 原样透传**
  （恒等），未知 provider 也恒等（`ProviderRegistry.cpp:113-117`）。
- `specMap(id)`：给 UI 的视图，字段
  `{id, displayName, defaultCommand, acpArgs(空格 join 成单串), installHint, baseUrlHint, chatCapable}`。

---

## 2. Agent 数据模型与持久化

参照：`AgentStore.h:75-93`、`AgentStore.cpp:515-553`、路径 `AppPaths.cpp:29-34`。

### 2.1 Agent 字段

```rust
pub struct Agent {
    pub id: String,           // UUID 不带花括号（QUuid::WithoutBraces 格式）
    pub name: String,
    pub provider: String,     // kimi | claude | codex | custom
    pub is_default: bool,
    pub enabled: bool,        // 默认 true
    pub model: String,        // 如 "moonshot-v1-auto"；当前仅展示/记录用
    pub base_url: String,
    pub cli_path: String,     // 空 = 用 provider defaultCommand 按 PATH 解析
    pub api_key: String,      // 明文存储（见 2.4 mask 规则）
    pub extra_args: String,   // 追加在 acpArgs 后的额外参数（custom 时即全部参数）
    pub mcp_servers: String,  // 原始 JSON 数组文本；会话启动时才解析，非法则降级为空数组
    pub avatar_path: String,  // 自定义头像绝对路径；空 = 内置默认
    pub created_at: i64,      // epoch millis
    pub updated_at: i64,
}
```

新建 Agent 的默认值（`AgentStore.cpp:106-118`）：`name = "新 Agent"`，
`provider = "kimi"`，`model = "moonshot-v1-auto"`，`cliPath = ""`（留空让配置页
自动探测填绝对路径），`enabled = true`；第一个创建的 Agent 自动成为 default。

### 2.2 持久化格式

数据根目录（`AppPaths.cpp:11-34`）：Windows 下 `%APPDATA%\WarDex\`（即
`C:/Users/<user>/AppData/Roaming/WarDex`）；**开发版 exe 名为 `wardex-dev` 时用
`%APPDATA%\WarDex-dev\`**，与正式数据隔离。Tauri 版可用 `dirs::data_dir()` 拼同样结构。

```
<root>/agents/
├── index.json      // { "defaultAgentId": "...", "agents": ["<id>", ...] }（顺序即列表顺序）
└── <id>.json       // 每个 Agent 一个文件，字段即 2.1 的全部字段
```

- `<id>.json` 即 2.1 结构体的 JSON 序列化（旧代码用缩进格式 `QJsonDocument::Indented`，
  `AgentStore.cpp:502, 511`；Tauri 版用 `serde_json::to_string_pretty`）。
- `index.json` 的 `agents` 数组只存 id 字符串（`AgentStore.cpp:491-503`）。
- **加载容错**（`AgentStore.cpp:440-489`）：
  - index.json 缺失/损坏 → 当作空索引继续；
  - agents 目录里存在但不在索引中的 `*.json`（孤儿文件）**也要捡回来**追加进列表；
  - 单个 agent 文件打不开 → 跳过；
  - 加载后 `isDefault` 以索引的 `defaultAgentId` 为准重算；
  - 索引没有 defaultAgentId 且列表非空 → 挑第一个 `chatCapable` 的 provider 的 Agent
    设为默认（`AgentStore.cpp:479-488`）。
- `fromJson` 的字段缺省值（`AgentStore.cpp:515-533`）：`provider` 缺省 `"kimi"`，
  `enabled` 缺省 `true`，`cliPath` 缺省 `"kimi"`（历史遗留，注意与新建默认值 "" 不同），
  时间戳从 JSON double 转 i64。
- 删除 Agent：删 `<id>.json` 文件；若删的是默认 Agent，则列表第一个递补为默认
  （`AgentStore.cpp:182-207`）。
- `setDefault` 的限制：只允许 `chatCapable` provider（旧版报错文案
  "仅 Kimi 可用于默认对话（当前版本）"，`AgentStore.cpp:209-219`；
  重写时若放开多 provider 默认，此限制可移除，但要同步改 UI 文案）。

### 2.3 updateAgent 的 apiKey 保留规则

参照：`AgentStore.cpp:154-159`。

UI 表单回传的 `apiKey` 字段：**为空字符串或包含 `*`（即仍是 mask 后的占位串）→ 保留
旧值不变**；否则替换为新值。这保证用户编辑其他字段时不会意外清空已存的 key。
其余字段（name/provider/model/baseUrl/cliPath）更新时 trim；provider 还要 toLower。

### 2.4 apiKey mask 规则

参照：`AgentStore.cpp:586-593`。只在**展示**时 mask，存储始终明文：

```
key 为空        → ""
key.len() <= 8  → "********"（8 个星号，与长度无关）
否则            → key[..3] + "****" + key[len-4..]
```

注意旧代码按 UTF-16 code unit 计长；Rust 按 `chars().count()` 处理即可（API key
实际均为 ASCII）。

### 2.5 avatarFor

参照：`AgentStore.cpp:574-584`。`avatarPath` 非空且文件仍存在 → 返回 `file:///`
URL；否则返回空串，调用方回退到内置头像。id 为空时解析为默认 Agent。

---

## 3. 会话启动时的 env/args 组装（ensureAcp）

参照：`ChatController.cpp:837-906`。这是 ProviderSpec + Agent 配置合流的地方：

1. **env**：
   - `apiKey` 非空 → 按 1.2 节规则注入 `apiKeyEnvs` + 可能的 `bearerTokenEnv`；
   - `baseUrl` 非空 → 注入全部 `baseUrlEnvs`；
   - `clearEnvs` 里每个名字 → 插入 null 值（删除语义，见 acp-protocol.md 2.2）。
2. **command**：`cliPath` trim 后为空 → provider `defaultCommand`；再为空（custom 未填）
   → 启动时会被 AcpClient 拒（"未配置 CLI 命令"）。
3. **args**：`acpArgs` + `extraArgs`（trim 后非空时按 shell 规则 split——旧代码用
   `QProcess::splitCommand`，支持引号分组；Rust 可用 `shell-words` crate）。
   custom provider 的 `acpArgs` 为空，此时 `extraArgs` 就是全部参数
   （`ChatController.cpp:870-875`）。
4. **mcpServers**：`mcp_servers` 文本 trim 后非空则解析；**必须是合法 JSON 数组**，
   否则记警告日志并降级为空数组（不阻断会话）（`ChatController.cpp:877-894`）。
5. **cwd**：会话的 workspace 绝对路径，空则当前进程目录。
6. **mode**：`ProviderRegistry::mapMode(provider, permissionMode)` 翻译后传给 ACP 层
   （`ChatController.cpp:908-916`）。WarDex 侧 mode id 为
   `default | plan | auto | yolo`。

---

## 4. testAgent 连通性验证

参照：`AgentStore.cpp:236-411`。

### 4.1 前置条件

- 单飞行：`m_testing == true` 时直接忽略新请求（`AgentStore.cpp:237-238`）。
- Agent 不存在 → `testResult = "Agent 不存在"`。
- provider 非 `chatCapable` → `testResult = "该 Provider 暂不支持测试"`。

### 4.2 流程

1. 组装 env（同 1.2 节注入规则；**注意 testAgent 不注入 baseUrl 之外的差异——
   它确实也注入 baseUrlEnvs**，`AgentStore.cpp:274-277`；但不处理 clearEnvs）。
2. 解析 program：`cliPath` → provider `defaultCommand` → 兜底 `"kimi"`
   （`AgentStore.cpp:281-285`）。args = `acpArgs` + splitCommand(`extraArgs`)。
3. Windows `.cmd/.bat` 包 `cmd.exe /c`（与 ACP 传输层同一套逻辑，
   `AgentStore.cpp:292-305`）。
4. spawn；**进程一进入 started 状态**就向 stdin 写一条 `initialize` 请求
   （id 固定为 **1**，params 与 AcpClient 完全一致：protocolVersion 1、
   clientInfo `WarDex`/`0.2`、capabilities fs rw + terminal false）
   （`AgentStore.cpp:326-348`）。**改 clientInfo 版本号时这里和 AcpClient 要同步改。**
5. 读 stdout 按行解析：非 JSON object 的行视为 banner/日志噪声跳过；`id != 1` 的
   消息跳过（`AgentStore.cpp:349-362`）。
6. 收到 `id == 1` 的响应：
   - 含 `error` → 失败：`"失败 (<program>): initialize 被拒绝 — <error.message 截 200 字>"`；
   - 否则成功：读 `result.agentInfo.{name, version}`，组成
     `"成功: ACP 握手完成 — <name> <version>"`（name/version 为空则省略对应部分）
     （`AgentStore.cpp:363-382`）。
7. **成功判据是完成 ACP initialize 握手**，不是"进程能启动"。
8. 兜底分支（全部汇入统一的 `finish` 出口，保证只结束一次，`AgentStore.cpp:307-320`）：
   - 进程在握手前退出 → `"失败 (<program>): 进程在握手前退出 (code N) — <stderr 截 200 字>"`；
   - spawn 失败 → `"无法启动 «<program>»: <errorString>"`；
   - **15000ms 看门狗超时** → `"失败 (<program>): ACP initialize 握手超时 — 请检查 cli 路径、参数与登录态"`。
9. finish 时 kill 并清理测试进程。

---

## 5. CliProbe 探测流程

参照：`CliProbe.h`、`CliProbe.cpp` 全文。

### 5.1 入口与结果结构

- `probe(providerId, preferredPath?)`：异步扫描（候选队列逐个 `--version`），
  完成时发 `finished(result)` 并按 provider 缓存。
- `probePath(providerId, absolutePath)`：只验证用户手动浏览选中的单个文件
  （支持 `file:` URL 输入，`CliProbe.cpp:82-97`）。
- `result(providerId)`：取缓存（key 为 trim+toLower 的 providerId）。
- `cancel()`：杀当前进程、清队列（`CliProbe.cpp:104-112`）。
- 正在探测时再调用 probe/probePath → 先 cancel 旧的（`CliProbe.cpp:42-43, 84-85`）。
- `custom` 或未知 provider、或 `defaultCommand` 为空 → 立即完成，result.error =
  `"unsupported"`（不扫描）（`CliProbe.cpp:45-60`）。

result 结构（两种出口都产出，`CliProbe.cpp:174-181, 205-213`）：

```jsonc
{
  "providerId": "kimi",
  "found": true,                 // false 时 error = "not_found" 或 "unsupported"
  "path": "C:\\...\\kimi.exe",   // 原生分隔符；未找到为空串
  "version": "kimi 0.29.1",      // 首行输出；超时被接受时可能为空串
  "error": "",                   // "" | "not_found" | "unsupported"
  "message": "已找到 Kimi CLI kimi 0.29.1 @ C:\\..."  // 给用户看的中文文案
}
```

未找到的 message：`"未检测到 <displayName>。可点击「浏览…」手动选择可执行文件。"`

### 5.2 候选路径队列（完整顺序）

参照：`CliProbe.cpp:63-75, 249-287`。按下顺序拼接，去重后逐个验证：

1. **preferredPath**（agent 的 cliPath 覆盖），除非它只是裸命令名/`命令.exe`/`命令.cmd`
   （与 defaultCommand 相同则跳过，因为 PATH 查找会覆盖它）；
   若不带 `.exe`/`.cmd` 后缀，追加一个 `preferredPath + ".exe"` 候选
   （`CliProbe.cpp:255-263`）。
2. **kimi 专属已知安装目录**（仅 providerId == kimi，`CliProbe.cpp:266-272`）：
   - `%USERPROFILE%\.kimi-code\bin\kimi.exe`
   - `%USERPROFILE%\.kimi-code\bin\kimi`
   - `%USERPROFILE%\AppData\Local\kimi-code\bin\kimi.exe`
   - `%USERPROFILE%\AppData\Local\Programs\kimi-code\kimi.exe`
3. **npm 全局路径**（defaultCommand 非空时，`CliProbe.cpp:274-285`）：
   - `%APPDATA%\npm\<cmd>.cmd`
   - `%APPDATA%\npm\<cmd>.exe`
   - `%ProgramFiles%\nodejs\<cmd>.cmd`
   （claude-code-acp / codex-acp 经 npm -g 安装，GUI 进程的 PATH 经常缺这些目录）
4. **PATH 查找结果**：`whichOnPath(defaultCommand)` 非空且不在队列中则追加
   （`CliProbe.cpp:64-66`）。

随后**保序去重**：`cleanPath` 规范化后大小写不敏感比较（`CliProbe.cpp:68-75`）。

### 5.3 扩展 PATH（expandedSearchPath）

参照：`CliProbe.cpp:289-308`。GUI 进程的 PATH 常落后于用户 shell，因此：

1. 底为系统环境 PATH（按 `;` 切分）。
2. **合并注册表 `HKCU\Environment` 的 `Path` 值**（用户级 PATH，QSettings
   NativeFormat 读取）：按 `;` 切分，不在列表中的**前插**（用户 PATH 优先）。
3. kimi 专属：`%USERPROFILE%\.kimi-code\bin` 不在列表则**前插**。

`whichOnPath` 在上述目录列表中按顺序找以下四个名字，先存在先返回
（`CliProbe.cpp:310-327`）：`<name>`、`<name>.exe`、`<name>.cmd`、`<name>.bat`。

Rust 实现：读注册表可用 `winreg` crate（`HKEY_CURRENT_USER\Environment`，
值名 `Path`）；注意该值可能是 `REG_EXPAND_SZ`，含 `%VAR%` 引用时按原样使用即可
（旧代码不展开）。

### 5.4 `--version` 验证与 4s 超时语义

参照：`CliProbe.cpp:114-199`。

队列中每个候选：文件不存在或不是文件 → 跳过；否则 spawn `<path> --version`：

- **env 注入**：把 exe 所在目录前插到子进程 PATH（原生分隔符拼接），
  让 CLI 能找到随附的 DLL/node（`CliProbe.cpp:136-141`）。
- **4 秒硬超时**（`CliProbe.cpp:143-151`）：超时则 kill 进程，但**结果算接受**——
  "能存活 4 秒的二进制存在且可运行，只是没有 --version 或在等 stdin"，此时
  version 为空串。这是刻意的宽松语义，不要改成失败。
- 正常退出：version 取 stdout（trim），为空则取 stderr（trim），再取**第一行**
  （`CliProbe.cpp:156-161`）。
- 成功判据（`CliProbe.cpp:163-165`）：
  `超时被杀 || (正常退出 && (version 非空 || 退出码 == 0))`。
- 成功 → message `"已找到 <displayName>[ <version>] @ <path>"`，发布并结束。
- 失败/spawn 错误 → 异步尝试下一个候选（`CliProbe.cpp:188-196`）。
- 队列耗尽 → `finishNotFound`（error `"not_found"`）。
- 进程清理：kill 后最多等 300ms（`CliProbe.cpp:219-230`）。

### 5.5 kimi 安装帮助（配置页帮助对话框）

参照：`CliProbe.cpp:22-38`。`installHelpText` / `installHelpUrl` 为常量：

- URL：`https://www.kimi.com/code`
- 文本（原文照搬）：

```
WarDex 通过本机 Kimi CLI 与模型通信。

1. 安装 Kimi Code CLI（官方文档 / 产品页）。

2. 安装完成后常见路径：
   %USERPROFILE%\.kimi-code\bin\kimi.exe

3. 回到本页点击「检测 CLI」，成功后会自动填入绝对路径。
   也可点「浏览…」手动选择 kimi.exe。

说明：仅写 kimi 时，图形界面 PATH 可能与 PowerShell 不一致，因此建议保存绝对路径。
```

---

## 实现检查清单

Provider 注册表（`src-tauri/src/providers.rs`）：

- [ ] ProviderSpec 全部 13 个字段与四个 provider 的逐项值同 1.1 表一致（`ProviderRegistry.cpp:3-85`）
- [ ] claude `clearEnvs` 三变量经"null = 删除"注入（`ProviderRegistry.cpp:34-38`）
- [ ] ANTHROPIC_AUTH_TOKEN 特例：`sk-ant-` 前缀官方 key 不注入 bearerTokenEnv（`ChatController.cpp:849-856`）
- [ ] `mapMode` 未映射恒等透传；`spec(id)` trim+toLower 匹配（`ProviderRegistry.cpp:89-117`）
- [ ] `specMap` 中 `acpArgs` 以空格 join 成单串（`ProviderRegistry.cpp:128`）

Agent 存储（`src-tauri/src/store/agents.rs`）：

- [ ] 数据根 `%APPDATA%\WarDex`（dev 构建 `WarDex-dev`）；`agents/index.json` + `agents/<id>.json`（`AppPaths.cpp:11-34`）
- [ ] index.json 格式 `{defaultAgentId, agents:[id...]}`；加载时捡回孤儿 agent 文件（`AgentStore.cpp:456-465`）
- [ ] fromJson 缺省值：provider=kimi、enabled=true、cliPath="kimi"（`AgentStore.cpp:515-533`）
- [ ] 新建默认值与"首个 Agent 自动默认"（`AgentStore.cpp:106-118`）
- [ ] updateAgent：apiKey 为空或含 `*` 时保留旧值（`AgentStore.cpp:154-159`）
- [ ] maskKey：≤8 字符全星；否则前 3 + `****` + 后 4（`AgentStore.cpp:586-593`）
- [ ] 删除默认 Agent 时列表首个递补（`AgentStore.cpp:192-201`）
- [ ] avatarFor：文件存在才返回 file:/// URL（`AgentStore.cpp:574-584`）

会话启动组装（ensureAcp，与 chat 层配合）：

- [ ] env 组装顺序：apiKeyEnvs → bearerTokenEnv 特例 → baseUrlEnvs → clearEnvs 置 null（`ChatController.cpp:845-865`）
- [ ] args = acpArgs + shell 规则 split 的 extraArgs；custom 时 extraArgs 即全部参数（`ChatController.cpp:867-875`）
- [ ] mcpServers 文本解析失败降级为空数组 + 警告日志（`ChatController.cpp:877-894`）
- [ ] mode 经 mapMode 翻译后再下传（`ChatController.cpp:908-916`）

testAgent：

- [ ] 单飞行守卫与三个前置失败文案（`AgentStore.cpp:237-251`）
- [ ] program 解析链 cliPath → defaultCommand → "kimi"；`.cmd/.bat` 包 cmd.exe（`AgentStore.cpp:281-305`）
- [ ] started 后立即写 id=1 的 initialize，params 与 AcpClient 完全一致（`AgentStore.cpp:326-348`）
- [ ] stdout 噪声行与 id≠1 消息跳过（`AgentStore.cpp:354-362`）
- [ ] 成功判据 = 握手完成，成功文案含 agentInfo.name/version（`AgentStore.cpp:363-382`）
- [ ] 15s 看门狗 + 进程早退 + spawn 失败三分支统一 finish 出口（`AgentStore.cpp:307-320, 384-408`）

CliProbe（`src-tauri/src/cli_probe.rs`）：

- [ ] custom/未知 provider 立即返回 error="unsupported"（`CliProbe.cpp:45-60`）
- [ ] 候选队列完整顺序：preferredPath(+.exe) → kimi 四个已知目录 → npm 三个 shim 路径 → PATH 查找（`CliProbe.cpp:249-287`）
- [ ] cleanPath 后大小写不敏感保序去重（`CliProbe.cpp:68-75`）
- [ ] expandedSearchPath：系统 PATH + HKCU\Environment Path 前插合并 + kimi bin 前插（`CliProbe.cpp:289-308`）
- [ ] whichOnPath 四后缀顺序：裸名/.exe/.cmd/.bat（`CliProbe.cpp:310-327`）
- [ ] --version 子进程 PATH 前插 exe 所在目录（`CliProbe.cpp:136-141`）
- [ ] 4s 硬超时 = kill 但**接受**（version 空）；成功判据含"超时 || (正常退出 && (有输出 || code 0))"（`CliProbe.cpp:143-165`）
- [ ] version 取 stdout→stderr→第一行；结果按 provider 缓存（`CliProbe.cpp:156-161, 240-247`）
- [ ] not_found/unsupported/found 三种 result 结构与中文文案（`CliProbe.cpp:174-181, 201-217`）
- [ ] installHelpText/installHelpUrl 常量（`CliProbe.cpp:22-38`）

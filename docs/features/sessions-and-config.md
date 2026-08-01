# 功能规格：会话选择页 + Agent 配置页

> 本文档是 WarDex Tauri 重写（`C:/workspace/Wardex-rust`）中**会话选择页**与 **Agent 配置页**的完整功能规格。旧项目（C++/Qt6，`C:/workspace/WarDex`，只读参照）是唯一行为基准，文中以 `旧: path:行号` 标注参照位置。界面文本一律为**纯中文**。

## 相关文档

- [../architecture.md](../architecture.md) — 总体架构与数据目录兼容策略
- [../design-principles.md](../design-principles.md) — 设计原则
- [../implementation-plan.md](../implementation-plan.md) — 阶段划分
- [../acp-protocol.md](../acp-protocol.md) — ACP 协议（initialize 握手、provider 差异）
- [../data-formats.md](../data-formats.md) — sessions/agents/projects/user_prefs 数据格式
- [../ui-design.md](../ui-design.md) — WC3 视觉体系
- [../assets.md](../assets.md) — 资源清单
- [chat.md](chat.md) — 聊天页（rail 右键菜单、权限模式下拉等与本文档交叉引用）
- [main-menu-and-misc.md](main-menu-and-misc.md) — 页面状态机、入场动画、全局杂项

## 新版目标实现位置

| 模块 | 位置 |
|---|---|
| 会话选择页 | `src/pages/SessionSelect.vue` |
| Agent 配置页 | `src/pages/Config.vue` |
| 前端状态 | `src/stores/sessions.ts`、`src/stores/agents.ts`（Pinia） |
| Rust 持久化 | `src-tauri/src/store/sessions.rs`、`src-tauri/src/store/agents.rs`、`src-tauri/src/store/projects.rs` |
| CLI 探测 | `src-tauri/src/probe.rs` |
| provider 差异 | `src-tauri/src/acp/provider.rs`（对应旧 ProviderRegistry） |

---

# 第一部分：会话选择页

旧: `ui/SessionSelectPage.qml`（871 行）全文

## 1. 布局

- ShellFrame 下落入场（同其他页，见 main-menu-and-misc.md）。
- 左面板（宽 62%）：`历史会话` 标题 → 搜索框 → 全文搜索结果区块（条件可见）→ 分组会话列表。
- 右上面板：选中会话的概略信息；右下操作湾：`进入会话(L)`、`返回(B)` 两个主菜单尺寸按钮（276 宽、aspect 4.87）。
- 底栏高度 `max(188, height*0.20)`，与聊天页同款弹性思路。

## 2. 分组会话列表

旧: `src/SessionStore.cpp:1173-1216`（groupedSessions）、`ui/SessionSelectPage.qml:55-76, 367-650`

### 2.1 数据与排序

- `groupedSessions()`：全部会话按 `projectDir` 分组。组顺序 = 组内最新会话的 updatedAt 倒序（即"最新活跃的组在最前"）；组内会话 updatedAt 倒序，**置顶（pinned）在前**（稳定排序， pinned 类内部仍保持时间序）。
- 组字段：`{projectDir, projectName, sessions: [{sessionId, title, provider, updatedAt, messageCount, pinned}]}`。
- `projectName`：`projectDir` 为空的组固定 `临时会话（无项目）`；否则取目录本名（末段），末段为空（盘符根）用完整路径。UI 显示时优先使用**项目别名**（`projectStore.displayNameFor`，见 2.4）。
- 平铺索引（sessions 模型）始终 updatedAt 倒序；重命名/置顶**不重排**列表（旧: `src/SessionStore.cpp:583-641`）。

### 2.2 组头

旧: `ui/SessionSelectPage.qml:378-513`

- 行高 30：`▶/▼` 折叠箭头（金）+ 文件夹图标 + 项目名（别名优先，金色加粗）+ 完整路径（灰、中部省略）+ 右侧 `＋新会话` 按钮（仅项目组；临时会话组没有）。
- **左键**：折叠/展开该组。折叠状态是运行期 map（`collapsed[projectDir]`），不落盘。**搜索非空时忽略折叠状态**，直接展示匹配行（`ui/SessionSelectPage.qml:382-384`）。
- **右键**（仅项目组）：菜单 `重命名项目` → 组头行内重命名（见 2.4）。
- `＋新会话`：在该项目目录新建会话并直接进入聊天页（走主菜单"打开最近项目"同一条路径，含目录存在性检查，见 main-menu-and-misc.md）。

### 2.3 会话行

旧: `ui/SessionSelectPage.qml:515-639`

- 行高 40：📌（置顶时）+ 标题（选中行白色，其余 `#e8ecf4`）+ provider（灰）+ updatedAt（`yyyy-MM-dd HH:mm`）。选中行蓝底 `#1a3a6e`。
- **未读徽标**：后台回合完成且未查看的会话，行右侧显示蓝色胶囊 `NEW`（`ui/SessionSelectPage.qml:619-637`；未读语义见 chat.md 7.7）。
- **单击**：仅选中——右侧面板加载概略（`sessionSummary`），**不打开会话**。
- **双击**：选中并进入会话（`Qt.callLater` 延迟一拍发 `enterSession`，避免与按钮手势重复打开，`ui/SessionSelectPage.qml:608-618`）。
- **右键菜单**（`ui/SessionSelectPage.qml:771-798`）：`置顶会话/取消置顶`、`重命名会话`、`复制会话内容`、`基于此提问`、`删除会话`——与聊天页 rail 菜单完全一致（语义见 chat.md 第 5 节）。删除有确认对话框 `确定删除这条会话及其全部消息吗？\n该操作不可撤销。`；删除的是当前选中会话时清空右侧概略。
- **行内重命名**：标题位置替换为 TextInput，初始值为现标题、全选聚焦；Enter 确认（trim 非空才提交，上限 48 字）、Esc 取消。重命名后若该行是选中行，同步刷新右侧概略。

### 2.4 项目别名行内重命名

旧: `ui/SessionSelectPage.qml:40-49, 425-460`

- 组头项目名位置替换为 TextInput（宽 160），初始值为当前显示名（别名或目录本名）、全选聚焦；Enter 确认、Esc 取消。
- 确认规则：输入 trim 后**等于目录本名 → 清除别名**（`setAlias(dir, "")`），否则设为新别名。别名独立存储，项目掉出最近列表也不丢失（见 main-menu-and-misc.md 最近项目）。

## 3. 搜索

旧: `ui/SessionSelectPage.qml:24-29, 78-114, 228-366, 855-870`、`src/SessionStore.cpp:1538-1658`

搜索框（占位 `搜索会话标题与内容…`）同时驱动两套机制：

### 3.1 标题过滤（即时）

- 每次输入立即执行：会话标题子串匹配（忽略大小写），组内无匹配则**整组隐藏**；命中组直接展开（忽略折叠状态）。
- 无匹配时列表空态文案 `无标题匹配的会话`；无搜索词且无会话时 `暂无历史会话\n请先「新建会话」或「打开项目」`。

### 3.2 全文搜索（500ms 防抖 + 代际防旧）

- 输入停顿 **500ms** 后自动触发；**回车立即触发**（停掉防抖计时器）。
- `searchMessages(query, 50)`（旧: `src/SessionStore.cpp:1538-1658`）：
  - 跨**全部**会话（含未打开的，worker 线程直接扫 JSONL 文件，GUI 线程零阻塞），匹配范围 = 会话标题 + user/assistant 消息 `content`（**跳过** thinking/toolCalls/segments payload），忽略大小写。
  - 每次调用递增共享 generation 计数并**取代**上一次：旧 worker 在会话间检查代际，过期即放弃；结果到达时再次校验代际，不匹配直接丢弃。前端也持有本次 generation，信号回来比对，不一致忽略（双保险，`ui/SessionSelectPage.qml:861-870`）。
  - 空查询：取消在跑搜索并立即回空结果。
  - 结果项：`{sessionId, sessionTitle, projectDir, snippet, timestamp, updatedAt, hitCount, titleOnly}`。每个会话最多 3 条命中（最新的在前，JSONL 为时间序故倒序取）；snippet = 命中处前后各 40 字符上下文，两端截断加 `…`；标题命中但正文无命中时 `titleOnly:true`、snippet 用标题。会话按 updatedAt 倒序遍历，总数 ≤50。跳过 pending 占位行与历史前导 `…` 残留（同加载时的 scrub）。
- **结果区块**（标题过滤列表上方，独立区域，`ui/SessionSelectPage.qml:264-366`）：
  - 搜索词非空且（搜索中 | 搜索过 | 有结果）时可见。
  - 头部：`全文搜索中…` / `全文搜索结果（N）`。
  - 结果行（最多约 120px 高、超出滚动）：会话标题（金、加粗）+ 命中消息时间 + snippet。snippet 中命中词高亮：HTML 转义后首个命中处包 `<font color="#f2cf6b"><b>…</b></font>`（StyledText）；`titleOnly` 时显示 `（仅标题命中）`。
  - 搜索完成且无结果：`无匹配内容`。
  - **点击结果行**：直接进入该会话（同双击会话行路径）。

## 4. 右侧概略与进入会话

旧: `ui/SessionSelectPage.qml:654-758`

- 概略面板（选中后可见）：标题（金、可换行）+ 两列网格 `Agent:` / `Provider:` / `消息数:` / `更新:` / `项目:`（无项目显示 `（临时会话，无项目）`，路径中部省略）+ `摘要:` 段（meta.summary，无则 `（无）`）。未选中时面板中央提示 `选择左侧会话\n查看概略信息`。
- `进入会话(L)`：选中才可用，等价双击。`返回(B)` / `Esc`：回主菜单——但**行内编辑（会话/项目重命名）或搜索框聚焦时 Esc 让位**（`ui/SessionSelectPage.qml:763-769`）。
- 页面进入时：`reloadSessions()` 重扫索引 → 重建分组 → 无选中时自动选中第一组第一条（`selectFirstIfNeeded`，`ui/SessionSelectPage.qml:170-175, 843-853`）。`sessionsChanged` 时重建并保持兜底选中。
- **进入会话的导航守卫**（Main 侧，旧: `ui/Main.qml:463-520`）：
  - 页面过渡动画进行中或已有进入流程在飞（`enteringSession`）→ 忽略，防止动画中重复 openSession。
  - 会话的项目目录已被删除/移动 → 弹 `项目不存在` 对话框：`<路径>\n点击确定后将删除这条会话。`，确认后 `closeRuntime` + `deleteSession`。
  - 正常路径：延迟两拍（离开输入事件栈）→ `openSession` → 再延迟一拍 → 切到聊天页。失败显示 banner `无法打开会话`。

## 5. 聊天页 rail 的交叉一致性

聊天页左栏 rail（chat.md 第 5 节）与本页共享以下后端语义，实现时必须复用同一代码路径：

- `sessionsForProject`：置顶在前 + updatedAt 倒序（旧: `src/SessionStore.cpp:1144-1171`）。
- `renameSession`：trim、48 字上限、不重排、不 bump updatedAt。
- `toggleSessionPinned` / `copySessionTranscript` / `deleteSession` / `基于此提问`（pendingComposerText 一次性预填 `基于会话「<标题>」：`）。

---

# 第二部分：Agent 配置页

旧: `ui/ConfigPage.qml`（1075 行）全文

## 6. 布局与导航

- 左面板（宽 48%）：`Agent 配置` 标题 + 计数行（`共 N 个 · 点击选中编辑`，空列表时 `暂无 Agent — 点击下方新建`）+ Agent 列表 + `新建 Agent` 按钮（主菜单尺寸）。
- 右面板：可滚动表单（Flickable），依次是 **应用级设置**（我的头像 / 我的名字 / 界面字体缩放）→ 分隔线 → **Agent 编辑器**。
- 右下操作湾（贴右对齐的窄铁条框）：`保存并返回`、`返回(B)`。
- `Esc` / `返回(B)` → `tryBack()`：有未保存修改弹 `未保存的更改` 对话框（`保存并返回` / `丢弃` / `取消` 三钮，`ui/ConfigPage.qml:1033-1066`）；无修改直接返回主菜单。
- 页面进入时：有 agent 则自动选中**默认 Agent** 载入编辑器（`ui/ConfigPage.qml:1068-1074`）。

## 7. Agent 列表

旧: `ui/ConfigPage.qml:307-371`

- 行高 48，斑马纹；选中行蓝底 `#1a3a6e`；**默认 Agent 金边框** `#c9a227`。
- 行内容：`★`（默认）/ `·` + 名称 + 副行 `provider`（拼接 `· 默认`；`canUseForChat=false` 时显示 `· 暂不可对话` 且变红褐 `#a06040`）。
- **点击行**：若当前编辑有未保存修改且切换到另一行 → **先自动保存当前**再载入新行（`ui/ConfigPage.qml:352-356`）。
- `新建 Agent`：当前 dirty 也先自动保存；创建名为 `新 Agent` 的记录（默认 provider kimi）→ 选中新行 → 名称输入框聚焦全选 → 状态行 `已新建 Agent`。cliPath 为空会触发自动探测（见 9.2）。

## 8. 应用级设置（表单顶部）

旧: `ui/ConfigPage.qml:406-552`

### 8.1 我的头像

- 56×56 预览（金边框）+ 说明 `对话页用户气泡使用此头像\n未上传时使用默认金发肖像` + `上传…` / `恢复默认`。
- 上传：图片文件对话框（`图片 (*.png *.jpg *.jpeg *.webp *.bmp)`）；选中后**复制进应用数据目录**并设为 userAvatar（`userPrefs.setUserAvatarFromFile`），状态行 `头像已更新` / `头像导入失败`。
- `恢复默认`：清除自定义头像（`clearUserAvatar`），状态行 `已恢复默认头像`。聊天气泡立即生效。

### 8.2 我的名字

- 输入框（占位 `阿尔萨斯`），`editingFinished` 即保存到 userPrefs；说明 `对话页用户气泡显示此名字，留空则默认「阿尔萨斯」`。

### 8.3 界面字体缩放

- 下拉四档：`85%` / `100%` / `115%` / `130%` → `fontScale` 0.85 / 1.0 / 1.15 / 1.30；当前值取最近档。**立即生效**并持久化，作用于聊天气泡、输入框、会话列表、本表单等主要阅读区（各页面 `fs()` 辅助函数；共享控件艺术不缩放，见 ../ui-design.md）。

## 9. Agent 编辑器

旧: `ui/ConfigPage.qml:555-884`

未选中 agent 时整个字段区禁用半透明，标题显示 `请先在左侧新建或选择 Agent`（金色）。

### 9.1 字段一览

| 字段 | 控件 | 说明 |
|---|---|---|
| 名称 | TextField | 占位 `例如：工作 Kimi` |
| Provider | WarDropdown | `kimi` / `claude` / `codex` / `custom`，切换见 9.2 |
| （接入提示） | 文本行 | 当前 provider 的 `installHint`（ProviderRegistry 提供），无则隐藏 |
| Model | TextField + 「选择…」下拉 + 「刷新」按钮 | 自由文本兜底；点刷新按当前 Base URL 调 `fetch_models`（GET `{root}/models`，剥掉 `/chat/completions` 后缀，OpenAI 兼容）拉模型列表；kimi 且 **Base URL 为空**时另并入 `kimi_model_aliases`（`~/.kimi-code/config.toml` 的 `[models]` 表键）——有自有 Base URL 时以端点 `/models` 为准，全局别名不并入，避免污染每个 kimi Agent 的候选；拉到后「选择…」下拉列出候选，点选填入。**kimi + 已保存 Agent + 拉到非空列表时，同次刷新会把整个列表批量写进 config.toml**（`sync_agent_models`）：provider 节按 Agent 命名空间 `[providers.wardex-agent-<id>]`（apiKey 明文、各 Agent 不混），模型别名为裸模型 id（`max_context_size` = 上下文长度 ×1024），该 Agent 命名空间下已不在最新列表的别名被清除；已有 `support_efforts`/`default_effort`（默认思考强度写入的）在重写中保留。之后 CLI picker 即含这些模型，对话页切换走热切换。kimi 侧生效路径：是 CLI 别名的走 ACP `set_config_option("model", …)`（新会话自动应用，chat.md §6.1）；非别名在 spawn 时注入 `KIMI_MODEL_NAME`/`KIMI_MODEL_API_KEY`/`KIMI_MODEL_BASE_URL`（有 baseUrl 时附带 `KIMI_MODEL_PROVIDER_TYPE=openai`） |
| 上下文长度（K） | number 输入（0–4096，0=256K） | 仅 kimi + 有 Base URL 时显示；`agent.maxContextK`，刷新批量同步时统一作为所有模型别名的 `max_context_size = 值×1024`（/models 接口不返回上下文长度，故统一填写） |
| Base URL（可选） | TextField + 「预置…」下拉 | 上方随 provider 联动的 `baseUrlHint` 提示行；预置下拉（DeepSeek `https://api.deepseek.com/v1` / Kimi `https://api.kimi.com/coding/v1` / OpenCode Zen `https://opencode.ai/zen/go/v1`，前端常量 `baseUrlPresets`）点选即填入，仍可手改 |
| CLI 路径 | TextField + 三按钮 | 占位：内置 provider `留空自动探测`，custom `CLI 可执行文件完整路径` |
| API Key | TextField（Password echo） | 掩码显示规则见 9.5 |
| 额外参数 | TextField | 说明 `额外参数（追加在 ACP 启动参数后；custom 时即为完整启动参数）` |
| MCP Servers | TextArea（4 行） | 说明 `MCP Servers（JSON 数组，建会话时通过 ACP 下发；格式错误将被忽略并记日志）`，占位给 JSON 示例 |
| 头像 | 56×56 预览 + `选择图片…` / `重置` | 绝对路径引用、**不复制**；空 = 内置默认；选择后提示 `头像已选择，保存后生效` |

- 所有字段编辑即标 dirty（TextArea 无 textEdited 信号，用"有焦点时 textChanged"判定用户输入，`ui/ConfigPage.qml:751-755`——新版用 `@input` 事件天然等价）。
- **draft 机制**：字段值先进 draft 属性，显式保存才写盘；切行/新建/设为默认/测试连接/保存并返回都会先触发 `saveCurrent()`（dirty 时）。保存失败状态行显示 `agentStore.lastError || 保存失败`。

### 9.2 Provider 切换与 CLI 自动探测

旧: `ui/ConfigPage.qml:78-137, 581-608, 657-706`、`src/CliProbe.h` 全文

- Provider 下拉切换：标 dirty → 更新 draftProvider → **若 CLI 路径为"裸值"则延迟一拍自动探测**。裸值判定（`isBareCliPath`）：为空，或等于该 provider 的 `defaultCommand`（含 `.exe`/`.cmd` 后缀变体）；**custom 永不探测**（无规范 CLI）。
- 探测是**异步**的（旧版同步 waitForFinished 曾冻结 UI，新版用 tokio 异步 spawn）：候选路径逐个扫描（显式路径优先 → 知名安装目录 → PATH），每个幸存二进制异步问 `--version`。结果按 provider 缓存，切换 agent/provider 即时重显。
- 探测状态行（仅内置 provider 且选中行可见）：
  - 探测中：`正在检测 <displayName>…`
  - 找到（绿）：`已找到 <displayName> <version> @ <path>`
  - 未找到（红）：显示该 provider 的 `installHint`（或结果 message）
- **自动回填**：探测完成且本次探测以 autoFill 模式发起 → 把找到的绝对路径填入 CLI 路径字段、标 dirty、状态行 `已自动填入 CLI 路径`；非 autoFill（如手动点检测但路径已显式填）→ 仅提示 `已找到 CLI`。
- **过期防护**：探测结果返回时若 provider 已切换（`r.providerId !== currentProviderId`）→ 丢弃，不得动表单（`ui/ConfigPage.qml:119-122`）。
- 触发自动探测的时机：载入选中行时（路径为裸值）、provider 切换时（路径为裸值）、点 `检测 CLI` 按钮、`浏览…` 选定文件后（probePath 验证该文件）。
- 按钮区：`检测 CLI`（仅内置 provider 可见；探测中文案 `检测中…` 且禁用）、`浏览…`（exe 过滤器）、`如何安装`（**仅 kimi** 可见 → 安装指南对话框：help 文本 + URL，三钮 `打开链接`（系统浏览器）/ `重新检测` / `关闭`，`ui/ConfigPage.qml:992-1031`）。

### 9.3 底部动作

旧: `ui/ConfigPage.qml:824-871`

- `设为默认`：先保存当前 → `setDefault` → 状态行 `已设为默认` / 失败原因。默认 Agent 是新建会话使用的 agent（`ChatController::startNewSessionImpl` 找不到可用默认时拒绝并报 `请先在配置中创建 Kimi Agent 并设为默认`，旧: `src/ChatController.cpp:692-698`）。
- `测试连接`（`testAgent`，旧: `src/AgentStore.cpp:236-…`）：先保存当前；按钮文案 `测试中…` 且禁用（全局同时只跑一次）。语义：
  - 若 CLI 路径为裸值且是内置 provider → 先发起探测，状态行 `正在解析 CLI 路径，完成后请再点测试连接`，本次不测试。
  - 否则按 ChatController 同一约定 spawn CLI（provider 的 env 注入 + acpArgs + extraArgs；Windows `.cmd/.bat` shim 包 `cmd.exe /c`），写入一条 ACP `initialize` JSON-RPC 请求，**成功判据 = 收到合法的 initialize 响应**（不是"进程起来了"）；成功/失败/超时/崩溃都恰好收尾一次，结果文本显示在表单底部状态区（与 statusMsg 拼接换行）。
  - provider 不支持对话 → `该 Provider 暂不支持测试`。
- `删除 Agent`：立即删除（**无确认框**）→ 选中默认 agent（若无则清空选择）→ 状态行 `已删除`。

### 9.4 MCP Servers 容错

旧: `src/ChatController.cpp:877-894`

配置页存原始 JSON 数组文本；**建会话时**解析：解析失败或非数组 → 降级为"无 MCP servers"并记日志，**不阻塞会话**；条目结构不校验，原样透传给 agent。

### 9.5 API Key 存储与掩码

旧: `src/AgentStore.cpp:50, 586-592`

- 磁盘明文存储（数据格式见 ../data-formats.md）；**列表/日志等展示面**使用掩码：长度 ≤8 → `********`；否则 `前 3 字符 + **** + 后 4 字符`。
- 编辑器内用 Password echo 输入框；保存的是用户输入的原文。

## 10. Provider 注册表（配置页展示与行为的事实源）

旧: `src/ProviderRegistry.cpp:3-85`。新版在 `src-tauri/src/acp/provider.rs` 复刻同样四条记录；配置页的提示文本、探测目标、env 注入全部从这里取，**不得在各处硬编码**。

| id | displayName | defaultCommand | acpArgs | apiKeyEnvs | baseUrlEnvs | 其他 |
|---|---|---|---|---|---|---|
| `kimi` | Kimi CLI | `kimi` | `acp` | `KIMI_API_KEY`, `OPENAI_API_KEY` | `KIMI_BASE_URL`, `OPENAI_BASE_URL` | installHint `安装见 https://www.kimi.com/code`；baseUrlHint：OpenAI 兼容端点以 /v1 结尾，留空用本机登录态 |
| `claude` | Claude Code | `claude-code-acp` | （空，适配器直接讲 ACP） | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL` | modeMap：`auto→acceptEdits`、`yolo→bypassPermissions`；clearEnvs：`CLAUDECODE`、`CLAUDE_CODE_ENTRYPOINT`、`CLAUDE_CODE_SSE_PORT`（防嵌套）；中转 key 额外注入 `ANTHROPIC_AUTH_TOKEN`（官方 `sk-ant-` 前缀 key 不注入）；installHint 提及 `npm i -g @zed-industries/claude-code-acp` 与 `claude /login` 本地凭据 |
| `codex` | Codex CLI | `codex-acp` | （空） | `OPENAI_API_KEY` | `OPENAI_BASE_URL` | installHint 提及 `npm i -g @zed-industries/codex-acp` 与 `codex login`；baseUrlHint 提及 `wire_api="chat"` 中转注意事项 |
| `custom` | 自定义 (ACP) | （空，必须显式填 cliPath） | （空，extraArgs 即完整参数） | `OPENAI_API_KEY` | `OPENAI_BASE_URL` | 逃生舱：任何 ACP CLI 不改代码接入；installHint 提示填进入 ACP 模式的参数 |

- `mapMode(provider, mode)`：查 modeMap，缺省恒等。
- `chatCapable(provider)`：四者均 true；未注册 provider 在 `startSend` 时落定错误（见 chat.md 7.1）。
- `specMap`（QML 用）：`{id, displayName, defaultCommand, acpArgs(空格连接), installHint, baseUrlHint, chatCapable}`。

## 实现检查清单

会话选择页
- [ ] 分组：组按最新会话倒序、组内 updatedAt 倒序 + 置顶在前（稳定）、空 projectDir 组名 `临时会话（无项目）`
- [ ] 组头：折叠/展开（运行期状态）、搜索时忽略折叠、路径中部省略、`＋新会话`（仅项目组）、右键重命名项目
- [ ] 项目别名：行内编辑、等于目录本名即清除别名、别名独立持久化
- [ ] 会话行：单击仅预览 / 双击进入（callLater 防重复）/ 右键五菜单项 / NEW 未读徽标 / 行内重命名（48 字、Enter/Esc）
- [ ] 标题过滤即时、组级隐藏、两种空态文案
- [ ] 全文搜索：500ms 防抖 + 回车立即、worker 扫 JSONL、generation 双重防旧、≤3 命中/会话、±40 字符 snippet、titleOnly 分支、命中词金色加粗高亮、点击进入会话、总数 ≤50
- [ ] 右侧概略六字段 + 摘要；未选中提示
- [ ] 进入守卫：过渡中忽略、项目目录丢失弹窗（确认删会话）、两拍延迟 openSession→导航
- [ ] 进页 reloadSessions + 兜底选中第一条；Esc 在行内编辑/搜索聚焦时让位

Agent 配置页
- [ ] 左列表：★/默认金边框、斑马纹、暂不可对话红字、切行自动保存 dirty、新建 Agent（默认 kimi、命名 `新 Agent`、聚焦全选）
- [ ] 应用级：我的头像上传（复制进数据目录）/恢复默认、我的名字（editingFinished 保存、阿尔萨斯兜底）、字体缩放四档立即生效
- [ ] 编辑器全字段（名称/Provider/Model/Base URL/CLI 路径/API Key Password/额外参数/MCP Servers TextArea/头像路径引用不复制）+ draft + dirty 跟踪
- [ ] provider 提示行（installHint / baseUrlHint）随下拉联动
- [ ] CLI 探测：异步、裸值判定（含 .exe/.cmd 变体、custom 免探测）、autoFill 回填 vs 仅提示、过期 provider 结果丢弃、四种触发时机、探测状态三态文案
- [ ] 如何安装对话框（仅 kimi）：打开链接/重新检测/关闭
- [ ] 测试连接：先保存、裸路径先探测并提示再点、initialize 握手为成功判据、单次收尾、不支持 provider 分支
- [ ] 设为默认、删除 Agent（无确认、回退选中默认）
- [ ] tryBack 三选对话框（保存并返回/丢弃/取消）；保存并返回按钮
- [ ] API Key 掩码规则（≤8 全掩、否则前3+****+后4）
- [ ] MCP Servers JSON 解析失败降级不阻塞（建会话时）
- [ ] Provider 注册表四条记录与 modeMap/clearEnvs/中转 key 特例完整复刻

# 功能规格：聊天页（Chat）

> 本文档是 WarDex Tauri 重写（`C:/workspace/Wardex-rust`）的聊天页完整功能规格。读者是实现者，无旧项目上下文；旧项目（C++/Qt6，位于 `C:/workspace/WarDex`，**只读参照，禁止修改**）是唯一行为基准。文中以 `旧: path:行号` 标注旧代码参照位置。
>
> 界面文本一律为**纯中文**；代码、标识符、文件路径保持英文。

## 相关文档

- [../architecture.md](../architecture.md) — 总体架构（Tauri 命令/事件模型、内存设计原则 P1~P4）
- [../design-principles.md](../design-principles.md) — 设计原则
- [../implementation-plan.md](../implementation-plan.md) — 阶段划分与任务拆解
- [../acp-protocol.md](../acp-protocol.md) — ACP JSON-RPC 协议细节（initialize、session/prompt、权限反向 RPC 等）
- [../data-formats.md](../data-formats.md) — `%AppData%/WarDex/` 数据格式（sessions/agents/projects/prefs JSON/JSONL）
- [../ui-design.md](../ui-design.md) — WC3 视觉体系（九宫格、配色、字体、字号缩放 `fs()`）
- [../assets.md](../assets.md) — 贴图/音效资源清单
- [sessions-and-config.md](sessions-and-config.md) — 会话选择页与 Agent 配置页
- [main-menu-and-misc.md](main-menu-and-misc.md) — 主菜单、页面状态机、待办页、全局杂项

## 新版目标实现位置

| 模块 | 位置 |
|---|---|
| 页面组件 | `src/pages/Chat.vue` |
| 气泡组件 | `src/components/chat/ChatBubble.vue` |
| 子 Agent 面板 | `src/components/chat/SubagentPanel.vue` |
| 文件预览弹窗 | `src/components/chat/FilePreviewDialog.vue` |
| 前端状态 | `src/stores/chat.ts`（Pinia，渲染状态）、`src/stores/sessions.ts` |
| Rust 会话运行时 | `src-tauri/src/chat/`（runtime 管理、流式合并、限流重试、续写、subagent 跟踪） |
| Rust 持久化 | `src-tauri/src/store/sessions.rs`（消息模型、JSONL 读写、搜索、@引用、工作区文件树） |
| 通信 | 前端 `invoke()` 调 Tauri 命令；Rust `emit()` 推事件（`acp://chunk`、`acp://tool`、`acp://permission`、`acp://turn`、`chat://retry` 等，事件名以 ../acp-protocol.md 为准） |

**权威数据在 Rust 侧**：消息 segments 的唯一数据源是 Rust 的会话 store；前端只维护渲染状态。流式渲染走**增量 DOM 追加**（text 段只 append 新 chunk 到末尾文本节点，绝不整段重绑定）；markdown 在回合结束后才对完整正文渲染一次。工具 payload 在前端展示层截断到 64KB（完整内容只落盘）。

---

## 1. 页面布局

旧: `ui/ChatPage.qml:473-519`（ShellFrame + layout 度量）

三栏 + 底部双框布局，全部装在 `ShellFrame` 下落入场动画容器里（动画见 main-menu-and-misc.md）：

- **最左栏「本项目会话」rail**：宽 196px，贯穿全高，九宫格框 `frame_fat_bar.png`（slice 28/32/28/32，hole 24/26/24/26，带玻璃底）。
- **左中聊天面板**：`(width - railW - 2*gap) * 0.72`，上为标题行 + 消息列表，贴图 `frame_iron_panel.png`（带吊链）。
- **右栏「会话简介」**：剩余宽度，上为信息/工作区文件树。
- **底部左输入框** 与 **底部右操作湾**（两个主菜单尺寸按钮），贴图 `frame_iron_bar.png`。
- 底栏高度弹性：`botH = max(minBotH, min(max(menuBtnH*2+76, height*0.185), height-gap-minTopH))`，其中 `minTopH=240`、`minBotH=menuBtnH+67`、`menuBtnW=276`、`btnAspect=4.87`。小窗口时底栏收缩、消息区先让位，输入行永远完整可见（旧: `ui/ChatPage.qml:492-517` 的注释记录了该 bug 修复史，新版布局必须保留此弹性，不要写死 190px）。
- 浮动件（附件条 / 发送队列 / 子 Agent 面板 / 内存横幅 / 限流横幅 / 回到底部钮）全部 `z≥26` 覆盖在聊天面板上方，**绝不挤压输入框高度**。

## 2. 消息列表

### 2.1 数据模型

每条消息（旧: `src/SessionStore.cpp:106-183` MessageListModel）字段：

- `messageId`（UUID 无括号）、`role`（`user` | `assistant`）、`content`、`createdAt`（ms epoch）、`provider`、`status`、`thinking`、`toolCalls[]`、`segments[]`、`attachments[]`。
- `status` 取值：`pending`（占位行，内容为 `"…"`）→ `streaming` → `done` | `error` | `interrupted`。
- `segments[]` 为**到达顺序**的交错块流：`{kind:"thinking", text}` / `{kind:"text", text}` / `{kind:"tool", toolCallId, ...tool 字段}`。思考→回复→工具→回复 按真实流式顺序交错（旧: `src/SessionStore.cpp:784-798, 866-997`）。
- 磁盘 JSONL 每行一条消息（格式见 ../data-formats.md）。读取旧数据时若 `segments` 为空，按 thinking→text→tools 顺序合成（旧: `src/SessionStore.cpp:1757-1776`）；并清除历史 bug 残留的前导 `"…"`（旧: `src/SessionStore.cpp:1752-1755`）。
- **新版简化**：`content/thinking` 不再与 segments 冗余双写——segments 为权威，`content` 由 text 段拼接派生（对应架构文档 P2）。

### 2.2 气泡（ChatBubble）

旧: `ui/ChatBubble.qml` 全文

- **对齐**：user 消息靠右、头像槽在右；assistant 靠左、头像槽在左（`ui/ChatBubble.qml:226-227`）。
- **头像槽**：固定 64×58（`slotW/slotH`，`ui/ChatBubble.qml:74-75`），位于九宫格正文框**外侧**贴顶，不随行数拉伸；框贴图 `frame_chat_bubble_slot.png`，user 侧水平镜像。槽内头像 48×48，深色底 `#141018` + 2px 内边距，`PreserveAspectCrop`。
- **头像解析顺序**（`ui/ChatBubble.qml:42-56`）：
  - assistant：当前会话 meta 的 `agentId` → `agentStore.avatarFor(id)`（自定义头像存在则用）→ `userPrefs.agentAvatarUrl` → 内置 `avatar_agent.png`。
  - user：`userPrefs.userAvatarUrl` → 内置 `avatar_user_default.png`。
  - 配置页改头像后所有气泡立即刷新（旧版靠 `agentStore.revision`  bump 触发重绑定；新版用 Pinia 响应式）。
- **显示名**（`ui/ChatBubble.qml:59-65`）：assistant 用会话 meta `agentName`，空则 `"Agent"`；user 用 `userPrefs.userName`，空则 `"阿尔萨斯"`。头部行：`名字（蓝 #7eb6ff / 金 #f2cf6b）· HH:mm:ss · 状态`。
- **状态标签**（`ui/ChatBubble.qml:151-163`）：`生成中…`（金）/ `错误`（红 `#d08070`）/ `已中断`（灰金 `#a09070`）；`done` 不显示。错误气泡正文变红、玻璃底改 `#882a1518`；流式中气泡描边 `#66f2cf6b`。
- **segments 交错渲染**（`ui/ChatBubble.qml:379-517`）：
  - **流式中**：thinking/tool 段内嵌单行折叠块（见下），`text` 段纯文本增量展示。
  - **回合结束后（含历史消息）**：所有 thinking/tool 段**移出气泡**，气泡里只留一行 `⚙ N 个步骤（思考×a · 工具×b）`（不带工具名列表），点击打开 `ProcessDialog` 过程明细弹框（frame_popup 720×540）：按到达顺序逐条单行（思考暖色块 / 工具冷色块），点击展开 payload，整体一条 WarScrollBar，Esc/遮罩/「关闭」关闭；`text` 段仍内嵌 markdown。流式行不走此逻辑——live-append 契约要求末尾段挂在 DOM 上。
  - `thinking` 段（内嵌/弹框同式样）：暖色折叠块（底 `#44191510`、边 `#4a4232`、标题 `#c8b890`），标题行 `▶/▼ 思考过程`，**默认折叠**，点击切换；正文灰 `#908878`、小一号字。
  - `text` 段：正文，不可折叠，可选中。流式期间纯文本展示；**回合结束后 markdown 渲染一次**（新版：流式期向末尾文本节点增量 append DOM 文本；`status` 变为 done/error/interrupted 时用 markdown 渲染器整体替换该段为渲染结果，此后不再变）。
  - `tool` 段：单行 `▶/▼ · <名称>  [<status>]`（底 `#4412151c`、边 `#3a4a40`），**默认折叠**，点击展开 pretty-print 的 payload。名称取 `name || title || kind || "tool"`（`ui/ChatBubble.qml:192-195`）。payload 取 `rawInput || arguments || content || output`，对象则 `JSON.stringify(,,2)`（`ui/ChatBubble.qml:201-208`）。**新版变更：展开内容超过 64KB 时截断并追加 `…（已截断）`；完整 payload 只在磁盘 JSONL 中保留**（旧版不截断，长 payload 会撑爆 UI）。
- **展开状态保持**：`segOpen` 是气泡级 map，以 segment 下标为键（`ui/ChatBubble.qml:113-123`）。segments 只增不改结构，下标稳定；流式增量 append 不重建段列表，用户中途展开的思考/工具块不会在下一个分片到达时自动合上。新版必须以同等机制保证：增量 append 不触发整段重渲染。
- **复制按钮**（`ui/ChatBubble.qml:165-179, 360-374, 600-602`）：头部行尾部 `复制` 文字钮。**惰性填充**：只在点击时才把全文写入剪贴板（流式期间不维护副本）；复制后按钮文字变 `已复制`（绿 `#80f0a0`），1200ms 后恢复。复制内容 = 全部 text 段拼接（不含 thinking/工具块）；占位 `"…"` 在流式中显示为 `"…"`、结束后视为空（`displayBody`，`ui/ChatBubble.qml:140-144`）。
- **附件展示**（`ui/ChatBubble.qml:539-593`）：user 消息的 `attachments[]`（本地路径）。图片（`png/jpg/jpeg/webp/gif/bmp`）内嵌显示，宽 ≤280px、解码上限 560px，点击用系统默认方式打开；非图片显示为文件图标芯片（图标 + 文件名），点击同样系统打开。
- **无 segments 的兜底**（`ui/ChatBubble.qml:519-537`）：user 消息与 pending 占位行直接渲染 `content`。
- **宽度策略**（`ui/ChatBubble.qml:84-111, 277-279`）：整组（槽+缝+框）最大为列表宽 82%；流式中或含 thinking/tool 段的消息直接用最大宽（避免流式期框宽抖动）；短消息按内容自然宽收缩（旧版用隐藏测量件，新版可用 CSS `width: fit-content` + `max-width` 实现，不必复刻测量逻辑）。

### 2.3 流式增量追加

旧: `src/SessionStore.cpp:866-938`、`src/ChatController.cpp:356-372, 1184-1201`、`ui/ChatPage.qml:909-920`、`ui/ChatBubble.qml:125-138`

- Rust 侧把 ACP 分片按 50ms 合并 flush（待冲积 >64KB 时拉长到 250ms，旧: `src/ChatController.cpp:359-361, 368-370`），追加进当前会话消息模型的末条 assistant 消息：
  - 末段已是同 kind（text/thinking）→ **纯延伸**：只发轻量增量事件（`streamChunkAppended(sessionId, row, kind, chunk)`），前端对末段做 DOM append，不重渲染整段。
  - 否则（新增段 / 状态变化）→ 发结构变更事件，前端重建该气泡的段列表。
- tool_call / tool_call_update 到达前**先 flush 文本缓冲**，保证 text/tool 在段流中的到达顺序（旧: `src/ChatController.cpp:373-384`）。
- 前端对不在视口的气泡可跳过增量 append——下次进入视口时从 store 拿全量自愈（旧: `ui/ChatBubble.qml:127-128` 注释）。
- 运行期内存 buffer 只保留尾部 2000 字符（`kStreamBufferKeep`，`src/ChatController.h:166-168`）：全文由会话 store 逐段持有，buffer 仅服务断线续写（取尾 500 字）与判空。

### 2.4 滚动跟随

旧: `ui/ChatPage.qml:859-921, 2107-2135`

- `nearBottom`：距底 ≤80px 视为在底部。`followBottom` 默认 true。
- 用户上滚（拖动/惯性/滚动条按下）→ `followBottom = nearBottom`；程序化贴底不参与该判定（`_progScroll` 守卫）。
- `followBottom` 为 true 时：新消息、流式撑高、窗口缩放、回合开始（busy→true）都自动贴底。
- 发送成功、切换会话 → 强制 `scrollToEnd()`（恢复跟随 + 贴底）。
- **回到底部浮钮**：`消息数>0 && !nearBottom` 时出现在消息区右下（108×28 胶囊，`↓ 回到底部`），点击 `scrollToEnd()`。

## 3. 输入区（Composer）

### 3.1 字数上限 64K

旧: `ui/ChatPage.qml:33-55, 1288-1292, 1330-1361`

- `maxInputLen = 64000`。超过上限的输入/粘贴**直接丢弃尾部**并显示截断提示 `已达输入上限（64000 字），超出部分已截断；大段内容请作为附件发送`，6 秒后自动隐藏。
- 三条截断路径：① `textChanged` 兜底（IME 组词期间不动文档，避免打断候选）；② 按键拦截：已达上限且无选区、按键为可打印字符（无 Ctrl/Alt）时直接拒收（有选区放行——替换不增长度）；③ Ctrl+V 超长粘贴：先读剪贴板文本长度，只插入上限内的头部，避免整段巨型文本先排版再截断的卡顿。
- 字数指示 `N / 64000` 仅在超过 75% 时显示，触顶变红 `#ff8a70`，位于输入框**顶部**（小窗口下底部不可见）。
- @引用展开注入的文件内容**不受 64K 限制**（单文件 200KB 上限由读取侧保证，见 3.3）。

### 3.2 发送与快捷键

旧: `ui/ChatPage.qml:200-219, 1296-1329, 1662-1672`

- Enter 发送；Shift+Enter 换行；IME 组词期间 Enter 不发送。
- `send()`：trim 后为空且无附件 → 不发送。@引用先展开（3.3），再调 `sendUserMessage` 或 `sendUserMessageWithAttachments`。**仅成功才清空输入框与附件条**——队列已满/无会话时保留用户已输入内容。成功后若队列非空自动展开队列面板，并 `scrollToEnd()`。
- 发送按钮文案：空闲 `发送`；busy 时 `入队`；busy 且队列满（10 条）时 `已满` 且禁用。

### 3.3 @ 文件引用

旧: `ui/ChatPage.qml:76-198`、`src/SessionStore.cpp:1333-1453`

- **触发**：光标前最后一个 `@` 到光标之间的片段匹配 `^[^\s@]*(?:, ?[^\s@]*)*$`（路径段不含空白和 `@`，多段以 `, ` 分隔）时弹出选择器，锚在输入框上方、非模态、焦点留在输入框。
- **过滤**：当前段 = 最后一个逗号之后；剥离已输入的 `:起-止` 行号后缀后作为子串过滤词（忽略大小写），调用工作区文件列表（相对路径，排除忽略目录与二进制/图片扩展，上限 50 条）。段文本已精确等于某个文件路径（忽略大小写）→ 视为引用完成，不再弹出。无匹配 → 关闭。
- **选中**（点击 / Enter / Tab）：保留前面的逗号段与已输入的行号，只替换当前路径段为 `@path`；↑↓ 移动高亮，Esc 关闭。底部提示固定文案：`↑↓ 选择 · Enter 确认 · Esc 关闭 · 选中后可直接补 :起-止 行号，逗号可连引多个`。
- **行号语法**：`@path:10`（仅第 10 行）、`@path:10-50`（区间）。
- **发送时展开**（`expandReferences`，`ui/ChatPage.qml:158-173`）：正则 `/@([^\s@]+(?:, ?[^\s@,]+)*)/g` 匹配 token，逐段读文件展开为内容块，输入框里始终只保留短 token。展开格式（`refBlock`，`ui/ChatPage.qml:175-198`）：

  ```
  【引用文件：rel/path，第 10-50 行】       （无行号时为「全文」）
    10  <行文本>
    11  <行文本>
    …（文件超过 200KB，已截断）
  【引用结束】
  ```

- **读取规则**（`readFileRange`，`src/SessionStore.cpp:1371-1453`）：路径必须在工作区内（清理后绝对路径前缀校验，Windows 忽略大小写；绝对路径直接拒绝）→ 否则 `escape`；文件不存在 `missing`；不可读 `unreadable`；已知二进制/图片扩展或内容含 NUL → `binary`；`from<=0` 读全文；`to<=0` 读 `from` 单行；`from>总行数` → `range`；`to` 超界钳到末行。UTF-8 解码失败（含替换字符）回退系统本地编码（GBK）。单文件注入上限 200KB，截断时丢弃可能被切断的末行并标记 `truncated`。
- **失败占位**：读取失败时展开为单行说明，不阻断发送：`【引用文件：path：文件不存在或不可读】` / `二进制文件，已跳过` / `路径超出工作区，已拒绝` / `行范围超出文件（共 N 行）`。
- 工作区文件列表的忽略集与工作区文件树一致：`.git*`（任何以 `.git` 开头）、`node_modules`、`build`、`dist`、`.venv`、`__pycache__`、`.qt`、`.rcc`（`src/SessionStore.cpp:256-265`）。

### 3.4 粘贴图片

旧: `ui/ChatPage.qml:221-233, 1341-1345`

- Ctrl+V 时剪贴板里是图片 → **不作为文本粘贴**：存入媒体缓存（按日期 + 会话分目录，不写会话工作区），路径加入附件条（3.5）。
- 剪贴板无图片 → 走 3.1 的超长文本粘贴截断逻辑。

### 3.5 附件条

旧: `ui/ChatPage.qml:57-74, 1733-1837, 2163-2173`、`src/ChatController.cpp:953-1004`

- 📎 按钮打开系统文件对话框（多选；过滤器 `图片 (*.png *.jpg *.jpeg *.webp *.gif *.bmp)` / `所有文件 (*)`）；`file:///` URL 归一化为本地路径（反斜杠）。
- 附件条浮在输入框上方（聊天面板下缘），与发送队列/子 Agent 面板纵向堆叠（附件条在最上）。**最多 6 个**，去重；超出/重复直接忽略。
- 每个附件 56×56 芯片：图片显示缩略图（`PreserveAspectCrop`，解码 112px）；非图片显示文件图标 + 文件名（中部省略）。右上角 ✕ 角标移除。
- **发送规则**（`sendUserMessageWithAttachments`，`src/ChatController.cpp:961-1004`）：
  - 图片且当前 agent 声明了 `promptCapabilities.image` → ACP image block；否则（非图片，或 agent 不支持图片）→ 在 prompt 文本尾部追加 `\n[附件] <路径>`（agent 以 cwd 内工具自行读取）。
  - 文本为空但有图片 → 发送占位文本 `（图片）`。
  - busy 时附件消息**同样入队**（新版改为支持）：附件路径在粘贴时已落盘（媒体目录），队列元素为 `{text, images, display}`，排队不会丢失/失效；出队、插队（guideAt）均带附件发送。队列快照中带附件的条目显示 `文本 📎n` 标注。
- 气泡内的附件展示见 2.2。

### 3.6 提示词模板（已移除）

旧: `ui/ChatPage.qml:363-390, 1506-1642`

**Web 版已移除**：`模板`/`📎` 按钮与模板菜单均从 Composer 去掉（2026-07 按需求调整）；图片附件改由 Ctrl+V 粘贴进入。以下为旧 Qt 版行为存档：

- `模板` 按钮（与附件钮同款的四方蓝钮）弹出模板菜单，锚在按钮上方、右缘对齐按钮右缘，列出 PromptStore 全部模板（最多显示 8 行，超出滚动）。
- 点击模板 → 在**光标处**插入正文：前文非空且不以换行结尾时先补一个 `\n`，插入后走统一 64K 截断入口，焦点回输入框。
- 菜单底部固定项 `保存当前输入为模板`（输入框非空才可用）：模板名 = 首行前 20 字，正文 = 全文 trim。

### 3.7 Permission Mode 下拉

旧: `ui/ChatPage.qml:1644-1660`、`src/ChatController.cpp:125-136, 908-916`

- 向上展开的下拉（`WarDropdown`），宽 140。模式 id 与中文文案映射：
  - `default` → `需批准`；`plan` → `计划`；`auto` → `自动`；`yolo` → `YOLO`
- 持久化到 user_prefs（全局，非按会话）。切换后立即对活动会话进程生效（`session/set_config_option`），后台会话下次 prompt 时生效。
- 发给 agent 前经 ProviderRegistry 映射（kimi 系 id 原样；claude：`auto→acceptEdits`、`yolo→bypassPermissions`；映射表见 sessions-and-config.md 附录与 ../acp-protocol.md）。
- 状态行后缀同步显示 `· 需批准` 等（见 6.1）。

### 3.8 `/` 斜杠命令补全（ACP available_commands_update）

新实现（无旧码对应）：`src-tauri/src/acp/client.rs`（`available_commands_update` 分派）、
`src/stores/chat.ts`（`acp://commands` → `commandsBySession`）、
`src/components/chat/Composer.vue`（slash 补全弹层）。

- agent 通过 `available_commands_update` 下发命令列表（如 kimi 的 `/tasks`）；
  协议层存 `available_commands` 并发 `AcpEvent::AvailableCommands`，回放期只存不发，
  session_ready 补发（状态非历史，load 会话不丢命令）。
- 输入框内容恰为 `/<filter>`（首 token、无空白）时按子串过滤弹补全层，
  样式复用 @ 引用弹层；↑↓ 选择、Enter/Tab 确认、Esc 关闭。
- 选中只把 `/name ` 补进草稿——**命令由 agent 执行**，选中后继续输入参数，
  发送走普通 prompt 路径（客户端不做命令解析）。
- 前端按 sessionId 缓存命令列表（`commandsBySession`），会话切换不丢后台会话的
  命令（事件只在该 agent 下发时重发）；删除会话时清缓存。

### 3.9 ACP plan 更新 → 计划卡片

新实现（无旧码对应）：agent 的 `plan` update（`entries: [{content, status, priority?}]`）
经 `AcpEvent::Plan` 到 chat 层，构造 `{toolCallId:"plan", kind:"plan", title:"计划"}` 段
走工具段 upsert 通道（重复更新按 id 替换）；ChatBubble 识别 `toolCallId === 'plan'`
渲染为计划卡片：○ pending / ▶ in_progress / ✓ completed。

- 计划卡片在流式与最终行都内嵌可见，**不进**「⚙ N 个步骤」过程行/过程对话框
  （`structSegs` 过滤掉 plan 段）。
- 状态符号着色：completed 绿色，其余 muted。

## 4. 浮动件

### 4.1 发送队列面板

旧: `ui/ChatPage.qml:1839-1969`、`src/ChatController.cpp:920-951, 1109-1180, 1386-1396`

- busy 时发送消息 → 入队（**上限 10 条**，`kMaxQueueSize`，`src/ChatController.h:47`；含附件消息，条目以 `📎n` 标注）；满则拒绝并报错 `队列已满（最多 10 条）`，输入框内容保留。
- 回合结束后自动取出队首发送（`drainQueue`）。
- 面板浮在输入框上方，`queueLength>0` 时可见。标题行 `▶/▼ 发送队列 (n/10)` 点击折叠/展开（展开最多 6 行高）+ `清空`（红）。发送后若队列非空自动展开；队列清空自动折叠。
- 每行：`序号. 预览（空白压缩、截 48 字加…）` + 两个操作：
  - `引导`（金色，guideAt，见 7.3）——插队：中断当前回合，立刻发送该条。
  - `移除`（红色）——从队列删除。

### 4.2 子 Agent 面板（SubagentPanel）

旧: `ui/SubagentPanel.qml` 全文、`src/ChatController.cpp:485-652`

- 展示当前会话**当前回合**的子 Agent 调用。`subagents.length > 0` 时可见，位于发送队列面板上方。
- 头部 `▶/▼ 子 Agent (执行中 n / 共 m)`，点击折叠/展开（展开上限 160px）。
- 条目字段：`{id, kind, title, status, children, childNames, summary, input, output, startedAt, finishedAt, lastUpdate}`；`status ∈ pending | in_progress | completed | failed | interrupted`。
- 每行：状态圆点（in_progress 绿 / pending 金 / failed 红 / interrupted 金 / 其他灰；pending/in_progress 时 550ms 呼吸闪烁）+ 标题（`children>0` 时前缀 `[N 个子任务] `，completed 后标题变灰）+ 右侧 `状态中文 · summary · 耗时`（`执行中/等待/完成/失败/中断`；耗时 `<60s` 显示 `Ns`，否则 `NmNs`）。
- **1s 心跳**：仅面板可见且有 pending/in_progress 条目时运行，驱动耗时每秒刷新。
- **卡住提示**：live 条目距 `lastUpdate`（最后一次 tool_call/update 触碰）超过 120s 无更新 → 行尾红字 `可能卡住`。
- **点击条目 → `SubagentDialog` 详情弹框**（frame_popup 760×680）：标题 + `kind · 状态 · 已用时 · 距上次更新`（卡住阈值同上面板，超时红字 `可能卡住 · 无更新 Ns`）+ swarm 子任务 chips + 「任务书」（`input`：单 Agent 的完整 `prompt`，否则整个参数 JSON，≤32KB）+「最终报告」（`output`：完成时的 `rawOutput`，≤64KB；执行中显示占位）+「执行过程」（见下）；各区带 WarScrollBar；底部「停止回合」（仅 live 且回合 busy 时显示，调 `session/cancel` 停整个回合——ACP 无法单独杀一个子 Agent，弹框内有说明）+「关闭」，Esc/遮罩关闭。弹框按 id 实时跟随 `chat.subagents` 更新。
- **执行过程（kimi CLI 专属优化，读盘不经主 Agent）**：仅当会话 `provider === 'kimi'` 时弹框才显示此区；其他 ACP CLI 保持「任务书+最终报告」的通用视图（各自原生行为，不做私有适配）。kimi CLI（含 ACP 方式）把每个子代理的完整事件流写在 `~/.kimi-code/sessions/<项目目录>/<acpSessionId>/agents/<agentId>/wire.jsonl`；`agentId` 来自 rawOutput 的 `agent_id:` 行（后端提取存 `agentIds[]`，**完成后才有**）。`subagent_process` 命令（`inspect/subagent.rs`）按会话 meta 的 `acpSessionId` 定位 wire 文件（全项目目录 glob，id 白名单校验防穿越），解析 `context.append_loop_event`：`tool.call`（名称+description+参数）、`tool.result`（输出）、`content.part` 的 think（`think` 字段）/text；保留最后 400 步、每步 ≤4000 字。弹框自动加载首个 id（多 id 时显示切换 chips），可手动「刷新」。**脆弱点：kimi-code 私有格式，CLI 升级可能变**；文件被清理时显示错误占位。
- **跟踪规则**（`trackSubagentCall`，`src/ChatController.cpp:534-628`）：
  - ACP 无专用子 Agent 事件，CLI 以普通工具调用上报。识别工具名（忽略大小写）：`agent` / `agentswarm` / `task` / `spawn_agent`。
  - `tool_call_update` 不携带名称——新条目必须来自带名的 `tool_call`；后续 update 仅按 `toolCallId` 匹配已跟踪条目。
  - 输入参数流式到达：content 块是累积快照（kimi 末块为准）也可能是增量——先尝试解析最后一个块的 JSON，失败再试全部块拼接；可解析后取 `description` 作标题（否则 `prompt` 截 48 字），`items[]` 填充 `children/childNames`；完整任务书存 `input`（`prompt` 全文或 pretty JSON，≤32KB）。每次触碰写 `lastUpdate`。
  - 完成时从 `rawOutput` 提取摘要：swarm 结果 `<agent_swarm_result>` 统计 `outcome="completed"` 占比 → `完成 x/y`；单 Agent 结果取 `actual_subagent_type:` 行；`rawOutput` 全文存 `output`（≤64KB）；`agent_id:` 行提取存 `agentIds[]`（wire 目录名，执行过程用）。
  - 回合结束时仍 pending/in_progress 的条目：正常结束 → `completed`；被中断 → `interrupted`，并写 `finishedAt`。
  - 新回合开始时清空面板。

### 4.3 限流重试横幅

旧: `ui/ChatPage.qml:2054-2106`、`src/ChatController.cpp:50-64, 404-438, 1233-1340`

- **触发**：`session/prompt` 以错误结束（`turnFinished("error")`）且错误文本（JSON-RPC error，先由 `protocolError` 事件暂存）匹配限流特征——忽略大小写包含 `429` / `rate limit` / `ratelimit` / `too many requests` / `quota` / `resource exhausted`。
- **策略**：最多自动重发 3 次（`kMaxRateLimitRetries`），退避 20s → 40s → 80s（`kRateLimitBaseDelaySec=20` 指数翻倍），单次上限 300s（`kRateLimitMaxDelaySec`）。
- **仅纯文本 prompt 可重发**：含 image block 的回合不重试（base64 不留存、重读文件可能拿到变更内容），走普通失败路径。非图片附件已内联为 `[附件] path` 文本，可正常重发。
- **行为**：回合保持 busy，不重写历史——用户行已持久化，重发的回复流回**同一个** assistant 气泡。气泡正文先被替换为静态提示 `请求被限流，N 秒后自动重试（第 x/3 次）…`；横幅显示实时倒计时 `请求被限流，N 秒后自动重试（第 x/3 次）` + `取消重试`（每秒滴答；状态行同步显示同款文案）。
- **触发重发**：倒计时归零 → 气泡重置为 `"…"` 占位，进程活着直接重发同一 prompt；进程已死走 pendingPrompt 握手路径（进程就绪后自动发出）。
- **取消**：点 `取消重试` / 用户发新消息 / guideAt 插队 / 手动停止 → 停表，气泡落定 `回合失败：请求被限流，已取消自动重试`（error），回合关闭。进程退出时挂起的重试随之取消（不 finalize，交给中断/续写逻辑）。

### 4.4 内存压力横幅

旧: `ui/ChatPage.qml:255-259, 1987-2052`

- PerfMonitor 看门狗检测到内存压力时，聊天区顶部浮出金边横幅：`内存占用较高（N MB），建议重启应用释放` + `知道了`。
- 纯提示性质，不杀任何东西。`知道了` 仅隐藏横幅；压力状态解除（低于复位阈值）后重置 dismissed，再次触发时横幅会重新出现。压力持续期间每 60s 的重复信号只刷新 MB 数值，不取消用户已点的隐藏。
- 与限流横幅同时出现时，限流横幅下移（y 12 → 52）。

### 4.5 权限确认对话框

旧: `ui/ChatPage.qml:2149-2161, 2830-2949`、`src/ChatController.cpp:385-403, 654-674`

- ACP 反向 RPC `session/request_permission` 到达时（仅活动会话弹窗；后台会话只在 rail 亮金点，切过去才弹）弹出模态对话框，标题 `工具权限请求`，`closePolicy = NoAutoClose`（Esc 不关）。
- **消息体**：标题行 = `toolCall.title || toolCall.kind || "Agent 请求执行工具"`；明细 = content 块的 text 拼接（否则 rawInput 的 `path/file_path/abs_path`），空白压缩；超过 160 字做中部省略（头 78 + ` … ` + 尾 78）。明细为空或与标题相同则只显示标题。
- **动态 options**：按钮来自 ACP `options[]`，点击回传 `optionId || id`。通用批准名中文化（`optionLabel`，`ui/ChatPage.qml:2893-2915`）：
  - `approve once` / `allow once` / `allow` / `approve` → `允许一次`
  - `approve for this…`（前缀）/ `allow always` / `always allow` → `总是允许`
  - `reject` / `reject once` / `deny` → `拒绝`
  - `reject always` / `always reject` → `总是拒绝`
  - 其余保留原文（AskUserQuestion 的真实选项文本必须原样显示）；无 name 时按 kind 兜底：`allow_once/allow_always/reject_once/reject_always` → 四态文案，再兜底 `选项`。
- **布局**：>3 个选项时两列网格（AskUserQuestion 多选项场景），≤3 单列；按钮按可用高度自适应。
- **AskUserQuestion 分组模式**：kimi acp 适配器把 AskUserQuestion 桥接进 `request_permission`，option id 带 `q{n}_opt_{i}` / `q{n}_skip` 命名空间（acp-protocol.md §5.1）。Rust 侧（`acp/types.rs::parse_question_request`）解析成问题分组随 `acp://permission` 的 `questions` 字段下发；非空时对话框切换为分组渲染：每个问题独立一节（问题文本 + 选项按钮 + `跳过`），多问题标注 `问题 i/n`，`multi_select` 问题附提示。应答仍回传单个 optionId（ACP 一次响应只能带一个选项，与 kimi 适配器的窄化语义一致；`multi_select` 同样按单选应答）。注意：kimi 0.29.x 适配器自身会把多问题请求降级为第一问（agent 侧丢，线上抓包实证），客户端解析/渲染是前向兼容的那一半。
- **无 options 兜底**：显示 `允许`（回传 `"allow"`）/ `拒绝`（回传空 + `cancelled=true`）两钮。
- 回应后清除请求状态；后台会话的权限请求不抢焦点（`src/ChatController.cpp:396-402`）。回合结束/进程切换时未决请求自动清除（拒绝语义）。

## 5. 左栏：本项目会话 rail

旧: `ui/ChatPage.qml:281-361, 521-767, 2139-2147, 2175-2243`

- 数据源：当前会话所属项目的全部会话（`sessionsForProject`），置顶在前（稳定排序，组内仍 updatedAt 倒序）；无项目会话归入 `""` 组。
- 顶到下：`本项目会话` 标题 → `＋ 新会话` 按钮 → 搜索框（占位 `搜索会话…`，标题子串即时过滤、忽略大小写）→ 会话列表 → 底部图例行 `● 执行中 ● 等待 ● 空闲`（绿/金/灰三圆点富文本着色）。
- **每行**：状态圆点（running 绿 `#57d977` / waiting 金 `#f2cf6b` / idle 灰 `#4a5265`；非 idle 时 550ms 呼吸闪烁，回 idle 复位不透明）+ 标题（当前会话金色加粗）+ 副行 `N 条`（running 追加 `· 执行中`，waiting 追加 `· 等待确认`；未读前缀 `NEW · ` 蓝）+ 右上角 📌（置顶时）。
- **点击**：切换会话（后台会话继续运行；仅切活跃指针 + 重绑消息模型，延迟一拍执行避免事件栈内切页）。切走时对**从未发言的空会话**执行 `discardIfEmpty`（见 7.4）。
- **右键菜单**：`置顶会话/取消置顶`、`重命名会话`（行内 TextInput：Enter 确认——trim 非空才提交，标题上限 48 字；Esc 取消）、`复制会话内容`（transcript 到剪贴板，见 7.5）、`基于此提问`（同项目新建空会话，不复制历史，输入框预填 `基于会话「<标题>」：`——经一次性 pendingComposerText 传递，ChatPage 消费后清空并聚焦）、`删除会话`（确认对话框 `确定删除这条会话及其全部消息吗？\n该操作不可撤销。` → 先 `closeRuntime` 停运行时，再删盘；删当前会话则页面落到空会话态）。
- **行内重命名时**：该行 MouseArea 禁用；页面级 Esc 快捷键让位给输入框（取消编辑而非返回主菜单）。
- 空列表显示 `（无会话）`。
- 会话切换后：刷新 rail、刷新 git 分支、消息列表贴底。

## 6. 右栏：面板坞（会话信息）

> **架构变更**：右栏不再是写死的固定区域，而是**可扩展信息面板坞**——手风琴堆叠、拖拽调高、布局记忆。容器/交互/铁框规范以 [../panels.md](../panels.md) 为权威；本节只定义首批面板的功能内容（`agent`/`git`/`files`，见 panels.md §2 映射表）。

### 6.1 标题行与 Agent 切换器

旧: `ui/ChatPage.qml:792-849`、`src/ChatController.cpp:741-808`

- 聊天面板顶部：会话标题（金）+ 状态行（`chatController.statusText`）+ **思考档位下拉**（见下）+ Agent 切换器（`WarDropdown` 下拉框样式，bar 显示 `◆ <name>`，列表项 `name · provider`）。
- **思考档位下拉（kimi 专属，新版）**：ACP `session/new|load` 返回的 `configOptions[]` 里有 `id="thinking"` 的 picker 时才显示（kimi 总有，档位由模型 `support_efforts` 决定，如 Low/High/Max；DeepSeek 类布尔思考的模型为开/关式档位；其他 ACP CLI 不上报则不显示）。bar 显示 `思考 <当前档名>`；选择 → `set_config_option {configId:"thinking", value}`，刷新后的 options 经 `acp://configOptions` 回来更新 currentValue（`config_option_update` 通知也会刷新）。切会话/删会话清空。自定义 baseUrl 的模型想启用强度档：在配置页给 Agent 设"默认思考强度"，保存时 WarDex 会把该模型以 `support_efforts` + `default_effort` 写入 `~/.kimi-code/config.toml`（`[providers.wardex-<host>]` + `[models."<id>"]`，文本级 section 改写，不动其他内容；清空强度则移除模型 section），之后该模型走 config.toml 别名路径（不再 KIMI_MODEL_* env 合成），picker 即显示 Low/Medium/High/XHigh/Max。
- **模型下拉（同机制 + 端点模型合并）**：候选 = `configOptions[]` 里 `id="model"` 的 picker 选项 ∪ 当前 Agent `baseUrl` 的 `fetch_models`（`/models`，随 agent/ baseUrl 变化重拉；baseUrl 为空则不拉），Agent 配置的 model 两边都没有时也兜底显示。bar 显示 `模型 <当前模型名>`。选择 picker 内值 → `set_config_option {configId:"model", value}`（热切换）；选择端点独有值 → `set_session_model`：runtime 把模型经 `update_agent` 持久化到 Agent 记录、清 `acpSessionId`（恢复的会话会保留 CLI 记忆的模型，env 注入只在 session/new 生效）、杀掉当前进程（忙时先按中断处理）并重新 spawn，由 build_launch 的 `KIMI_MODEL_*` env 注入生效。Agent 配置了默认模型时，**新会话**（非 session/load 恢复的）在首个 configOptions 到达后自动应用一次（仅当该值在 picker 选项内且 ≠ currentValue）；恢复的老会话尊重 CLI 侧记忆，不覆盖。
- 状态行组成（`refreshStatusLine`，`src/ChatController.cpp:1398-1427`）：`限流，N 秒后自动重试（第 x/3 次）…` / `等待批准…` / `连接 ACP…` / `生成中…` / `就绪`，后缀 `· 需批准|计划|自动|YOLO`，队列非空再后缀 `· 队列 n/10`。
- 点击芯片弹出 Agent 菜单：全部 agent，每项 `✓/　 + name + · provider`；当前项与 `canUseForChat=false` 的项禁用。选择 → `switchAgent(agentId)`：
  - 同 id → no-op；agent 不存在/不可用 → 报错 `Agent 不可用，请在配置页检查`。
  - **忙中切换 = 强制取消**：与 cancel 同语义中断回合（标记已中断），清 pendingPrompt/pendingGuide/权限请求/重试。
  - **同 provider**：保留 agent 侧 `acpSessionId`，新进程经 `session/load` 恢复 agent 侧历史；**跨 provider**：丢弃 `acpSessionId`，走 `session/new` 全新 agent 侧会话。两种情况下 WarDex 本地历史都不动。
  - 切换持久化到会话 meta（`agentId/agentName/provider`），重开会话沿用新 agent；后台预热新连接；状态行显示 `已切换 Agent · <name>`。

### 6.2 会话简介块

旧: `ui/ChatPage.qml:945-990, 1213-1222`

- `会话简介` 标题；`agentName · provider`（金，最多 2 行）；`消息 N · yyyy-MM-dd HH:mm`（updatedAt）；分隔线；`工作目录` + 完整路径（任意位置换行）。
- 底部错误行：`chatController.lastError` 非空时红字显示（含启动期错误，如无可用默认 agent）。

### 6.3 Git 分支徽标 + 提交历史

旧: `ui/ChatPage.qml:392-398, 991-1133`、`src/SessionStore.cpp:1455-1488`

- **分支徽标**：读 `.git/HEAD`（无进程开销；支持 worktree gitfile：`gitdir: <path>` 指向真实 HEAD）。`ref: refs/heads/x` → `x`；`ref: 其他` → 去 `ref: ` 前缀；detached → 短 SHA 前 7 位。非 git 目录时徽标与提交历史**整体隐藏**。显示 `⎇ Git <branch>`（分支名中部省略）。
- 刷新时机：进页面、切会话、**回合结束**（agent 可能在回合里切换/新建分支或提交）。
- **提交历史**（只读）：`git log` 拉取最近 200 条，列表最多显示约 4 行高（132px）超出内部滚动。每行：`subject` + `shortHash · author · date`。`刷新` 链接（加载中显示 `加载中…`）；错误红字；空显示 `暂无提交`。刷新时机同分支徽标（手动点刷新亦可）。新版用 Rust 侧执行 `git log --format` 或读 git 库，交互语义不变。
- **提交详情弹框**：点击历史中的提交（`git_diff_commit` 一次取全提交 diff，前端按文件过滤）或「更改」列表中的文件（`git_diff_file`，标题为文件路径，副标题标注 工作区/已暂存/未跟踪）→ 同一个 `GitCommitDialog` 弹框。左侧为涉及的文件列表（含 +增/−删行数），点击文件 → 右侧 GitLab 风格 diff 视图（文件头、新旧双行号、+/− 标记列、增行绿底/删行红底、hunk 头蓝底）；两侧内容溢出时各带一条 WC3 滚动条（diff 区超长行另有原生横向滚动条）；鼠标移到弹框外圈石块边框上可八向拖拽改大小（最小 520×360）；Esc/点遮罩/底部「关闭」按钮关闭。

### 6.4 工作区文件树

旧: `ui/ChatPage.qml:1134-1212`、`src/SessionStore.cpp:185-340, 1105-1114`

- 扁平化可见行树：目录懒加载，展开时子行插入父行之后；折叠移除所有后代行。**无文件系统监听**——根目录在会话打开/回合结束/手动刷新时重读，重读时所有展开状态坍缩。
- 忽略集同 3.3；目录在前、按名称忽略大小写排序；目录行仅在有可见子项时显示 ▶/▼ 箭头；缩进 `depth × 14px`；目录金色 `#e8d9a0`、文件灰白 `#d0d6e0`；文件夹/文件图标。
- **点击**：目录 → 展开/折叠；文件 → 预览弹窗（6.5）。**右键** → 菜单 `打开（系统默认方式）`。
- 空目录显示 `暂无产出文件`。
- 底部操作湾的 `刷新工作区(R)` 按钮（busy 时变为 `停止生成`）手动重读 + 刷新 git 分支。

### 6.5 文件预览弹窗

旧: `ui/ChatPage.qml:422-471, 2270-2741`、`src/SessionStore.cpp:1226-1331`

- **入口判定**：`previewFile` 返回 `{ok, size, image, text, truncated}` 或 `{ok:false, size, reason: missing|unreadable|binary}`。图片扩展（png/jpg/jpeg/gif/webp/bmp）不读文本直接 `image:true`；已知二进制扩展或头部 256KB 内含 NUL → `binary`；UTF-8 失败回退本地编码。**文本预览上限 256KB**，超出 `truncated:true`。
- **>2MB 先询问**（`previewAskSize = 2*1024*1024`）：对话框 `文件较大` —— `<name> 约 X.X MB，超过 2MB。\n继续预览可能较慢，如何选择？`，三钮 `继续打开` / `外部打开`（系统默认方式）/ `取消`。
- **二进制/读取失败**：对话框 `无法直接预览` —— `<name> 不是文本文件，无法直接预览。\n是否用系统默认方式打开？`，`打开` / `取消`。
- **弹窗本体**：模态、Esc 关闭、frame_popup 铁框。默认尺寸 A4 竖版（210:297）适配窗口 92%；用户拖过大小则用 userPrefs 持久化的 `previewWidth/Height`。标题 = 文件名。
- **三种正文**（kind 判定：image / 扩展名 `.md|.markdown` → markdown / 其余 text）：
  - **text**：行号槽（逻辑行数变化时才重建；不换行模式下与正文同字体逐行对齐）+ 可编辑文本区（Consolas 12）。工具行：`换行：开/关` 切换（关时实测最长行像素宽出横向滚动条——粗筛 `行字符数×7 ≤ 已知最大宽` 跳过测量，最多测 10000 行）；`保存` 按钮仅 dirty 且未截断时可见。`truncated` 文件**只读**（状态行 `文件过大，仅显示前 256KB，内容只读`——整文件回写会丢数据）。
  - **markdown**：默认渲染态（只读），工具行 `显示原文/渲染预览` 切换原文（可编辑）↔ 渲染。未保存编辑经 `currentRaw` 在两种组件间传递不丢失。
  - **image**：gif/webp 用动画组件，静态图用普通图片组件；解码上限 1024px，等比缩放适配宽度，超出滚动。
- **编辑保存**：打开时 `\r\n` 归一化为 `\n`（避免假 dirty）；dirty = 当前文本 ≠ originalText；`保存` → 整文件 UTF-8 写回（拒绝二进制/图片：扩展名短路 + 头 4096 字节 NUL 嗅探）；成功状态行 `已保存`，失败 `保存失败：<原因>`；dirty 时清空状态行。
- **边缘拖拽调大小**：四边 12px 热区 + 四角 26×26 热区，最小 380×480，钳在窗口内；松开时把宽高持久化到 userPrefs。新版拖拽热区 hover 时无需复刻手套光标变绿（光标体系见 ../ui-design.md，有则沿用）。
- **关闭即彻底卸载**：正文组件销毁、文本副本清空，不残留文档/图片内存。

## 7. 回合生命周期与异常路径

### 7.1 发送与流式回合

旧: `src/ChatController.cpp:1006-1068`

`startSend` 的顺序必须严格保持：

1. 追加 user 行（含展示附件列表）→ 追加 assistant 占位行（内容 `"…"`、status `pending`）。
2. 清运行期 buffer、丢弃陈旧待冲积分片（防止落进新占位行）、`continueRetries=0`、`userStop=false`。
3. 缓存 `retryPrompt`（纯文本才缓存，见 4.3）、`retryAttempt=0`、清 `lastTurnError`；清空 subagent 面板。
4. provider 未注册（非 kimi/claude/codex/custom）→ assistant 行落定错误 `Provider «x» 未注册，请在配置页选择 kimi / claude / codex / custom。`，回合结束。
5. ACP 进程未就绪 → prompt 存 `pendingPrompt`，启动进程，`started` 握手完成后自动发出（含图片路径）。
6. 就绪 → 先同步 mode，再 `session/prompt`。

### 7.2 取消（cancel）

旧: `src/ChatController.cpp:1070-1107`

- 有挂起的限流重试 → 先取消重试（finalize），不发送 `session/cancel`。
- 清 `pendingGuide`/`pendingPrompt`；有未决权限请求 → 以 cancelled 回应。
- busy → `userStop=true` + `session/cancel`。**2.5s 超时强杀**：agent 无视 cancel 则停掉进程（其回台状态未知，必须重启连接防 desync）、标记已中断、关闭回合。
- `turnFinished` 的 stopReason 归一：`userStop` 或 `cancelled/canceled` → `interrupted`；`error` → `error`；其余 → `done`。空内容被中断 → 正文 `（已中断）`；`done` 且内容仍是占位 `"…"` → `（空回复）`（`src/SessionStore.cpp:1030-1033`）。

### 7.3 guideAt 插队引导

旧: `src/ChatController.cpp:1129-1180`

- 从队列取出目标条；未决权限请求先以 cancelled 回应（否则 `session/cancel` 落不下去，旧对话框会楔在新回合上）；挂起的限流重试取消（finalize）。
- 不 busy → 直接 `startSend`。
- busy → 存 `pendingGuide` + `userStop` + `session/cancel`；**800ms 超时**：回合已停 → 发送 guide；回合仍活着 → **杀进程**（否则旧分片会流进 guide 的新气泡，且 `m_turnBusy` 未清会拒发）、标记已中断、发送 guide。
- `finishReply` 后若仍有 `pendingGuide`（正常取消路径），优先于队列 drain 发送。

### 7.4 空会话丢弃（discardIfEmpty）

旧: `src/ChatController.cpp:274-286, 716-718, 817-819`、`src/SessionStore.cpp:400-412`

- 新建后从未发言的会话（`messageCount == 0` 且不 busy、队列空）：切走（openSession）或再建会话（startNewSession）时直接删除——停运行时、删盘、下索引。
- 应用启动时同样清理磁盘上遗留的空会话目录。

### 7.5 复制 transcript

旧: `src/SessionStore.cpp:1490-1536`

- `复制会话内容`：整段用户可见 transcript 进剪贴板，逐行格式 `User: <content>` / `Assistant: <content>`，跳过 pending assistant 行，不含 thinking/工具块。已打开的会话用内存模型，未打开的直接解析 JSONL（不留存模型）。无内容可复制 → 失败并报错 `会话没有可复制的消息`。

### 7.6 断线续写（resumeInterruptedTurn）

旧: `src/ChatController.cpp:340-355, 445-463, 1210-1231`

- ACP 进程退出且回合 busy：非用户停止、已有部分输出（buffer 非空）、`continueRetries < 2` → **自动续写**：
  - 合成续写 prompt（**不写入历史**）：`上一条回复因连接中断被截断。请紧接着已输出的内容继续，不要重复已输出的部分，不要重新开头，不要解释。已输出内容的结尾片段：\n…<尾部 500 字符>`。
  - 尾部锚点保证即使 `session/load` 失败退回全新 agent 侧会话，模型也能接上。回复流回**同一气泡**。
  - 状态行 `连接中断，自动续写…`；经 `ensureAcp` 重启（尽量 `session/load` 恢复）。
- 续写启动失败（`startFailed`）：已有部分输出 → 保留部分内容标记中断（不用错误文本覆盖）；无输出 → assistant 行落定 `ACP 启动失败: <err>`（error）。
- 不满足续写条件（用户停止 / 无输出 / 重试耗尽）→ `markAssistantInterrupted`：空内容 → 替换为 `（已中断）`；有内容且不含「已中断」→ 尾部追加 `\n\n（已中断）`；落盘 status `interrupted`，子 Agent 全部置 interrupted。

### 7.7 多会话并发与未读

旧: `src/ChatController.cpp:293-317, 1359-1384`、`src/SessionStore.cpp:27-28, 385-398, 508-538`

- 每个并行会话一个 runtime（ACP 进程 + 回合状态）；并发进程软上限 3（`kMaxParallelAcp`）——超限时停最久未活动的**空闲**进程（会话经 `session/load` 恢复）；全忙时允许临时超限。
- 后台 runtime 继续流式写入自己的会话模型与磁盘。后台回合完成 → 该会话标未读（rail 行 `NEW` 前缀 / 会话选择页 NEW 徽标）；打开该会话即清除。未读是运行期标记，不落盘。
- 前台/后台共用同一条代码路径；QML 可见属性（busy/statusText/queueLength/permissionRequest/subagents/retry*）始终代理**活动** runtime。

## 8. 快捷键与其他

旧: `ui/ChatPage.qml:1710-1728, 2139-2147`

- `Esc` / `B`：返回主菜单（行内重命名会话时 Esc 归输入框）。聊天页常驻（主菜单字母快捷键仅在主菜单生效，见 main-menu-and-misc.md）。
- `R`：空闲 `刷新工作区(R)` / busy `停止生成`（同一按钮双态）。
- 字号缩放 `fs()`：输入框、消息正文、rail、右栏阅读文本按 `userPrefs.fontScale`（0.85~1.30）缩放；装饰性图标字符（📎 📌 ⎇ ✕ ▶▼）不缩放（详见 ../ui-design.md 与 main-menu-and-misc.md）。
- 页面 cached Loader 常驻（见 main-menu-and-misc.md）：切出聊天页再回来，消息列表、输入框草稿、队列面板展开态、滚动位置全部保留。新版用 Vue `<KeepAlive>` 等价实现。

## 实现检查清单

消息列表与气泡
- [ ] segments 交错渲染：流式中 thinking 暖色折叠（默认折叠）、text、tool 行（默认折叠）按到达顺序排列；回合结束后 thinking/tool 收成一行 `⚙ N 个步骤`，点击开 ProcessDialog
- [ ] 流式增量 DOM append（50ms 合并、积压 >64KB 降频 250ms），绝不整段重绑定；markdown 回合结束后渲染一次
- [ ] user 右 / agent 左气泡；头像槽 64×58 在框外贴顶；头像三级回退解析；改头像即时刷新
- [ ] 头部行：显示名（阿尔萨斯兜底）· HH:mm:ss · 状态标签（生成中…/错误/已中断 + 对应配色）
- [ ] 复制按钮惰性填充 + `已复制` 1200ms 反馈；复制内容仅 text 段拼接
- [ ] segOpen 展开状态在流式期间保持（增量 append 不重建段列表）
- [ ] 工具块展开 pretty-print payload，前端 64KB 截断 + `…（已截断）`，完整内容只落盘
- [ ] 附件：图片内嵌（≤280px，点击系统打开）、非图片芯片
- [ ] 滚动跟随：80px nearBottom 阈值、用户上滚暂停、发送/切会话恢复、回到底部浮钮
- [ ] 占位行 `"…"`、`（空回复）`、`（已中断）` 三种落定形态

输入区
- [ ] 64K 上限三条截断路径（textChanged 兜底含 IME 守卫 / 按键拒收 / 粘贴预量长度）+ 75% 计数器 + 6s 截断提示
- [ ] Enter 发送 / Shift+Enter 换行 / IME 组词 Enter 不发送；仅发送成功才清空
- [ ] @引用选择器：触发正则、逗号多段、`:起-止` 行号保留、精确匹配自动关闭、键盘导航、提示文案
- [ ] 发送时展开为 `【引用文件：…】` 块；escape/missing/unreadable/binary/range 五种失败占位；200KB 注入上限；GBK 回退
- [ ] Ctrl+V 图片 → 媒体缓存 + 附件条；附件条 ≤6 去重、缩略图/图标芯片、✕ 移除
- [ ] 附件发送规则：图片 + imageSupported → image block，否则 `\n[附件] 路径` 内联；空文本纯图片发 `（图片）`；busy 拒收附件消息（报错文案）
- [ ] ~~模板菜单~~（已移除，见 §3.6）
- [ ] permission mode 下拉四态中文化、持久化、ProviderRegistry 映射、状态行后缀

浮动件
- [ ] 发送队列：上限 10、忙中文本入队、面板折叠/清空/引导/移除、发送后自动展开、清空自动折叠、drainQueue
- [ ] SubagentPanel：五种状态、1s 心跳耗时、swarm 子任务计数与 `完成 x/y` 摘要、回合结束兜底置 completed/interrupted、新回合清空
- [ ] 限流重试：错误特征匹配、20/40/80s backoff 上限 300s、仅纯文本可重发、倒计时横幅 + 取消重试、三处取代路径（新消息/引导/手动停止）
- [ ] 内存压力横幅：知道了隐藏、压力解除后复位、60s 重复信号只刷新数值
- [ ] 权限对话框：动态 options、四态中文化 + AskUserQuestion 原文保留、>3 选项两列网格、无 options 兜底、NoAutoClose、后台会话不抢焦点

左右栏
- [ ] rail：置顶在前、状态圆点呼吸、NEW 未读、标题即时过滤、右键五菜单项、行内重命名（Enter/Esc、48 字上限）、基于此提问预填、删除确认（先 closeRuntime）
- [ ] Agent 切换器：同 provider 保 acpSessionId / 跨 provider session/new、忙中强制取消、持久化 meta
- [ ] git 分支徽标（.git/HEAD 直读、worktree gitfile、detached 短 SHA）+ 只读提交历史（200 条、回复完成时刷新）
- [ ] 工作区文件树：懒加载展开、忽略集、刷新坍缩展开态、点击预览 / 右键系统打开
- [ ] 文件预览：>2MB 三选询问、二进制询问、256KB 截断只读、行号槽、换行开关、可编辑保存（二进制拒绝 + dirty 跟踪）、markdown 原文↔渲染切换、图片动画、A4 默认尺寸 + 拖拽调大小持久化（min 380×480）、关闭彻底卸载

回合生命周期
- [ ] startSend 六步顺序（含 pendingPrompt 握手路径、provider 未注册落定）
- [ ] cancel：先清重试/权限，2.5s 超时强杀进程
- [ ] guideAt：先回应权限取消，800ms 超时杀进程再发 guide，pendingGuide 优先于队列
- [ ] discardIfEmpty：切走/新建时删空会话；启动清理
- [ ] 断线续写：尾 500 字合成 prompt 不写历史、continueRetries<2、startFailed 保留部分输出、（已中断）追加而非替换
- [ ] 多会话并发：进程上限 3 LRU 停空闲、后台未读标记、属性代理活动 runtime
- [ ] 复制 transcript：User:/Assistant: 行、跳过 pending、未打开会话直接解析 JSONL

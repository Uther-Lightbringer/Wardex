# 设计原则与红线

> 相关文档：[architecture.md](./architecture.md) · [implementation-plan.md](./implementation-plan.md) · [data-formats.md](./data-formats.md) · [acp-protocol.md](./acp-protocol.md) · [performance.md](./performance.md)（R1~R4 的量化预算，与本文同级）· [panels.md](./panels.md)

本文档是重写的"宪法"：实现任何功能前先看这里。原则分四类：性能红线、兼容性红线、代码约定、测试约定。

## 1. 性能红线（对应旧版审计 P1~P4，见旧仓库 `memory-audit-streaming.md`）

旧版的卡死/内存暴涨源于四条系统性设计错误，新版**在架构层面禁止重蹈**，不是"优化点"而是"红线"：

### R1 —— 流式渲染只做增量，禁止全量重建

- 文本/thinking 段：新 chunk 只 append 到该段末尾的 DOM 文本节点；**禁止**把整段已累积文本重新绑定/重新 `v-html`/重新创建组件。
- markdown 渲染只在回合结束时执行一次；流式期间纯文本显示。
- 列表 keyed 渲染：segment 有稳定 id，追加/延伸末段不得导致兄弟节点重建（`v-for :key` 用 segment id，不用数组身份）。
- 任何"每次刷新都重新解析整篇文档"的隐藏副本（旧版 copyEdit）禁止存在；复制功能在点击时才取全量文本。

### R2 —— 消息单一数据源

- Rust 侧 segments 是消息的唯一权威表示；**不维护** content/thinking 的冗余副本（旧版同一份回复存 2~3 份）。需要拼接全文时（复制、transcript 导出）按需临时拼接。
- 运行期续写 buffer 只保留尾部（≤2000 字符 + 续写用 500 字符），不留全文。
- 前端不持有消息权威副本，只持有渲染所需视图状态。

### R3 —— 有界驻留，必须淘汰

- 内存中打开会话的消息模型设上限，LRU 淘汰；淘汰后可从 messages.jsonl 按需重建。
- ACP 子进程上限 `kMaxParallelAcp = 3`，超限停最久未活动的 idle 进程（全 busy 允许临时超限）。
- 任何"打开过就永远留在内存"的结构一律禁止。

### R4 —— 工具 payload 有界

- 内存中工具 payload（rawInput/rawOutput/content）截断到 **64KB**，完整内容只落盘。
- `tool_call_update` 的 content 块是累积快照，只解析**最后一个块**；禁止每次更新拼接全部块（旧版 O(n²)）。
- 展开视图的美化打印基于截断后的内容。

## 2. 兼容性红线（出错 = 三类 CLI 之一不可用或用户数据损坏）

### C1 —— 磁盘格式逐字段兼容旧版

- `%AppData%/WarDex` 下所有 JSON/JSONL 的字段名、类型、缺省值、写时序（追加 vs 全量重写）以 [data-formats.md](./data-formats.md) 为准，包括其中标注的"历史遗留怪癖"（如追加行不写 segments 键、改名不刷 updatedAt、apiKey 掩码保护）。**不要"修正"旧行为**，除非文档明确标注为可变更。
- 时间戳字段按 f64 解析（Qt 历史写出可能是浮点形态）。

### C2 —— ACP 协议行为逐条对齐旧代码

- Windows 特例：`.cmd/.bat` shim 包 `cmd.exe /c`；env 覆盖中 null = 删除变量（clearEnvs 防嵌套）。
- `session/load` 回放期间的通知全部丢弃（本地历史为权威）；load 失败自动回退 `session/new`。
- prompt 错误路径顺序固定：protocolError → messageChunk("回合失败：…") → turnFinished("error")——限流检测依赖此顺序。
- 权限应答是**双层嵌套** `result.outcome.outcome`。
- 详见 [acp-protocol.md](./acp-protocol.md) 与 [providers-and-cli.md](./providers-and-cli.md)，实现时逐条对照旧代码行号。

### C3 —— provider 差异只收敛在一处

- kimi/claude/codex/custom 的差异（acpArgs、env、modeMap、clearEnvs、中转 key 特例）只允许出现在 provider registry 模块；协议层与 chat 层不写 `if provider == ...`。

## 3. 代码约定

- **代码与注释用英文**（与旧版一致）；面向用户的界面文本用**纯中文**。
- **设计说明写在代码注释里**，不维护代码外的设计文档（docs/ 是迁移期的例外，实现落定后允许其逐渐让位于代码注释）。
- Rust：每个模块一个目录/文件对（与 `src-tauri/src/{acp,chat,store}/` 骨架对齐）；协议结构体用 serde 定义，未知字段 `#[serde(flatten)]` 兜底以容忍 CLI 侧新增字段。
- Vue：页面在 `src/pages/`，自绘控件在 `src/components/war/` 并加 `War` 前缀；全局状态用 Pinia，禁止组件间直接跨页引用。
- 素材引用一律 `/assets/...` 绝对路径（public 静态），不进 Vite 打包管线。
- 新依赖（npm / cargo crate）必须先确认确有必要：优先标准库/已有依赖；添加时在 implementation-plan.md 登记。

## 4. 测试约定

- 旧版无自动化测试；新版对**协议层与存储层**必须配 Rust 单元测试：
  - acp：NDJSON 帧切分、反向 RPC 应答结构（双层 outcome）、env 组装（覆盖/删除/.cmd 包装）、tool 归一化
  - store：meta.json/messages.jsonl 往返读写（含旧版遗留格式样本：无 segments 键的追加行、浮点时间戳、含 `…` 前缀的 content）、apiKey 掩码保护、projects 去重
- 测试样本数据放 `src-tauri/tests/fixtures/`（可从旧版数据目录脱敏复制）。
- UI 层不做自动化 E2E，手动验证（验收标准见 [implementation-plan.md](./implementation-plan.md)）。

## 5. 明确不做（范围红线）

- 不支持 Windows 以外的平台（代码中有意不做跨平台抽象）。
- 不做视频背景（旧版已移除）；model 背景用 Three.js 且可后置。
- 不引入 Qt/旧仓库的任何代码；旧仓库全程只读。
- 第一版不做：多语言、主题切换、插件系统、云端同步。

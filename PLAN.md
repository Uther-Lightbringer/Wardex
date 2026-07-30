# WarDex → Tauri 重写计划

## 决策（已与用户确认）

- **目标栈**：Tauri 2（Rust 后端）+ Vue 3 + TypeScript + Vite（前端）
- **动机澄清**：当前卡顿/内存暴涨的根源是审计报告 `memory-audit-streaming.md` 里的 P1~P4（O(N²) 全量重建、2~3 倍重复存储、会话无淘汰），不是 C++/Qt 本身。重写时**按新架构规避这些设计**，而不是照搬旧逻辑。
- **重写方式**：新建独立目录 `C:/workspace/Wardex-rust`（新 git 仓库），现有 C++ 项目原样保留作参照，不做任何修改。
- **数据兼容**：新版直接读写现有的 `%AppData%/WarDex/` 数据（sessions/agents/projects/user_prefs 全是 JSON/JSONL，格式已在旧代码中定型），用户历史会话和 Agent 配置零迁移。开发期用 `WarDex-tauri-dev` 数据目录隔离。

## 新架构总览

```
WarDex-tauri/
├── src-tauri/           # Rust 后端（Tauri 命令 + 事件）
│   ├── src/acp/         # ACP 协议客户端（JSON-RPC over stdio，NDJSON）
│   ├── src/chat/        # 会话 runtime 管理、流式合并、重试
│   ├── src/store/       # sessions / agents / projects / prefs 持久化
│   └── src/probe.rs     # CLI 探测
├── src/                 # Vue 3 前端
│   ├── pages/           # MainMenu / Config / SessionSelect / Chat / Todo
│   ├── components/war/  # WC3 自绘控件库（border-image）
│   └── stores/          # Pinia 状态
└── assets/              # 从旧仓库拷贝的 PNG/WAV（直接复用）
```

**通信模型**：前端 `invoke()` 调 Rust 命令；Rust 用 `emit()` 推流式事件（`acp://chunk`、`acp://tool`、`acp://permission` 等）。前端只维护渲染状态，消息权威数据在 Rust 侧。

**内存设计原则（对应旧 P1~P4）**：
1. 流式渲染走**增量 DOM 追加**（`text` 段只 append 新 chunk 到末尾文本节点），绝不整段重绑定；markdown 在回合结束后才渲染一次。
2. 消息内容单一数据源：Rust 侧 segments 为权威，`content/thinking` 不再冗余存储（加载时按需拼接）；工具 payload 截断到 64KB（完整内容只落盘）。
3. 会话模型 LRU 淘汰：内存中最多保留 N 个打开会话的消息模型，超出即卸载（可按需从 JSONL 重读）；ACP 进程维持上限 3。
4. `tool_call_update` 只解析最后一个累积块，不拼接全部。

## 阶段划分

### 阶段 0 — 脚手架（半天）
- `npm create tauri-app` 初始化 Vue3+TS 模板到 `C:/workspace/Wardex-rust`，`git init`
- 窗口配置 1280×720（最小 960×600），标题"WarDex"，中文界面基调
- 拷贝旧仓库运行时资源：`assets/ui/{frames,buttons,dropdown,scroll,avatars,misc}`、`assets/Sound/*.wav`、`assets/background/LodolonFall.jpg`、wc3_extracted 的 6 个 qrc 图标
- 验证：`npm run tauri dev` 空窗口能起

### 阶段 1 — Rust 后端核心（最大头）
对照旧代码逐模块重写（行号为旧代码参照）：

1. **acp 模块**（参照 `src/AcpClient.cpp`）：
   - tokio spawn 子进程，stdin/stdout NDJSON JSON-RPC；stderr 打日志
   - 方法：`initialize` / `session/new` / `session/load` / `session/prompt`（text+image block）/ `session/cancel` / `session/set_config_option`
   - 反向 RPC：`session/request_permission`（转发前端等待回答）、`fs/read_text_file`（支持 line/limit）、`fs/write_text_file`
   - **Windows 关键细节必须保留**：`.cmd/.bat` shim 包 `cmd.exe /c`、env 覆盖与 null 删除、`session/load` 回放丢弃（m_replaying）、prompt 错误路径顺序（protocolError→messageChunk→turnFinished）
2. **provider registry**（参照 `ProviderRegistry.cpp`）：kimi/claude/codex/custom 四类差异（acpArgs、apiKeyEnvs、clearEnvs 防嵌套、modeMap、中转 key 的 ANTHROPIC_AUTH_TOKEN 特例）
3. **chat runtime**（参照 `ChatController.cpp`）：50ms 合并 flush（积压 >64KB 升 250ms）、队列上限 10、`kMaxParallelAcp=3` 进程淘汰、断线续写（尾部 500 字符合成续写 prompt）、限流重试（429 检测，20/40/80s backoff）、subagent 跟踪
4. **store 模块**（参照 `SessionStore.cpp`/`AgentStore.cpp`/`ProjectStore.cpp`）：JSON/JSONL 格式与旧版**逐字段兼容**；messages.jsonl 追加写 + 回合结束重写；全文搜索（tokio 任务 + 代际作废）
5. **cli probe**（参照 `CliProbe.cpp`）：候选路径队列、`--version` 4s 超时、按 provider 缓存
6. 每个模块配 Rust 单元测试（旧项目无测试，新项目对协议层和持久化层补上）

### 阶段 2 — WC3 UI 组件库 + 页面骨架
1. **war 控件库**（CSS `border-image` 1:1 复用贴图，九宫格参数从 QML 属性里搬运）：
   - `WarFrame`（FrameImage 等价，hole*/inset 像素内嵌参数一起带走）、`WarButton`（三态贴图 + 音效）、`WarDialog`（border 56 + 标题板/按钮区贴图分数定位）、`WarDropdown`、`WarMenu`、`WarScrollBar`、链条平铺 `repeat-y`、蓝色高亮 `mix-blend-mode: plus-lighter`
2. 全局：自定义光标（`cursor: url()`）、3 个音效（click/popUp/popDown + 音画时序逻辑）、image 背景（model 背景用 Three.js GLTFLoader，**放最后做，可后置**）
3. 页面骨架 + 路由：主菜单（钢板+最近项目）、Config、SessionSelect、Chat、Todo；页面切换下落动画

### 阶段 3 — 聊天页完整功能
- 消息列表：segments 交错渲染（thinking 折叠/text/tool），**增量 append 实现**；用户右/agent 左气泡 + 头像
- 输入区：@文件引用（`:起-止` 行号）、Ctrl+V 贴图、附件条 ≤6、64K 字上限、prompt 模板、permission mode 下拉
- 权限确认对话框（四态中文化 + 多选项网格）、发送队列面板、SubagentPanel、限流重试横幅、回到底部
- 右栏：agent 切换、git 分支+提交历史（Rust 侧读 `.git` 和跑 `git log`）、文件树预览
- 文件预览弹窗（文本/markdown/图片）
- SessionSelect 页：项目分组、行内重命名、标题+全文搜索（防抖）

### 阶段 4 — 收尾与打包
- Todo 页、user_prefs（字号缩放等）、内存看门狗（可选）
- `background.json` 外部配置约定保留（exe 旁读取）
- `npm run tauri build` 出 NSIS 安装器 + 便携 exe；打 zip（对应旧 package.bat 形态，体积小一个数量级）
- **验收**：三类 CLI（kimi acp / claude-code-acp / codex-acp）各跑通：新会话、流式回复、工具调用、权限确认、图片发送、断线恢复；长回复不再卡顿（对照旧版同 prompt 对比）；旧数据目录的会话能打开

## 明确不做
- 不修改现有 C++ 仓库任何文件
- 不做视频背景（旧版已移除）、不做 Quick3D 动态插件机制（Three.js 静态引入即可，且后置）
- 不迁移 `tools/` Python 脚本、`mockup-open-project.html` 等开发残留
- 第一阶段不做自动化 E2E 测试（保持手动验证，只对 Rust 协议/存储层加单元测试）

## 风险
- **工作量**：旧后端 ~8800 行 C++ + ~8600 行 QML，重写是大工程，按阶段增量交付，阶段 1+3 是关键路径
- **ACP 行为细节丢失**：上面列的 Windows 特例清单是兼容性的命门，实现时逐条对照旧代码
- **Tauri 前端流式性能**：高频 `emit` 事件本身有 IPC 开销，50ms 合并节奏保留；若成为瓶颈再改 channel/批量

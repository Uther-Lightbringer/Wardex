# 分阶段实施计划与验收

> 相关文档：[README.md](./README.md)（阅读顺序）· [architecture.md](./architecture.md) · [design-principles.md](./design-principles.md)（红线）· 各专题文档

本文档把重写拆成 5 个阶段，每阶段列出任务、依赖文档、完成判据。**严格按顺序执行**：阶段 1（后端）和阶段 3（聊天页）是关键路径。执行前必读 [design-principles.md](./design-principles.md) 的红线。

## 环境前提（阶段 0 前）

- Node ≥ 20、Rust stable（`x86_64-pc-windows-msvc`）、VS 2022 C++ 工作负载（MSVC 链接器）、WebView2（Win10/11 通常内置）
- 首次 `npm install` + `npm run tauri dev`（首编译 5~15 分钟）

## 阶段 0 —— 脚手架与素材 ✅（已完成）

- [x] Tauri 2 + Vue3 + TS + Vite + Pinia 项目骨架，`git init`
- [x] 窗口 1280×720（最小 960×600），中文基调
- [x] 53 个素材拷贝至 `public/assets/`（与旧版 qrc 清单一一对应，见 [assets.md](./assets.md)）
- [x] 后端模块骨架 `src-tauri/src/{acp,chat,store}/`（头部注释含迁移要点）
- 判据：`npm run tauri dev` 起空窗口

## 阶段 1 —— Rust 后端核心（最大头）

依赖文档：[acp-protocol.md](./acp-protocol.md) · [providers-and-cli.md](./providers-and-cli.md) · [data-formats.md](./data-formats.md) · [architecture.md](./architecture.md) §4

任务（建议顺序）：

1. **store 模块**：sessions（meta.json/messages.jsonl 读写、遗留格式兼容、LRU 驻留）、agents、projects、user_prefs、todos、prompts、media 缓存清理。配 serde 模型 + 单元测试（含旧版遗留样本）。
2. **provider registry + probe**：四类 ProviderSpec、env/args 组装、CliProbe 候选队列与 4s 超时语义。
3. **acp 模块**：tokio 子进程、NDJSON 帧、全部方法与反向 RPC、Windows 特例（.cmd 包装、env 删除）、tool 归一化。配单元测试（mock 子进程用测试桩 CLI）。
4. **chat 模块**：Runtime 管理、50ms/250ms 合并 flush、队列上限 10、进程上限 3、限流重试（20/40/80s）、断线续写、subagent 跟踪、switchAgent、guideAt/cancel 超时。
5. **Tauri 命令与事件接线**：lib.rs 注册全部 `invoke` 命令与事件转发；日志（logs/ 分阶段计时）+ panic hook（crashes/）。
6. **数据目录**：dev 用 `WarDex-tauri-dev`、release 复用 `WarDex`（判定方式参照旧版 exe 名规则，可用 `cfg!(debug_assertions)` 替代）。

完成判据：

- Rust 单元测试全绿（`cargo test`）
- 命令行级冒烟：能 spawn kimi acp 完成 initialize → session/new → prompt → 收到流式 chunk → 落盘 JSONL 与旧版格式一致（可用旧版 WarDex 打开同目录交叉验证）
- 能读取旧版数据目录的真实会话/Agent 配置（只读模式指向 `%AppData%/WarDex` 验证）

## 阶段 2 —— WC3 控件库 + 页面骨架

依赖文档：[ui-design.md](./ui-design.md) · [assets.md](./assets.md) · [features/main-menu-and-misc.md](./features/main-menu-and-misc.md)

任务：

1. **war 控件库**（`src/components/war/`）：WarFrame（border-image 九宫格 + hole/inset 开孔）、WarButton（三态+音效+uiGate）、WarDialog（标题板/按钮区贴图分数定位）、WarDropdown、WarMenu、WarScrollBar（九宫格 thumb，不可滚时隐藏轨道）、吊链平铺、蓝色高亮 mix-blend。每个控件的 border 值严格从 ui-design.md 抄。
2. **面板坞框架**（`WarDock`/`WarPanel` + `src/panels/registry.ts`）：手风琴堆叠、拖拽调高（min 80px/max 60%）、`panelLayout` 布局记忆、懒挂载——规范见 [panels.md](./panels.md)。
3. **全局**：自定义光标、三事件音效（200ms 节流+音画时序）、image 背景 + background.json 外部配置、铁轨边框布局、450ms 三段下落切页动画（已提速，原 750ms）、fontScale。
4. **五页骨架 + 导航**：main（钢板+最近项目）、config、sessionSelect、chat、todo 的占位→静态版；uiGate 切页禁输入；打开项目对话框。

完成判据：五页可导航、视觉与旧版并排对比一致（同屏截图对比）、音效/光标/动画时序正确。

## 阶段 3 —— 聊天页完整功能（关键路径）

依赖文档：[features/chat.md](./features/chat.md)（最细规格）· [features/sessions-and-config.md](./features/sessions-and-config.md) · 红线 R1~R4

任务：

1. 消息列表：segments 交错渲染（thinking 折叠/text/tool）、**增量 DOM append**、markdown 回合结束渲染一次、气泡/头像/复制按钮（惰性取全文）、segOpen 保持
2. 输入区：64K 上限、@引用（行号区间）、Ctrl+V 贴图、附件条 ≤6、prompt 模板、permission mode 下拉
3. 权限对话框（双层 outcome 应答）、发送队列面板、SubagentPanel、限流重试横幅、回到底部
4. 左栏会话 rail（右键菜单）、右栏面板坞首批面板（`agent`/`git`/`files`，按 [panels.md](./panels.md) 注册表机制实现，含 `inspect/git.rs`、`inspect/files.rs` 后端命令）
5. 文件预览弹窗（文本编辑保存/markdown 切换/图片/>2MB 询问/尺寸持久化）
6. 会话选择页：项目分组、行内重命名、标题+全文搜索（防抖+代际作废）
7. Agent 配置页：编辑器全字段、CLI 探测状态、testAgent、apiKey 掩码
8. 待办页

完成判据：见 §验收。

## 阶段 4 —— 收尾与打包

- background.json 发布包附带 example；`npm run tauri build` 出 NSIS 安装器 + 便携 zip（对应旧版 package.bat 形态）
- （可后置）model 背景 Three.js、--perf HUD、内存压力横幅
- 文档收尾：实现与文档的差异回填；各文档检查清单打勾

## 验收标准（全部通过才算完成）

1. **三类 CLI 各跑通**（kimi acp / claude-code-acp / codex-acp）：新会话 → 流式回复 → 工具调用展示 → 权限确认 → 图片发送 → 取消 → 会话重开（session/load 恢复）
2. **性能对比**：旧版会卡死的场景（长 thinking 回复、含大文件内容的工具调用）在新版流畅；内存随会话数有界（LRU 生效）
3. **数据兼容**：旧版数据目录的会话/Agent/项目/偏好在新版原样呈现；新版写的数据旧版可读
4. **视觉一致**：主菜单/聊天页与旧版并排截图对比无明显差异
5. `cargo test` 全绿

## 风险与注意

- **工作量**：旧后端 ~8800 行 C++ + ~8600 行 QML 的等价重写，按阶段增量交付，不要在阶段 1 追求完美抽象。
- **ACP 细节丢失**是最大风险：acp-protocol.md 与 providers-and-cli.md 中每条 `文件:行号` 参照都要对照旧代码核实后再写。
- **Tauri IPC 开销**：高频 emit 有成本，50ms 合并节奏保留；若成为瓶颈改批量事件（单事件携带多 chunk）。
- **依赖登记**：新增 npm/cargo 依赖在此处登记（当前基线：tauri、serde/serde_json、tokio、anyhow、thiserror、uuid、chrono、base64、vue、pinia、@tauri-apps/api）。
  - 阶段 1a（store）：cargo 新增 `dirs`（%AppData% 定位）、`encoding_rs`（readFileRange/previewFile 的 GBK 回退解码）、`image`（仅 png/jpeg codec：剪贴板图片降级链与 128px 头像裁剪）、dev-only `tempfile`（单元测试隔离数据根）。
  - 阶段 1b（provider/probe）：cargo 新增 `winreg`（CliProbe 合并 HKCU\Environment 用户 PATH）、`shell-words`（agent extraArgs 的 QProcess::splitCommand 等价拆分）。
  - 阶段 1c（acp）：cargo 新增 `log`（协议层日志门面：坏 JSON 丢弃、stderr 透传、session/load 回退；具体 logger 实现随阶段 1.5 日志基建接入）。
  - 阶段 1d（chat + 接线）：cargo 新增 `fern`（log 门面的具体 logger：写 logs/wardex-<date>.log + stderr，启动分阶段计时；panic hook 写 crashes/crash-*.txt，无新依赖）。无其他新增；chat 层集成测试复用 `MockTransport` 与 dev-only `tempfile`。
  - 阶段 3a（聊天页）：npm 新增 `markdown-it` + dev `@types/markdown-it`；cargo/npm 新增 `tauri-plugin-dialog` / `@tauri-apps/plugin-dialog`（附件与头像的系统文件对话框）。
  - 面板扩展（git diff / AskUserQuestion 分组）：**无新增依赖**。git diff 沿用 git CLI 子进程（与 `git_log` 同模式，4s 超时），未引入 git2。

## 实施进度快照（2026-07-29）

- [x] 阶段 0：脚手架 + 素材（dev 启动验证通过）
- [x] 阶段 1：后端四子任务完成，135+ 测试全绿、clippy 零警告；kimi acp 握手冒烟通过（`cargo test --test acp_smoke -- --ignored`，真实 sessionId）
- [x] 阶段 2：war 控件库 9 组件 + 面板坞 + 五页骨架；dev 窗口启动、后端初始化日志正常
- [x] 阶段 3：聊天页（R1 增量 DOM append + 虚拟滚动）+ 会话选择/配置/待办页完整功能；`vue-tsc` 0 错误
- [x] 阶段 4：background.json exe 旁 Rust 解析（`background_config` 命令，旧 main.cpp 规则）；release 构建产出 `wardex.exe`（19.6MB）+ NSIS 安装器 + 便携 zip `WarDex-win64-20260730.zip`（10.6MB）；exe 启动验证通过（日志干净、空闲内存 ~66MB）；kimi acp 握手冒烟通过
- 未闭合偏差（后续项）：payload 截断后 JSONL 无全文（1d 遗留）、`git_log` 4s 超时无法 kill 子进程、队列预览仅本进程镜像、内存压力横幅/--perf HUD/Three.js model 背景后置
- 行为变更（有意）：`provider_supports_chat` 收敛到注册表 chatCapable，四类 provider 均可设为默认（旧版与 1a 桩仅限 kimi）
  - 阶段 3a（聊天页）：npm 新增 `markdown-it` + dev `@types/markdown-it`（气泡/预览的 markdown，回合结束后渲染一次，R1）；npm `@tauri-apps/plugin-dialog` + cargo `tauri-plugin-dialog`（输入区 📎 附件系统文件对话框，capabilities 加 `dialog:default`）。tauri.conf.json 启用 `assetProtocol`（scope `**`：气泡附件缩略图/自定义头像/预览图片的本地文件加载）。无其他新增。

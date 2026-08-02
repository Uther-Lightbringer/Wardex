# WarDex

WarDex 是一款 Windows 桌面端 AI 编程助手 GUI，采用魔兽争霸 III 风格自绘界面。它通过 [ACP](https://github.com/zed-industries/agent-client-protocol)（Agent Client Protocol，stdio JSON-RPC）连接 `kimi acp` / `claude-code-acp` / `codex-acp` / `opencode acp` 等 Agent CLI，提供多会话并行聊天、权限确认、图片发送、项目与会话管理、用量统计等功能。

## 功能特性

- **多会话并行**：每个会话独立 ACP 进程（软上限 3），互不阻塞
- **流式聊天**：增量渲染 + markdown、工具调用、子 Agent（subagent）跟踪
- **权限确认**：CLI 侧权限请求弹出窗口，支持临时放行/永久放行
- **会话与项目**：按项目分组、全文搜索、断线续写、会话用量统计
- **信息面板**：Git 历史、文件树、后台任务、提醒、待办
- **WC3 风格自绘 UI**：九宫格边框、三态按钮、铁轨装饰（自制素材）
- **多种 Agent 支持**：kimi / claude / codex / opencode，支持自定义 provider

## 下载

最新正式版（Windows x64）：

- [NSIS 安装器](https://github.com/Uther-Lightbringer/Wardex/releases/latest/download/WarDex_0.0.1_x64-setup.exe)
- [便携版 ZIP](https://github.com/Uther-Lightbringer/Wardex/releases/latest/download/WarDex-win64-0.0.1.zip)（解压即可运行，附 `make-desktop-shortcut.bat` 创建桌面快捷方式）

## 技术栈

| 层 | 技术 |
|---|---|
| 桌面框架 | Tauri 2 |
| 后端 | Rust（tokio、serde、ACP 协议客户端） |
| 前端 | Vue 3 + TypeScript + Vite + Pinia |
| 编辑器 | CodeMirror 6（文件预览） |

## 快速开始

环境要求：[Node.js](https://nodejs.org) ≥ 18、[Rust](https://rustup.rs) stable、[Tauri CLI 依赖](https://tauri.app/start/prerequisites/)。

```bash
npm install
npm run tauri dev        # 开发模式（Vite 热更新）
npm run tauri build      # 打包 release + NSIS 安装器
```

首次使用需在设置页配置至少一个 Agent（及对应 CLI），例如：

```bash
npm i -g @zed-industries/claude-code-acp   # claude
npm i -g opencode-ai                        # opencode
```

## 配置

- 背景图/视频：复制 `background.example.json` 为 `background.json`（放在 `wardex.exe` 同目录）后修改。
- 数据目录：开发版 `%AppData%/WarDex-tauri-dev`，发布版 `%AppData%/WarDex`。

## 素材版权声明

- `public/assets/ui/`（frames/buttons/dropdown/scroll/avatars/misc）为项目自制切图，由 `tools/` 下 Python 脚本生成。
- `public/assets/wc3_extracted/`、`public/assets/Sound/`、`public/assets/background/` 中的部分素材来自《魔兽争霸 III》提取或不明来源，**仅供个人使用，不得随本项目重新分发**。开源再分发前请先替换为自有/许可素材。

## 文档

- [docs/README.md](./docs/README.md)：架构与设计文档入口（ACP 协议、数据格式、UI 设计等）

## License

[MIT](./LICENSE)

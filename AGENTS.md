# AGENTS.md

## 版本号约定

发版（改版本号）时，以下三处必须同步修改为同一个版本号：

- `package.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`

**不要手改**：`build-release.bat` / `build-portable.bat` 每次打包前会自动跑 `scripts/bump-version.mjs`，把三处版本号 patch +1 同步；想显式指定版本（如升 minor），先跑 `node scripts/bump-version.mjs 0.1.0` 再打包。脚本只动三处 `version` 字段，`Cargo.lock` 由 cargo build 自行跟进。

界面左下角版本文字（`src/App.vue`）通过 Tauri `getVersion()` 读取，**不要硬编码**；发版后界面自动跟随，无需改动。

`src-tauri/src/acp/types.rs` 的 `CLIENT_VERSION` 是 **ACP 协议版本**，与产品版本无关，通常不要随发版改动（协议变更时才更新，且需同步 `probe.rs` testAgent 握手等多处写死的值）。

## 左右铁轨宽度

左右两侧铁轨装饰条宽度在 `src/App.vue` 的 `.rails img`（当前 58px）。修改时必须同步 `src/components/PageShell.vue` 的 `edgeW` 默认值（应相等），否则页面内容内缩会和铁轨错位。

## Pi（provider "pi"）

Pi 是 WarDex 内置的 coding agent 后端。不依赖 Node 端：用 **bun 编译成自包含二进制**（`pi.exe` + 资产目录），运行时不需要 Node。

### 源码：third_party/pi（git subtree 内置）

`third_party/pi` 是 Wardex-Pi fork（`github.com:Uther-Lightbringer/Wardex-Pi`）的 **git subtree** 内嵌副本，clone 本仓库即带 pi 源码。

- **改 pi 代码**：直接在 `third_party/pi` 里改，随本仓库一起提交；回写 fork 用 `git subtree push --prefix third_party/pi <Wardex-Pi-url> main`
- **拉 fork 新提交**：`git subtree pull --prefix third_party/pi <Wardex-Pi-url> main --squash`
- **vendored 首次构建**（新 clone 或 dist 缺失时）：`cd third_party/pi && npm install --ignore-scripts && npm run build:offline`。模型数据 `packages/ai/src/providers/data/`（含 `.manifest.json`）上游 gitignore 但**本仓库已提交**，clone 后即可纯离线构建；若需重新生成跑 `npm run hydrate:model-data`（联网 fetch models.dev）并把该目录变更一并提交
- **改 pi 内部代码时遵循 `third_party/pi/AGENTS.md`**（提交前 `npm run check`、只 stage 自己改的文件等）

### 打包链路

- **`npm run bundle:pi`**（scripts/bundle-pi.mjs）：定位 pi 源码 → 编译/复用 pi.exe → `copy-binary-assets` → 收敛到 `pi-runtime/packages/coding-agent/dist/`（gitignored）
- **源码定位优先级**：命令行参数 → `WARDEX_PI_DIR` → **`third_party/pi`（内置）** → workspace 同级 `pi/`（旧目录，仅兜底）
- **进安装包**：`npm run build:with-pi`（先 bundle-pi 再 tauri build）。`tauri.conf.json` `bundle.resources` 把 `pi-runtime/.../dist` 映射到 `resources/pi/packages/coding-agent/dist`，`pi-extensions/` → `resources/pi-extensions`，`pi-packages/` → `resources/pi-packages`
- **增量 vs 全量**：增量只按 `coding-agent/src` mtime 决定是否重编 pi.exe，**改了其他子包（ai/tui/agent/protocol/client 等）须先在该包 `npm run build`** 或设 `WARDEX_PI_REBUILD=1` 走离线 `build:offline` 全量重编（`build-release.bat` 默认开此开关，正式发版必带最新 pi）。联网 `build:binary` 会卡在 `generate-models`（fetch models.dev 超时），不要用它
- **不带 pi**：`WARDEX_BUNDLE_PI=0` 跳过，安装包不含 pi

### 运行时定位

`chat/pi.rs` `locate_plugin_dir`：Agent 配置的「Pi 插件目录」→ `WARDEX_PI_DIR` → 打包内置 `resources/pi` → workspace 同级 `pi/` → `third_party/pi`。**注意 dev 模式（tauri dev）同级旧目录优先于 vendored 副本**，改了 `third_party/pi` 但本机还留着 `C:\workspace\pi\...\dist\pi.exe` 时 dev 跑的是旧版——确认 vendored 链路没问题后删掉旧目录（或至少删它的 dist）。

### 自带扩展与包

- **`pi-extensions/*.ts`**（WarDex 仓库）：spawn Pi 时 `--extension` 注入（提醒 / codegraph 工具）。定位：`WARDEX_PI_EXTENSIONS_DIR` → `resources/pi-extensions` → 仓库根 `pi-extensions/`。spawn 注入 `WARDEX_SESSION_ID` / `WARDEX_TODOS_PATH` / `WARDEX_PROJECT_DIR`
- **`pi-packages/<pkg>/`**（如 pi-multiagent：agent_team 编排工具 + skill）：spawn 时挂 `<pkg>/extensions/*/index.ts` 为 `--extension`、skill 目录一并加载。**自带包优先于 pi 全局安装**：spawn 会把 `~/.pi/agent/settings.json` `packages` 里同名 `npm:` 条目摘除（`chat/pi.rs` `delist_bundled_packages_from_pi_settings`），否则两份扩展工具/flag 重名冲突，pi 直接 exit(1)（前端表现为「已中断」）

## 打包脚本

- **正式发版**：`build-release.bat`（bump 版本 + `WARDEX_PI_REBUILD=1` 全量编 pi + tauri build + NSIS 安装器）
- **绿色便携版**：`build-portable.bat`。bump 版本 → `npx tauri build --no-bundle` 编当前源码（**必须走 Tauri CLI**：裸 `cargo build --release` 仍连 `devUrl` localhost，界面黑屏）→ exe + `background.example.json` 拷进 `dist-portable/` → pi dist 镜像同步到 `dist-portable/resources/pi/packages/coding-agent/dist` → 拷 `pi-extensions`、`pi-packages` 到 `dist-portable/resources/` → 打 `WarDex-win64-<版本>.zip`。不动 NSIS 安装器
  - `WARDEX_PORTABLE_SKIP_BUMP=1` 跳过升版；`WARDEX_PORTABLE_SKIP_BUILD=1` 跳过编译用已有 release exe
  - **该 bat 必须保持纯 ASCII + CRLF**（cmd 按 OEM 代码页解析 .bat，UTF-8 中文注释会吞字符导致解析错乱，注释用英文写）

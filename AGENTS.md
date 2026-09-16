# AGENTS.md

## 版本号约定

发版（改版本号）时，以下三处必须同步修改为同一个版本号：

- `package.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`

**不要手改**：`build-release.bat` 每次打包前会自动跑 `scripts/bump-version.mjs`，把三处版本号 patch +1 同步；想显式指定版本（如升 minor），先跑 `node scripts/bump-version.mjs 0.1.0` 再打包。脚本只动三处 `version` 字段，`Cargo.lock` 由 cargo build 自行跟进。

界面左下角版本文字（`src/App.vue`）通过 Tauri `getVersion()` 读取，**不要硬编码**；发版后界面自动跟随，无需改动。

`src-tauri/src/acp/types.rs` 的 `CLIENT_VERSION` 是 **ACP 协议版本**，与产品版本无关，通常不要随发版改动（协议变更时才更新，且需同步 `probe.rs` testAgent 握手等多处写死的值）。

## 左右铁轨宽度

左右两侧铁轨装饰条宽度在 `src/App.vue` 的 `.rails img`（当前 58px）。修改时必须同步 `src/components/PageShell.vue` 的 `edgeW` 默认值（应相等），否则页面内容内缩会和铁轨错位。

## Pi 插件（provider "pi"）构建与打包

Pi 不依赖 Node 端：用 **bun 编译成自包含二进制**（`bundle-pi.mjs`），产物是「`pi.exe` + 资产目录」，运行时不需要 Node。

- **打自包含运行时**：`npm run bundle:pi`（node scripts/bundle-pi.mjs）。在 workspace 同级 `pi/`（或 `WARDEX_PI_DIR` / 参数指定）里跑 `bun build --compile` + `copy-binary-assets`，收敛到 `pi-runtime/packages/coding-agent/dist/`。
- **打包进安装包**：`npm run build:with-pi`（先 bundle-pi 再 tauri build）。`tauri.conf.json` 的 `bundle.resources` 把 `pi-runtime/packages/coding-agent/dist` 映射到安装包 `resources/pi/packages/coding-agent/dist`，并把 `pi-extensions/` 映射到 `resources/pi-extensions`（spawn Pi 时 `--extension` 注入提醒 / codegraph 工具）。
- **Pi extensions**：`pi-extensions/*.ts`（WarDex 仓库，不是 Pi 核心）。运行时定位（`chat/pi.rs` `locate_extensions_dir`）：`WARDEX_PI_EXTENSIONS_DIR` → 打包内置 `resources/pi-extensions` → 仓库根 `pi-extensions/`。spawn 注入 `WARDEX_SESSION_ID` / `WARDEX_TODOS_PATH` / `WARDEX_PROJECT_DIR`。
- **Bundled pi-packages**：`pi-packages/<pkg>/`（如 pi-multiagent：agent_team 编排工具 + skill）。打包进 `resources/pi-packages`，spawn 时把 `<pkg>/extensions/*/index.ts` 作为 `--extension` 注入、skill 目录一并挂上。**自带包优先于 pi 全局安装**：spawn 时会把 `~/.pi/agent/settings.json` `packages` 里同名的 `npm:` 条目摘除（`chat/pi.rs` `delist_bundled_packages_from_pi_settings`），否则 pi 启动时两份扩展工具/flag 重名冲突，直接 exit(1)（前端表现为「已中断」）。
- **全量重编 pi**：`WARDEX_PI_REBUILD=1` 时 `bundle-pi.mjs` 走 pi 根目录的**离线** `build:offline`（用仓库内固化模型数据，不联网 fetch models.dev；联网超时会把 `build:binary` 卡死在 `generate-models`），再 bun 编译自包含二进制 + `copy-binary-assets`。`build-release.bat` 默认设此开关（正式发版必带最新 pi）。
- **不带 pi**：`WARDEX_BUNDLE_PI=0` 跳过脚本，安装包不含 pi。
- 运行时定位（`chat/pi.rs` `locate_plugin_dir`）优先级：Agent 配置的「Pi 插件目录」→ `WARDEX_PI_DIR` → 打包内置 `resources/pi` → workspace 同级 `pi/` → `third_party/pi`。找的是 `dist/pi(.exe)` 二进制。
- 二进制由 pi 的 `build:binary` 生成；增量打包时改动 pi 代码后需重跑 `npm run bundle:pi` 才会带上新版（`WARDEX_PI_REBUILD=1` 则每次全量重编，无需手动重跑）。
- 增量模式下依赖子包（ai/tui/agent/protocol/client）需先构建好 dist，脚本不负责 rebuild 它们（全量重编模式除外）。
- **绿色便携版**：`build-portable.bat`。每次打包先跑 `scripts/bump-version.mjs`（三处 version patch +1，与正式发版同一套），再 `npx tauri build --no-bundle` 编当前源码（必须走 Tauri CLI：裸 `cargo build --release` 仍会连 `devUrl` localhost，安装包黑屏）。把 exe + `background.example.json` 拷进 `dist-portable/`，并把 pi dist 镜像同步到 `dist-portable/resources/pi/packages/coding-agent/dist`（`locate_plugin_dir` 会查 `<exe目录>/resources/pi`，无需改代码），再拷 `pi-extensions`、`pi-packages` 到 `dist-portable/resources/`，最后打 `WarDex-win64-<版本>.zip`。不动 NSIS 安装器。`WARDEX_PORTABLE_SKIP_BUMP=1` 跳过升版；`WARDEX_PORTABLE_SKIP_BUILD=1` 跳过编译用已有 release exe。**该 bat 必须保持纯 ASCII + CRLF**（cmd 按 OEM 代码页解析 .bat，UTF-8 中文注释会吞字符导致解析错乱，注释用英文写）。

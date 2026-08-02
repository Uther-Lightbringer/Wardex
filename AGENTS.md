# AGENTS.md

## 版本号约定

发版（改版本号）时，以下三处必须同步修改为同一个版本号：

- `package.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`

界面左下角版本文字（`src/App.vue`）通过 Tauri `getVersion()` 读取，**不要硬编码**；发版后界面自动跟随，无需改动。

`src-tauri/src/acp/types.rs` 的 `CLIENT_VERSION` 是 **ACP 协议版本**，与产品版本无关，通常不要随发版改动（协议变更时才更新，且需同步 `probe.rs` testAgent 握手等多处写死的值）。

## 左右铁轨宽度

左右两侧铁轨装饰条宽度在 `src/App.vue` 的 `.rails img`（当前 58px）。修改时必须同步 `src/components/PageShell.vue` 的 `edgeW` 默认值（应相等），否则页面内容内缩会和铁轨错位。

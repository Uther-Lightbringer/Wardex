# WarDex 插件（实验分支 feature/plugin-architecture）

一切皆插件：工具类（pi extension）与界面类（侧边栏面板）都从统一目录热管理。

## 目录布局

```
<数据根>/wardex-plugins/          ← 设置页「打开目录」直达
├── registry.json                 ← enabled 开关的唯一事实来源（自动维护）
└── <插件id>/
    ├── plugin.json               ← { name, version, entry?, ui? }
    ├── main.ts                   ← 工具类入口（pi extension 格式）
    └── panel.html                ← 界面类面板
```

内置插件：会话提醒 / Codegraph / 插件管理员（不可删除；管理员不可停用）。

## 使用方式

1. **手动**：在 `wardex-plugins/` 下新建目录写好文件 → 设置 → 插件 → 扫描 → 勾选 → 生效。
2. **对话**：直接对 AI 说「帮我加一个 XXX 的侧边栏面板/工具」，它会调用
   `plugin_write` 等工具写好文件，你点「生效」即可。
3. 生效 = 重启空闲会话的 pi 进程（会话上下文从 session-dir 恢复，不丢对话）；
   忙碌会话跳过，稍后再点。

## UI 面板桥 API（panel.html 内）

```js
window.postMessage({ source: 'wardex-plugin', type: 'ready' }, '*');
// 收: { source:'wardex-host', type:'info', payload:{ sessionId, projectDir } }
window.postMessage({ source:'wardex-plugin', type:'notify', text:'提示' }, '*');
window.postMessage({ source:'wardex-plugin', type:'sendPrompt', text:'...' }, '*');
```

iframe 以 sandbox 运行，只有上述白名单能力——这就是安全边界，扩白名单要慎重。

示例见 `examples/plugins/hello-panel/`，拷进数据根的 `wardex-plugins/` 即可试用。

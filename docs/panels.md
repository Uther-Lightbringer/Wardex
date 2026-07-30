# 信息面板坞（Panel Dock）与铁框视觉语言

> 相关文档：[architecture.md](./architecture.md) · [performance.md](./performance.md)（不可见不工作/面板数据预算）· [ui-design.md](./ui-design.md)（九宫格参数总表）· [assets.md](./assets.md) · [features/chat.md](./features/chat.md)

聊天页右栏是**可扩展的信息面板坞**：git 历史、文件树是首批面板，将来陆续加入 git 对比、数据库信息、HTTP 调试等开发信息面板。本文档定义：面板坞架构（新面板 = 一个文件 + 一行注册）、交互规范（手风琴 + 拖拽调高 + 布局记忆）、铁框三级视觉语言、新面板开发指南。

## 1. 面板坞架构

### 1.1 注册表（前端唯一扩展点）

```ts
// src/panels/registry.ts —— 加一个面板 = 加一个组件文件 + 在这里加一条
export interface PanelDef {
  id: string;                    // 'git' | 'files' | 'db' | ... 全局唯一
  title: string;                 // 中文标题："版本控制" / "工作区文件" / "数据库"
  icon?: string;                 // /assets/... 图标，可选
  component: () => Promise<Component>; // 动态 import，折叠时不加载代码
  defaultOpen: boolean;
  defaultHeight: number;         // px，首次使用且无记忆时的高度
  order: number;                 // 默认排序
  refreshOn: RefreshTrigger[];   // 数据刷新时机
}
type RefreshTrigger = 'turnEnd' | 'sessionSwitch' | 'expand' | 'manual';
```

### 1.2 容器行为（`src/components/war/WarDock.vue` + `WarPanel.vue`）

- **垂直堆叠手风琴**（WC3 风格）：面板自上而下排列，各自独立展开/折叠，折叠动画 200ms 滑收。
- **懒挂载**：折叠的面板不挂载组件、不取数据（[performance.md](./performance.md) §1.4 不可见不工作）；首次展开才动态 import + 拉数据。
- **拖拽调高**：每个展开面板下沿有拖拽手柄（铁框风格，见 §3），pointer 拖拽实时改高度；约束 min 80px / max 面板坞高度的 60%。拖拽过程中内容区 `pointer-events: none` 防误触。
- **布局记忆**：每个面板的 `{open, height, order}` 按面板 id 持久化到 `user_prefs.json` 的 `panelLayout` 字段：
  ```json
  "panelLayout": { "git": { "open": true, "height": 220 }, "files": { "open": false } }
  ```
  拖拽结束（pointerup）后 **300ms 防抖**写盘；下次打开同一面板恢复同样高度与展开状态。未登记的 id 使用 PanelDef 默认值。
- **排序**：order 持久化；拖拽换序为可后置项（v1 固定按 registry order）。

### 1.3 后端扩展点（`src-tauri/src/inspect/`）

- 每个信息域一个模块：`inspect/git.rs`（分支/log/diff）、`inspect/files.rs`（文件树）、将来 `inspect/db.rs` 等，各暴露自己的 Tauri 命令（如 `git_log`、`git_diff`、`db_tables`）。
- **不提前抽象统一 trait**：git diff 与数据库结果集形状差异太大，先按具体命令实现；等出现 3+ 个相似域再提炼公共模式。
- 数据预算：面板数据遵循 [performance.md](./performance.md) §3——折叠即弃、结果分页/限量（git log 默认 50 条、diff 按文件分片拉取、db 查询强制 LIMIT）。

## 2. 首批面板映射（现有右栏功能的重写）

| 面板 id | 标题 | 来源功能（旧版参照） | refreshOn |
|---|---|---|---|
| `agent` | 会话信息 | agent 切换器（ChatPage.qml 右栏） | sessionSwitch |
| `git` | 版本控制 | 分支徽标 + 只读提交历史 + 工作区更改列表 + diff 查看（`git_status`/`git_diff_file`/`git_diff_commit`，单 diff 64KB 截断） | turnEnd, sessionSwitch, expand, manual |
| `files` | 工作区文件 | 文件树（点击预览/右键系统打开） | sessionSwitch, expand, manual |

`agent` 面板固定置顶且不可折叠（或始终 open），其余按注册表机制。git 对比、数据库等为后续批次，按 §4 指南加入。

## 3. 铁框三级视觉语言

所有"框"统一为三级，**禁止发明第四种框样式**：

| 级别 | 贴图 | 用途 | border |
|---|---|---|---|
| L1 页面框 | `frame_chat_main.png` / `frame_panel*.png` | 页面级大区域（消息区、输入区） | 见 [ui-design.md](./ui-design.md) 参数总表 |
| **L2 面板框（铁框）** | `frame_iron_bar.png`（标题条）+ `frame_iron_panel.png`（内容框） | **所有信息面板的默认容器** | 抄旧 QML 实际值，登记入 ui-design.md |
| L3 内嵌框 | `frame_bar.png` / `frame_action.png` | 面板内部小分区（diff 文件条目、db 表单行） | 同上 |

`WarPanel` 统一外观规格（面板作者只写内容区，样式漂移锁死在这一处）：

- **标题条**：`frame_iron_bar.png` 九宫格，固定高 28px；左起 图标(可选) + 中文标题 + 弹性空间 + 折叠箭头（▶/▼，旋转动画 200ms）；hover 微亮反馈。
- **内容框**：`frame_iron_panel.png` 九宫格，内边距按贴图内嵌值；内容超高出 `WarScrollBar`（置黑禁用规则不变）。
- **拖拽手柄**：内容框下沿 6px 高热区，视觉为铁框底沿加粗/三道横纹 grip，hover 时 `cursor: row-resize`（自定义光标体系内的等价处理，见 ui-design.md §光标）。
- **面板间距**：垂直间隔 4px，露出版底背景。
- **字号**：标题与内容文本按 `fontScale` 缩放，装饰符号不缩放（与全局规则一致）。

## 4. 新面板开发指南（给执行模型的模板）

以"数据库信息"面板为例，完整步骤：

1. `src-tauri/src/inspect/db.rs`：实现命令（连接配置存 user_prefs 或项目级配置；查询强制 LIMIT；大结果分页）。在 lib.rs 注册命令。
2. `src/panels/DbPanel.vue`：只写内容区（表格/表单），外壳用 `<WarPanel>` 槽位，不碰框样式。
3. `src/panels/registry.ts` 加一条：`{ id: 'db', title: '数据库', component: () => import('./DbPanel.vue'), defaultOpen: false, defaultHeight: 260, order: 40, refreshOn: ['expand', 'manual'] }`。
4. 文档登记：[performance.md](./performance.md) §3 缓存表加一行；本文件 §2 面板表加一行。
5. 完成。框架（WarDock/WarPanel/布局记忆/懒加载）零改动。

### 面板实现检查清单

- [ ] 折叠时零请求、零定时器、组件未挂载
- [ ] 数据有上限（分页/LIMIT/条数），大内容分片拉取
- [ ] 展开/折叠/拖拽高度全部入 `panelLayout` 记忆
- [ ] 外观只用 WarPanel 槽位，未自定义框样式
- [ ] refreshOn 触发实现了去抖/代际作废（快速连点不堆请求）

## 5. 实现检查清单（框架本身）

- [ ] WarDock/WarPanel 组件：手风琴堆叠 + 200ms 折叠动画
- [ ] 拖拽手柄：min 80px / max 60% 坞高、pointerup 后 300ms 防抖持久化
- [ ] panelLayout 读写并入 user_prefs（[data-formats.md](./data-formats.md) §7 加字段说明）
- [ ] 懒挂载：动态 import + 折叠卸载
- [ ] 三个首批面板（agent/git/files）按注册表机制实现，无写死特例
- [ ] L2 铁框 border 值从旧 QML 抄准并登记 ui-design.md 参数总表

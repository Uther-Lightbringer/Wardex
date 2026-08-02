# 信息面板坞（Panel Dock）与铁框视觉语言

> 相关文档：[architecture.md](./architecture.md) · [performance.md](./performance.md)（不可见不工作/面板数据预算）· [ui-design.md](./ui-design.md)（九宫格参数总表）· [assets.md](./assets.md) · [features/chat.md](./features/chat.md)

聊天页右栏是**可扩展的信息面板坞**：git 历史、文件树、后台任务是首批面板，将来陆续加入 git 对比、数据库信息、HTTP 调试等开发信息面板。本文档定义：面板坞架构（新面板 = 一个文件 + 一行注册）、交互规范（抽屉式右栏 + 拖拽调宽 + 布局记忆）、铁框三级视觉语言、新面板开发指南。

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
  defaultWidth: number;          // px，首次使用且无记忆时的宽度
  order: number;                 // 默认排序
  refreshOn: RefreshTrigger[];   // 数据刷新时机
}
type RefreshTrigger = 'turnEnd' | 'sessionSwitch' | 'expand' | 'manual';
```

### 1.2 容器行为（`src/components/war/WarDock.vue` + `WarPanel.vue`）

- **抽屉式右栏**（非手风琴）：折叠态为右缘一条 44px 竖排按钮栏（每面板一个按钮，图标或竖排中文标题）。点击按钮 → 该面板从右侧**滑入**（250ms ease），聊天区被挤窄（网格 `188px 1fr auto`，抽屉占据布局空间，**非 overlay** 浮层）。
- **互斥展开**：同一时刻至多一个面板打开；点击已打开面板的按钮或其标题条 → 折叠收回。
- **打开状态不持久化**：`panelLayout` 不记 `open`；应用启动时全部面板折叠。已移除旧的 `alwaysOpen`（agent 面板常驻）概念。
- **懒挂载**：折叠的面板不挂载组件、不取数据（[performance.md](./performance.md) §1.4 不可见不工作）；首次展开才动态 import + 拉数据。
- **拖拽调宽**：展开面板**左沿**有拖拽手柄（铁框风格，见 §3），pointer 拖拽实时改宽度；约束 min 200px / max 聊天区宽度的 60%。拖拽过程中内容区 `pointer-events: none` 防误触。
- **布局记忆**：每个面板的 `{width, order}` 按面板 id 持久化到 `user_prefs.json` 的 `panelLayout` 字段：
  ```json
  "panelLayout": { "git": { "width": 320 }, "tasks": { "width": 320 } }
  ```
  拖拽结束（pointerup）后 **300ms 防抖**写盘；下次打开同一面板恢复同样宽度。未登记的 id 使用 PanelDef 默认值。旧格式遗留的 `open`/`height` 键直接忽略，不做迁移。
- **排序**：order 持久化；拖拽换序为可后置项（v1 固定按 registry order）。

### 1.3 后端扩展点（`src-tauri/src/inspect/`）

- 每个信息域一个模块：`inspect/git.rs`（分支/log/diff）、`inspect/files.rs`（文件树）、将来 `inspect/db.rs` 等，各暴露自己的 Tauri 命令（如 `git_log`、`git_diff`、`db_tables`）。
- **不提前抽象统一 trait**：git diff 与数据库结果集形状差异太大，先按具体命令实现；等出现 3+ 个相似域再提炼公共模式。
- 数据预算：面板数据遵循 [performance.md](./performance.md) §3——折叠即弃、结果分页/限量（git log 默认 50 条、diff 按文件分片拉取、db 查询强制 LIMIT）。

## 2. 首批面板映射（现有右栏功能的重写）

| 面板 id | 标题 | 来源功能（旧版参照） | refreshOn |
|---|---|---|---|
| `agent` | 会话信息 | agent 切换器（ChatPage.qml 右栏） | sessionSwitch |
| `git` | 版本控制 | 分支徽标 + 只读提交历史 + 工作区更改列表（行显示 文件名 + 暗色目录前缀）+ diff 查看（**点击更改行或提交 → `GitCommitDialog` 弹框**：左侧变更文件列表（+增/−删计数），右侧 GitLab 风格单文件 diff（新旧行号 +/− 列、增删行绿/红底） | turnEnd, sessionSwitch, expand, manual |
| `files` | 工作区文件 | 文件树（点击预览/右键系统打开） | sessionSwitch, expand, manual |
| `tasks` | 后台任务 | agent 后台任务列表（`src/panels/TasksPanel.vue`）：subagent 管道中 `kind='task'` 条目，来源为 `task_id:` 启动应答与 TaskList/TaskOutput 更新；会话切换恢复走新 Tauri 命令 `get_subagents`（镜像 `RuntimeSnap.subagents`，chat store `openSession` 中拉取，与 `pending_permission` 同模式）。registry：`defaultOpen: false, defaultWidth: 320, order: 12` | turnEnd, sessionSwitch |

所有面板 `defaultOpen: false`（抽屉打开状态不持久化，启动全折叠，见 §1.2）。git 对比、数据库等为后续批次，按 §4 指南加入。

## 3. 铁框三级视觉语言

所有"框"统一为三级，**禁止发明第四种框样式**：

| 级别 | 贴图 | 用途 | border |
|---|---|---|---|
| L1 页面框 | `frame_chat_main.png` / `frame_panel*.png` | 页面级大区域（消息区、输入区） | 见 [ui-design.md](./ui-design.md) 参数总表 |
| **L2 面板框（无耳胖铁框）** | `frame_fat_bar.png`（标题条，slice 28/32/28/32）+ `frame_fat_panel.png`（内容框，slice [28,32,28,32]，孔 [24,26,24,24]） | **所有信息面板的默认容器** | 登记入 ui-design.md §2.2/§3.2 |
| L3 内嵌框 | `frame_bar.png` / `frame_action.png` | 面板内部小分区（diff 文件条目、db 表单行） | 同上 |

`WarPanel` 统一外观规格（面板作者只写内容区，样式漂移锁死在这一处）：

- **标题条**：`frame_fat_bar.png` 九宫格；左起 图标(可选) + 中文标题 + 弹性空间 + 折叠箭头；hover 微亮反馈。**点击标题条折叠面板**（收回抽屉）。
- **内容框**：`frame_fat_panel.png` 九宫格，内边距按贴图内嵌值；内容超高出 `WarScrollBar`（不可滚时隐藏轨道）。
- **拖拽手柄**：内容框**左沿**拖拽热区（调宽度），视觉为铁框边沿 grip，hover 时 `cursor: col-resize`（自定义光标体系内的等价处理，见 ui-design.md §光标）。
- **字号**：标题与内容文本按 `fontScale` 缩放，装饰符号不缩放（与全局规则一致）。

> 无耳胖框（fat 家族）替换了面板坞原先使用的 `frame_iron_bar.png`/`frame_iron_panel.png`；iron 贴图仍用于页面级 FrameImage 等其它处，见 [ui-design.md](./ui-design.md) §2.2。

## 4. 新面板开发指南（给执行模型的模板）

以"数据库信息"面板为例，完整步骤：

1. `src-tauri/src/inspect/db.rs`：实现命令（连接配置存 user_prefs 或项目级配置；查询强制 LIMIT；大结果分页）。在 lib.rs 注册命令。
2. `src/panels/DbPanel.vue`：只写内容区（表格/表单），外壳用 `<WarPanel>` 槽位，不碰框样式。
3. `src/panels/registry.ts` 加一条：`{ id: 'db', title: '数据库', component: () => import('./DbPanel.vue'), defaultOpen: false, defaultWidth: 300, order: 40, refreshOn: ['expand', 'manual'] }`。
4. 文档登记：[performance.md](./performance.md) §3 缓存表加一行；本文件 §2 面板表加一行。
5. 完成。框架（WarDock/WarPanel/布局记忆/懒加载）零改动。

### 面板实现检查清单

- [ ] 折叠时零请求、零定时器、组件未挂载
- [ ] 数据有上限（分页/LIMIT/条数），大内容分片拉取
- [ ] 拖拽宽度入 `panelLayout[id].width` 记忆（打开状态不持久化）
- [ ] 外观只用 WarPanel 槽位，未自定义框样式
- [ ] refreshOn 触发实现了去抖/代际作废（快速连点不堆请求）

## 5. 实现检查清单（框架本身）

- [ ] WarDock/WarPanel 组件：44px 按钮栏 + 250ms ease 抽屉滑入，互斥展开，网格 `188px 1fr auto` 挤压式（非 overlay）
- [ ] 打开状态瞬态：启动全折叠，不持久化；无 `alwaysOpen` 特例
- [ ] 左沿拖拽手柄：min 200px / max 60% 聊天区宽、pointerup 后 300ms 防抖持久化到 `panelLayout[id].width`
- [ ] panelLayout 读写并入 user_prefs（[data-formats.md](./data-formats.md) §7 字段说明），旧 `open`/`height` 键忽略不迁移
- [ ] 懒挂载：动态 import + 折叠卸载
- [ ] 四个面板（agent/git/files/tasks）按注册表机制实现，无写死特例
- [ ] L2 无耳胖框（frame_fat_bar / frame_fat_panel）slice/hole 值登记 ui-design.md 参数总表

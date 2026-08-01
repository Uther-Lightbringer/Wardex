# 素材清单与用法

> 相关文档：[architecture.md](./architecture.md) · [ui-design.md](./ui-design.md) · [design-principles.md](./design-principles.md) · [implementation-plan.md](./implementation-plan.md) · [features/main-menu-and-misc.md](./features/main-menu-and-misc.md)

本文档列出新版 `public/assets/` 下全部 53 个运行时素材（与旧版 `assets/` 路径一致，已拷贝完成），以及素材的使用约定。九宫格 border 值的详细用法见 [ui-design.md](./ui-design.md)。

## 来源与版权

- `ui/`（frames/buttons/dropdown/scroll/avatars/misc）—— 项目自制切图（由 `tools/` Python 脚本从 WC3 风格原始切图生成，带真 alpha 的 PNG）。
- `wc3_extracted/` —— 从 Warcraft III MPQ 提取的**暴雪版权素材，仅供个人使用**，不可商用分发。
- `Sound/` —— 3 个 WAV 音效。
- `background/LodolonFall.jpg` —— 内置默认背景。
- 旧仓库 `assets/ui/raw/`（原始切图源）与 `assets/storm_loop.mp4`（废弃视频背景）**运行时不需要，未拷贝**。
- 旧版通过 qrc 打包；新版为 `public/` 静态文件，前端以 `/assets/...` 绝对路径引用（Vite 开发服务器与 Tauri 生产包行为一致）。

## frames/ —— 九宫格面板框（29 张）

| 文件 | 用途 | border（上/右/下/左，源像素） |
|---|---|---|
| `ui/frames/frame_tall.png` | 主菜单钢板竖框（SteelPanel） | 见 ui-design.md |
| `ui/frames/frame_short.png` | 钢板短框 | 见 ui-design.md |
| `ui/frames/frame_wide.png` | 宽面板框 | 见 ui-design.md |
| `ui/frames/frame_tall.png` 同族 `frame_panel.png` / `frame_panel_wide.png` / `frame_panel_narrow.png` | 通用面板三规格 | 见 ui-design.md |
| `ui/frames/frame_action.png` / `frame_bar.png` | 操作条/横条 | 见 ui-design.md |
| `ui/frames/frame_fat_panel.png` / `frame_fat_bar.png` / `frame_fat_sides.png` | 粗框系列 | 见 ui-design.md |
| `ui/frames/frame_chat_main.png` | 聊天主区框 | 见 ui-design.md |
| `ui/frames/frame_chat_input.png` | 输入区框 | 见 ui-design.md |
| `ui/frames/frame_chat_bl.png` | 聊天左下角框 | 见 ui-design.md |
| `ui/frames/frame_chat_bubble_body.png` | 消息气泡正文框 | **16/16/14/14** |
| `ui/frames/frame_chat_bubble_slot.png` | 气泡头像槽 | 见 ui-design.md |
| `ui/frames/frame_rail.png` | ~~左栏会话 rail~~（**已弃用**，会话栏改用 frame_fat_bar） | 见 ui-design.md |
| `ui/frames/frame_edge_left.png` / `frame_edge_right.png` / `frame_edge_top.png` / `frame_edge_bottom.png` | 永久铁轨边框（四边） | 拉伸平铺，非九宫格 |
| `ui/frames/dialog_frame.png` | 模态对话框框（源约 863×602） | **56** |
| `ui/frames/select_panel.png` | 选择面板 | 见 ui-design.md |
| `ui/frames/frame_popup.png` / `frame_popup_small.png` | 弹出面板（最近项目用 small） | 见 ui-design.md；**中心是画好的深蓝纹理（非镂空），使用时给 `WarFrame` 加 `fill` 把中心块作为背景** |
| `ui/frames/frame_iron_panel.png` / `frame_iron_bar.png` | 铁面板/铁条 | 见 ui-design.md |

## buttons/ —— 三态按钮（6 张）

| 文件 | 用途 |
|---|---|
| `ui/buttons/btn_normal.png` / `btn_hover.png` / `btn_pressed.png` | 主按钮三态（宽高比 4.87，菜单标准宽 276），Web 端 `:hover`/`:active` 换背景图 |
| `ui/buttons/dialog_btn_normal.png` / `dialog_btn_hover.png` / `dialog_btn_pressed.png` | 对话框按钮三态（宽高比 5.34） |

## dropdown/ —— 下拉框（2 张）

| 文件 | 用途 | border |
|---|---|---|
| `ui/dropdown/dropdown_bar.png` | 下拉条（右端烤入金箭头，非九宫格拉伸） | 见 ui-design.md |
| `ui/dropdown/dropdown_panel.png` | 展开面板（也用于 WarMenu 右键菜单） | **20/16/14/13**（以 ui-design.md 为准） |

## scroll/ —— 滚动条（4 张）

| 文件 | 用途 |
|---|---|
| `ui/scroll/scroll_up.png` / `scroll_down.png` | 上下箭头按钮 |
| `ui/scroll/scroll_track.png` | 轨道（垂直拉伸） |
| `ui/scroll/scroll_thumb.png` | 滑块（九宫格：slice 18 14，端帽固定、中段拉伸） |

内容装得下时滚动条隐藏轨道、只留置灰箭头（旧版"置黑"已弃用，见 ui-design.md §4.5）。

## avatars/ —— 头像（2 张）

| 文件 | 用途 |
|---|---|
| `ui/avatars/avatar_agent.png` | Agent 默认头像 |
| `ui/avatars/avatar_user_default.png` | 用户默认头像（可在 user_prefs 自定义，见 data-formats.md §7） |

## misc/ —— 杂项（3 张）

| 文件 | 用途 |
|---|---|
| `ui/misc/chain_link.png` | 吊链，垂直平铺（`background-repeat: repeat-y`），可伸出窗口顶 |
| `ui/misc/cursor.png` | 默认光标原始素材（128×99） |
| `ui/misc/cursor_green.png` | 可交互元素光标原始素材（128×99） |
| `ui/misc/cursor_32.png` / `cursor_green_32.png` | 预缩放 32×25 光标（CSS 实际引用，热点 `1 0`） |

## wc3_extracted/ui/ —— WC3 提取图标（5 张，运行时必需）

| 文件 | 用途 |
|---|---|
| `wc3_extracted/ui/icon-folder.png` / `icon-folder-up.png` / `icon-file.png` | 文件夹浏览/文件树图标 |
| `wc3_extracted/ui/GlueScreen-Button-KeyboardHighlight.png` | 蓝色高亮条，加色混合 `mix-blend-mode: screen`（菜单高亮、最近项目 0.55 透明度） |
| `wc3_extracted/ui/GlueScreen-Profile-Stretch2.png` | 资料拉伸条（SteelPanel 装饰） |

## Sound/ —— 音效（3 个 WAV）

| 文件 | 事件 | 说明 |
|---|---|---|
| `Sound/BigButtonClick.wav` | `click` | WarButton 点击 |
| `Sound/RightGlueScreenPopUp.wav` | `popUp` | 面板拉起，可闻长度 **1280ms**，音画时序以其为准 |
| `Sound/RightGlueScreenPopDown.wav` | `popDown` | 面板降下 |

播放规则：同名 200ms 节流、播新停旧、启动预载（详见 ui-design.md §音效 与 features/main-menu-and-misc.md）。

## background/

| 文件 | 用途 |
|---|---|
| `background/LodolonFall.jpg` | 内置默认背景（缺省/配置缺失/类型非法时回退） |

## 外部背景配置约定

- exe 旁放 `background.json` 可切换背景（发布包附带 `background.example.json` 样例）：
  ```json
  { "type": "image", "source": "/assets/background/LodolonFall.jpg 或绝对路径" }
  ```
- `type`：`image`（jpg/png/gif/webp，gif/webp 动图）| `model`（glTF/GLB，Three.js 加载，**可后置**）；`video` 已移除，出现即回退默认 image。
- 缺省/解析失败一律回退内置 `LodolonFall.jpg`。

## 新增素材约定

- 新贴图放入对应分类子目录（`ui/frames/`、`ui/buttons/` …），命名沿用 `frame_*` / `btn_*` / `scroll_*` 规律。
- 新音效放入 `Sound/`，并在音效模块注册事件名与节流规则。
- 新素材加入后需在 [ui-design.md](./ui-design.md) 登记九宫格 border 值（如适用）并在本表登记用途。
- 不要把素材挪进 `src/` 由 Vite 打包——保持 `public/` 静态引用，路径稳定且与 background.json 的外部路径约定一致。

## 实现检查清单

- [ ] 53 个文件全部以 `/assets/...` 路径可访问（dev 与生产包）
- [ ] 每张九宫格贴图的 border 值与 ui-design.md 登记一致
- [ ] 三态按钮 hover/pressed 切换无布局跳动（三图同尺寸）
- [ ] 光标 url() 加载失败有 `auto`/`pointer` 兜底
- [ ] 音效预载 + 200ms 节流 + 播新停旧
- [ ] background.json 读取/回退逻辑与旧版一致
- [ ] 版权提示：wc3_extracted 素材不用于商用分发

# WarDex UI 设计规格（WC3 视觉体系）

> 本文档是 WarDex Tauri 重写版（Rust + Vue 3）的界面实现依据。所有数值均逐行抄自旧版
> C++/Qt6 项目（`C:/workspace/WarDex`，只读参照）的 QML 源码，未做任何估算。
> 读者应按本文档 + `assets.md` 即可 1:1 复刻界面，无需查阅旧仓库。

## 相关文档

- [./architecture.md](./architecture.md) — 总体架构（Rust 后端 / Vue 前端划分）
- [./design-principles.md](./design-principles.md) — 设计原则与代码约定
- [./implementation-plan.md](./implementation-plan.md) — 实施计划与里程碑
- [./features/chat.md](./features/chat.md) — 聊天页功能规格
- [./features/main-menu-and-misc.md](./features/main-menu-and-misc.md) — 主菜单与杂项功能规格
- [./assets.md](./assets.md) — 素材清单与用法（本文档的姊妹篇）
- [./panels.md](./panels.md) — 信息面板坞与铁框三级视觉语言（L2 铁框的用法规范）

---

## 1. 设计基调

### 1.1 风格定位

魔兽争霸 III（Warcraft III）主菜单风格：铆钉铁框 + 石材描金 + 深色半透明"玻璃"内容区。
所有控件均为自绘贴图（**不使用任何原生控件样式**），界面文本为纯中文。

### 1.2 色板（全部抄自 QML 源码）

> **重要：Qt 色值格式是 `#AARRGGBB`（alpha 在前），CSS 是 `#RRGGBBAA`（alpha 在后）。**
> 下表已统一换算为 CSS 格式（8 位 hex），可直接使用。不带 alpha 的 6 位色值两格式相同。

**核心色（高频，务必用 CSS 变量管理）：**

| CSS 变量建议名 | CSS 值 | Qt 原值 | 用途 |
|---|---|---|---|
| `--war-gold` | `#f2cf6b` | 同 | 主金色：按钮文字、标题、选中/高亮文字（出现 52 次） |
| `--war-gold-bright` | `#ffe9a0` | 同 | 亮金：hover/焦点态金色文字 |
| `--war-gold-dim` | `#c9a227` | 同 | 暗金：输入框焦点边框、选中卡片边框 |
| `--war-gold-input` | `#8a6f24` | 同 | 黄铜：行内编辑框边框、列表选中边框 |
| `--war-text` | `#e8ecf4` | 同 | 正文主色（近白） |
| `--war-text-dim` | `#cfd6e4` | 同 | 次级文字：面板标题（Georgia）、下拉项、菜单项 |
| `--war-text-muted` | `#8a93a5` | 同 | 弱化文字：时间戳、占位、详情 |
| `--war-text-faint` | `#5a6272` | 同 | 最弱：空态提示、禁用菜单项（`#5a6472` 用于版本号） |
| `--war-glass` | `#0b0d12a6` | `#A60b0d12` | **统一玻璃层**：所有铁框内容区的半透明底色（alpha 0xA6≈65%） |
| `--war-glass-border` | `#00000044` | `#44000000` | 玻璃层描边（FrameImage.recessBorder） |
| `--war-error` | `#d08070` | 同 | 错误文字/错误态 |
| `--war-interrupted` | `#a09070` | 同 | 已中断状态色 |
| `--war-user-blue` | `#7eb6ff` | 同 | 用户侧名字、Git 图标等用户标识蓝 |
| `--war-outline-brown` | `#241500` | 同 | 金色文字描边色（Text.Outline styleColor） |
| `--war-outline-dark` | `#1a1000` | 同 | WarDialog 标题描边色 |

**背景与基底：**

| CSS 值 | Qt 原值 | 用途 |
|---|---|---|
| `#05080a` | 同 | 窗口底色（ApplicationWindow.color） |
| `#0e2a22 → #0a1a16(60%) → #04070a` | 同 | 背景图之下的垂直渐变底 |
| `#00000000 → #00000020(55%) → #00000090(100%)` | `#00000000`/`#20000000`/`#90000000` | 背景图之上的垂直压暗渐变 |
| `#0a1a16` | 同 | 3D 背景 clearColor |
| `#000000b0` | `#B0000000` | 模态遮罩（Overlay.modal / dim） |

**聊天气泡 / 结构化块：**

| CSS 值 | Qt 原值 | 用途 |
|---|---|---|
| `#2a151888` | `#882a1518` | 错误消息玻璃（替代 `--war-glass`） |
| `#f2cf6b66` | `#66f2cf6b` | 流式中气泡的 1px 金色呼吸边框 |
| `#19151044` + 边框 `#4a4232` | `#44191510` | thinking（思考过程）折叠块 |
| `#12151c44` + 边框 `#3a4a40` | `#4412151c` | tool（工具调用）折叠块 |
| `#c8b890` | 同 | thinking 标题文字 |
| `#908878` | 同 | thinking 展开正文 |
| `#d0d6e0` | 同 | tool 标题文字 |
| `#1a2334` + 边框 `#2c4a7a` | 同 | 附件芯片（非图片文件） |
| `#c0d0ec` | 同 | 附件文件名文字 |

**列表 / 表单杂项：**

| CSS 值 | Qt 原值 | 用途 |
|---|---|---|
| `#10141f` + 边框 `#8a6f24` | 同 | 行内编辑输入框（重命名/新建文件夹） |
| `#32509640` / `#32509633` | `#40325096` / `#33325096` | 列表行 hover 蓝（40=行hover，33=新建行底） |
| `#15192299` + 边框 `#2a3344` | `#99151922` | 配置页只读字段底 |
| `#0d1116f0` + 边框 `#6a5a3f` | `#F00d1116` | 浮动条（子Agent面板/发送队列/附件条）黄铜边框深色底 |
| `#201018c0` + 边框 `#f2cf6b` | `#c0201018` | 顶部 banner 通知条 |
| `#10141dcc` | `#cc10141d` | 最近项目条目底 |
| `#1a2230` / `#2c4a7a` | 同 | 最近项目条目边框（常态/hover） |
| `#6d7688` / `#8b93a6` | 同 | 最近项目日期/路径文字 |
| `#9aa2b2` | 同 | ".."上一层行文字 |
| `#9adf8f` + 底 `#000000b0` + 边 `#3a4a3a` | `#B0000000` | 性能 HUD（debug） |
| `#04050a` | 同 | SteelPanel 玻璃 1px 描边 |
| `#7a8070` | 同 | 禁用按钮文字 |
| `#0a0c10` | 同 | 会话列表条目边框 |
| `#4a3c14` / `#3a2f10` | 同 | 会话列表条目 hover 边框等暗金边 |

### 1.3 字体

| 角色 | 字体族 | 说明 |
|---|---|---|
| 正文 / 按钮 / 输入 / 中文 | `SimSun`（宋体） | Qt 里大量显式指定；CSS 写 `font-family: SimSun, serif` |
| 面板标题（SteelPanel header、各页面板大标题） | `Georgia` | 17~18px，`letter-spacing: 2px`，配黑色文字描边 |
| 性能 HUD | `Consolas` | 12px，仅 debug |

Vue 里建议定义全局工具类：

```css
.war-font-body  { font-family: SimSun, serif; }
.war-font-title { font-family: Georgia, serif; letter-spacing: 2px; }
/* Qt Text.Outline 的 CSS 等价：四向 1px 描边 */
.war-outline-black { text-shadow: -1px 0 #000, 1px 0 #000, 0 -1px #000, 0 1px #000; }
.war-outline-gold  { text-shadow: -1px 0 #241500, 1px 0 #241500, 0 -1px #241500, 0 1px #241500; }
```

### 1.4 全局字号缩放

- `userPrefs.fontScale`，clamp 到 **0.85 ~ 1.30**，配置页下拉档位固定为 `[0.85, 1.0, 1.15, 1.30]`（`UserPrefs.cpp:98`、`ConfigPage.qml:529`）。
- 各页面定义 `fs(n) = Math.round(n * fontScale)`，**阅读类文字**（正文、thinking、时间戳、表单标签）一律走 `fs()`；控件贴图内文字（WarButton 标签）不走，自己有宽度公式。
- Vue 实现：在根组件放一个响应式 `fontScale`（存用户配置），用 `provide/inject` 或 pinia store 暴露 `fs(n)` 工具函数。

```ts
// utils/fontScale.ts
export const fontScale = ref(1.0)
export function fs(n: number) { return Math.round(n * fontScale.value) }
```

---

## 2. 九宫格体系：Qt BorderImage → CSS border-image

### 2.1 映射规则（逐条对应，照抄即可）

| Qt BorderImage | CSS border-image | 备注 |
|---|---|---|
| `border.top / right / bottom / left` | `border-image-slice: <T> <R> <B> <L>` | **顺序不同**：QML 属性和 CSS 都是 T R B L，直接按数值顺序抄 |
| `horizontalTileMode/verticalTileMode: Stretch` | `border-image-repeat: stretch` | 默认值，本项目绝大多数贴图用它 |
| `…: Repeat` | `border-image-repeat: repeat` | **仅 `frame_chat_bubble_body.png` 用 Repeat** |
| 中心区块 | 默认丢弃 | Qt 会把中心切片也拉伸绘制，但本项目所有九宫格贴图中心都是透明孔，CSS 不加 `fill` 关键字视觉完全一致 |
| `source` | `border-image-source: url(...)` | |

标准 CSS 写法（以 dialog_frame 为例）：

```css
.war-dialog-frame {
  /* 边框带本身参与布局：透明 border 占位，border-image 画在上面 */
  border-style: solid;
  border-color: transparent;
  border-width: 56px;                     /* T R B L 同值 */
  border-image-source: url('/assets/ui/frames/dialog_frame.png');
  border-image-slice: 56;                 /* 无单位 = 源图像素 */
  border-image-repeat: stretch;
  box-sizing: border-box;
}
```

不同值时按 T R B L 顺序写两个属性：

```css
/* dropdown_panel.png: Qt border{left:20; right:16; top:14; bottom:13} */
border-width: 14px 16px 13px 20px;
border-image: url('/assets/ui/dropdown/dropdown_panel.png') 14 16 13 20 stretch;
```

### 2.2 全部九宫格贴图的精确参数（一个都不能漏）

| 贴图（`public/assets/` 下） | 源尺寸(px) | slice T/R/B/L | repeat | 使用处（旧版） |
|---|---|---|---|---|
| `ui/frames/dialog_frame.png` | 863×602 | 56 / 56 / 56 / 56 | stretch | WarDialog 模态框 |
| `ui/frames/frame_chat_bubble_body.png` | 1037×168 | 14 / 16 / 14 / 16 | **repeat** | ChatBubble 正文框（角固定、石边平铺） |
| `ui/frames/frame_iron_panel.png` | 717×276 | 96 / 110 / 69 / 108 | stretch | FrameImage 页面主面板（Config/SessionSelect/Todo/Chat 顶部） |
| `ui/frames/frame_iron_bar.png` | 717×243 | 62 / 110 / 70 / 108 | stretch | FrameImage 底部操作栏 |
| `ui/frames/frame_rail.png` | 392×656 | 92 / 36 / 40 / 36 | stretch | ~~ChatPage 左侧"本项目会话"栏~~（**已弃用**，会话栏改用 frame_fat_bar） |
| `ui/frames/frame_fat_bar.png` | 804×554 | 28 / 32 / 28 / 32 | stretch | ChatPage 左侧"本项目会话"栏（粗铆钉框，中心镂空） |
| `ui/frames/frame_popup.png` | 1023×548 | 88 / 100 / 90 / 100 | stretch | FolderBrowserDialog 列表框、ChatPage 文件预览弹窗 |
| `ui/frames/frame_popup_small.png` | 511×274 | 44 / 50 / 45 / 50 | stretch | RecentProjectsPanel（frame_popup 的半尺寸版，角块 50/50/44/45）；中心块是画好的深蓝纹理，**需加 `fill`** 才能作为背景显示 |
| `ui/dropdown/dropdown_bar.png` | 109×36 | 12 / 46 / 12 / 29 | stretch | WarDropdown 关闭态条（右侧 46px 含金箭头帽） |
| `ui/dropdown/dropdown_panel.png` | 100×75 | 14 / 16 / 13 / 20 | stretch | WarDropdown 展开列表、WarMenu 底、FolderBrowserDialog 标题牌/路径栏 |
| `wc3_extracted/ui/GlueScreen-Profile-Stretch2.png` | 64×512 | 8 / 8 / 8 / 8 | stretch | ChatPage Git 分支徽标底 |

非九宫格、整体拉伸/适配的贴图（**不要**给它们配 border-image）：
`frame_tall.png`(835×1000)、`frame_short.png`(826×482)、`frame_chat_bubble_slot.png`(180×163, PreserveAspectFit)、
`frame_edge_left/right.png`(120×720)、`chain_link.png`(155×78, 垂直平铺)、全部 `btn_*.png` / `dialog_btn_*.png`、
`scroll_*.png`、头像、图标、光标、`GlueScreen-Button-KeyboardHighlight.png`(128×64, 拉伸)。

### 2.3 Vue 组件 `WarFrame` 设计建议

```vue
<!-- components/WarFrame.vue —— 通用九宫格框 -->
<template>
  <div class="war-frame" :style="frameStyle">
    <div class="war-frame__content"><slot /></div>
  </div>
</template>

<script setup lang="ts">
const props = withDefaults(defineProps<{
  src: string                 // /assets/... 路径
  slice: [number, number, number, number]  // [T, R, B, L]，源图像素
  repeat?: 'stretch' | 'repeat'            // 默认 stretch
}>(), { repeat: 'stretch' })

const frameStyle = computed(() => ({
  borderStyle: 'solid',
  borderColor: 'transparent',
  borderWidth: props.slice.map(v => `${v}px`).join(' '),
  borderImageSource: `url('${props.src}')`,
  borderImageSlice: props.slice.join(' '),
  borderImageRepeat: props.repeat,
  boxSizing: 'border-box',
}))
</script>

<style scoped>
.war-frame { position: relative; }
.war-frame__content { width: 100%; height: 100%; }
</style>
```

用法：`<WarFrame src="/assets/ui/frames/dialog_frame.png" :slice="[56,56,56,56]">…</WarFrame>`

> 注意：`border-image` 会盖在 `border` 区域上，内容区自动位于中心切片内；
> 内容超出时给 `.war-frame` 配 `min-height` 即可，四角永不拉伸变形。

---

## 3. FrameImage「内容开孔」机制（重点）

`FrameImage.qml` 是页面级铁框的核心：贴图中心是透明孔，孔下垫统一玻璃层，内容嵌在孔里。
旧版有两种模式，**新版只需实现像素模式（模式 A）**，分数 fallback（模式 B）可完全忽略。

### 3.1 三层结构（z 序自下而上）

| 层 | z | 位置计算（像素模式） | 外观 |
|---|---|---|---|
| glass（玻璃） | 0 | inset = `max(2, hole* - 8)` 四边 | `background: #0b0d12a6`，`border-radius: 2px`。玻璃**塞进铁边底下 8px**（fillTuck），保证铁边毛糙内缘不露缝 |
| iron（铁框） | 1 | 铺满整个组件 | 九宫格 border-image，`pointer-events: none` |
| content（内容裁剪） | 3 | inset = `hole* + 2` 四边（contentGap=2 呼吸缝） | 透明（玻璃色来自底层）；内部再有内边距：左右 `10 + content*Extra`、上下 `8`；`overflow: hidden` |

关键常量（抄自 FrameImage.qml）：
- `contentGap = 2`（铁边内缘与内容区间的呼吸缝）
- `fillTuck = 8`（玻璃向铁边下塞入的距离，必须小于铁边厚度）
- `contentLeftExtra / contentRightExtra`（调用点按需右推文字避让永久铁轨，见各页面用法）

### 3.2 三张带孔贴图的精确参数（从各页面调用点抄）

| 贴图 | 源尺寸 | border L/R/T/B | hole L/R/T/B | 常见 extras |
|---|---|---|---|---|
| `frame_iron_panel.png` | 717×276 | 108/110/96/69 | **24/25/56/21** | `hasHangers: true`；左栏 `contentLeftExtra: 16`（避让铁轨），Chat 页用 `4/4`、`4/6` |
| `frame_iron_bar.png` | 717×243 | 108/110/62/70 | **24/24/22/21** | `hasHangers: false`；Chat 页 `4/4`，Config 页 `0/0` |
| `frame_fat_bar.png` | 804×554 | 32/32/28/28 | **26/26/24/24** | 无 hangers、无 extras（ChatPage 会话栏） |
| ~~`frame_rail.png`~~ | 392×656 | 36/36/92/40 | 24/23/77/26 | **已弃用**，会话栏改用 frame_fat_bar |

> `hasHangers` 只影响旧版 fallback 模式的分数内嵌，像素模式下**无实际作用**，新版可忽略该参数。

### 3.3 CSS/Vue 实现建议

```vue
<!-- components/WarIronFrame.vue —— FrameImage 像素模式等价物 -->
<template>
  <div class="war-iron" :style="ironStyle">
    <div class="war-iron__glass" :style="glassStyle"></div>
    <div class="war-iron__content" :style="contentStyle"><slot /></div>
  </div>
</template>

<script setup lang="ts">
const props = withDefaults(defineProps<{
  src: string
  slice: [number, number, number, number]   // [T,R,B,L]
  hole: [number, number, number, number]    // [T,R,B,L] 源像素孔位
  contentLeftExtra?: number
  contentRightExtra?: number
}>(), { contentLeftExtra: 0, contentRightExtra: 0 })

const TUCK = 8, GAP = 2
const [t, r, b, l] = props.hole
const glassStyle = {
  inset: `${Math.max(2, t - TUCK)}px ${Math.max(2, r - TUCK)}px ${Math.max(2, b - TUCK)}px ${Math.max(2, l - TUCK)}px`,
}
const contentStyle = {
  inset: `${t + GAP}px ${r + GAP}px ${b + GAP}px ${l + GAP}px`,
  padding: `8px ${10 + props.contentRightExtra}px 8px ${10 + props.contentLeftExtra}px`,
}
const ironStyle = {
  borderStyle: 'solid', borderColor: 'transparent',
  borderWidth: props.slice.map(v => `${v}px`).join(' '),
  borderImageSource: `url('${props.src}')`,
  borderImageSlice: props.slice.join(' '),
  borderImageRepeat: 'stretch',
  boxSizing: 'border-box',
}
</script>

<style scoped>
.war-iron { position: relative; }
.war-iron__glass {
  position: absolute; z-index: 0;
  background: var(--war-glass, #0b0d12a6); border-radius: 2px;
}
.war-iron__content { position: absolute; z-index: 2; overflow: hidden; }
/* border-image 画在元素自身 border 带上，天然位于 glass 之上 */
</style>
```

---

## 4. 控件精确规格

### 4.1 WarButton（菜单大按钮）

贴图与比例：

| 皮肤 | 贴图（normal/hover/pressed） | 源尺寸 | artAspect |
|---|---|---|---|
| 默认（主菜单/页面） | `ui/buttons/btn_normal.png` / `btn_hover.png` / `btn_pressed.png` | 560×115 | **4.87** |
| 对话框 | `ui/buttons/dialog_btn_normal.png` / `_hover` / `_pressed` | ≈684×128 | **5.34** |

几何与行为（抄自 WarButton.qml）：

- 标准宽 `menuWidth = 276`，高 = `round(width / artAspect)` ≈ 57；**高永远由宽推出，外部不许设高**。
- 禁用或全局过渡中（uiGate.busy）：`opacity: 0.38`，不响应悬停/点击/快捷键。
- 三张 `<img>` 叠放，按 hover/pressed 切换 `visible`（**不要**用 CSS filter 模拟，直接换图）。
- 文字：SimSun bold，居中，宽 = 按钮宽 × 0.88，右省略；
  字号 `max(13, min(19, round(width × 0.075)))`；
  颜色：常态 `#f2cf6b` / 按下 `#ffffff` / 禁用 `#7a8070`；描边 `#241500`。
- 点击音效 `click`（见 §7）；可配单字母快捷键（主菜单 O/C/L/S/T/A，仅主页可见时生效）。

```vue
<!-- components/WarButton.vue 骨架 -->
<template>
  <div class="war-btn" :class="{ disabled: !enabled || gated }"
       :style="{ height: Math.round(width / artAspect) + 'px' }"
       @click="trigger" @mouseenter="hover=true" @mouseleave="hover=false"
       @mousedown="pressed=true" @mouseup="pressed=false">
    <img :src="currentSrc" :style="{ aspectRatio: String(artAspect) }" draggable="false" />
    <span class="war-btn__label" :style="{ fontSize: labelSize + 'px' }">{{ text }}</span>
  </div>
</template>
<style scoped>
.war-btn { position: relative; width: 276px; user-select: none; }
.war-btn img { position: absolute; inset: 0; width: 100%; }
.war-btn.disabled { opacity: 0.38; pointer-events: none; }
.war-btn__label {
  position: absolute; inset: 0; display: flex; align-items: center; justify-content: center;
  padding: 0 6%; font-family: SimSun, serif; font-weight: bold; color: var(--war-gold);
  text-shadow: -1px 0 #241500, 1px 0 #241500, 0 -1px #241500, 0 1px #241500;
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}
.war-btn:active .war-btn__label { color: #fff; }
</style>
```

### 4.2 WarDialog（模态对话框）

- 底：`dialog_frame.png`（863×602，≈1.43:1）九宫格 slice 56。
- 宽 `dialogWidth = 560`（不超过窗口 90%），高 = `round(width / 1.4333)`。
- 遮罩：全屏 `#000000b0`。
- **标题/正文板**（上部黑色金边区，贴图自带金边，透明定位）：
  `x=12% y=14.5% w=76% h=35.5%`（相对对话框）；内部 Column 宽 = 板宽 − 28，spacing 8，居中。
  - 标题：`#f2cf6b`，bold，SimSun，`max(15, round(板高 × 0.11))`，描边 `#1a1000`，居中。
  - 正文：`#e8ecf4`，`max(13, round(板高 × 0.09))`，居中，最多 6 行右省略。
- **按钮区**（下部蓝色区）：`x=10% y=56% w=80% h=30%`（`buttonZoneY=0.56`、`buttonZoneH=0.30`，按钮竖排时可调高/扩区；贴图内容区下缘 ≈0.91）。
- 按钮横向 Row 排列，用对话框皮肤按钮（宽 190、artAspect 5.34）。
- Esc 关闭；开/关时需在遮罩与弹窗上恢复自定义光标（Web 版用 CSS cursor 天然解决，见 §6）。

### 4.3 WarDropdown（下拉框）

- 关闭态：`dropdown_bar.png` slice `12/46/12/29`；默认 140×32。
- 条上文字：SimSun bold `#f2cf6b`，左 margin 12、右 margin 44（避开右侧烤进贴图的金箭头），右省略。
- 展开层：`dropdown_panel.png` slice `14/16/13/20`，宽 `max(条宽, 120)`，上下 padding 10；
  位置：条下 2px；`dropUp: true` 时翻到条上方 2px（贴底栏用）。
- 选项行高 28：hover 时整行垫 `GlueScreen-Button-KeyboardHighlight.png`（左右各缩 8，拉伸，混合见 §9）；
  文字左 14 右 10；当前项或 hover 项 `#f2cf6b`（当前项加粗），其余 `#cfd6e4`。
- 点击条切换开合；选中即关并 emit。

### 4.4 WarMenu（右键/上下文菜单）

- 底：`dropdown_panel.png` slice `14/16/13/20`，最小宽 160；padding 上10 下10 左6 右6。
- 菜单项高 28，文字 SimSun 13px 左 padding 8：
  - 高亮：`#f2cf6b` + bold，背景垫 KeyboardHighlight 贴图（拉伸，见 §9）
  - 常态：`#cfd6e4`
  - 禁用：`#5a6272`

### 4.5 WarScrollBar（垂直滚动条）

四张贴图：`scroll_up.png` / `scroll_down.png`（82×82）、`scroll_track.png`（44×382）、`scroll_thumb.png`（68×79）。

- 总宽 22；上箭头 22×22 置顶、下箭头 22×22 置底；track 区上下各让 22、左右各让 3 拉伸；thumb 为九宫格（slice 18 14 fill，上下端帽+圆角固定、中间平段拉伸，永不变形），最小高度 40px、最小比例 0.08。
- 箭头按下时 `transform: scale(0.92)`。
- **禁用隐藏轨道**：内容装得下时（`size <= 0 || size >= 0.999`）thumb+track 隐藏（`visibility: hidden` 占位不抖动）、箭头禁点并降至 `opacity: 0.35` —— 替代旧版整条盖黑 0.68 的 WC3 置黑行为（黑条观感不佳，2026-08 改）。

```css
.war-scroll { width: 22px; position: relative; }
.war-scroll__track { position: absolute; inset: 22px 3px; }
.war-scroll__arrow { width: 22px; height: 22px; }
.war-scroll__arrow:active { transform: scale(0.92); }
.war-scroll__blackout { position: absolute; inset: 0; background: #000; opacity: 0.68; }
```

### 4.6 SteelPanel（链条悬挂钢面板，主菜单专用）

- 框体整图拉伸（非九宫格）：`frame_tall.png`（tall）/ `frame_short.png`（short）。
- **比例与开孔分数**（抄自 SteelPanel.qml 代码值；注意 short 实际 PNG 为 826×482，但布局用代码值）：

| variant | frameRatio（高/宽） | insetX | insetTop | insetBottom |
|---|---|---|---|---|
| tall | 1000/835 ≈ 1.1976 | 0.071 | 0.108 | 0.059 |
| short | 500/835 ≈ 0.5988 | 0.066 | 0.214 | 0.081 |

- 高 = `chainHeight + width × frameRatio`。
- **双链**：`chain_link.png`（155×78）垂直平铺；链宽 = 面板宽 × 0.186；
  两条链 x 中心分别在面板宽 × **0.2305** 和 × **0.658**（对齐框体上的铆板）；
  链条容器高 = `chainHeight + 22 + chainExtra + chainOverlapUp`，底边与框顶重叠 22px（bottomMargin −22）；
  `chainExtra` 让链条伸出窗口顶（主菜单 menuPanel 用 400），`chainOverlapUp` 让链顶插入上方悬挂的面板。
- 内容玻璃：`#0b0d12a6` + 1px 边 `#04050a`，按上表分数内嵌。
- 标题：Georgia 17px `#cfd6e4`，letter-spacing 2，黑描边，玻璃顶下 10px 居中；无标题时内容垂直居中。
- 主菜单实例：menuPanel（宽 344、chainHeight 50、chainExtra 400、右 margin 36、顶 margin 8，5 个按钮列宽 250 间距 10）；exitPanel（short、chainHeight 24、chainOverlapUp 18，挂在 menuPanel 正下方）。

### 4.7 ChatBubble（聊天气泡）

- 头像槽在正文框**外侧**：`frame_chat_bubble_slot.png`（180×163，PreserveAspectFit），槽 64×58 贴顶；
  头像 48×48 居中，底 `#141018` 圆角 2，头像图内缩 2 裁切填充（object-fit: cover）；
  用户消息整体右对齐、槽在右且槽图**水平镜像**（`transform: scaleX(-1)`）。
- 槽与框缝 `slotGap = 3`；整组最大宽 `floor(列宽 × 0.82)`；框最大宽 = 组宽 − 64 − 3（下限 120）。
- 正文框：`frame_chat_bubble_body.png` 九宫格 slice `14/16/14/16`，**repeat 平铺**；
  框内玻璃 inset 等于 slice 值，色 `#0b0d12a6`（错误 `#2a151888`）；流式中多 1px 边 `#f2cf6b66`。
- 框宽规则：短消息按内容自然宽（下限 140），流式/含 thinking·tool 块的消息直接用最大宽（免抖动）。
- 头部行（用户右→左排布）：名字（用户 `#7eb6ff` / Agent `#f2cf6b`，fs(12) bold 黑描边）· 时间 `#8a93a5` fs(11) · 状态（生成中 `#f2cf6b` / 错误 `#d08070` / 已中断 `#a09070`，fs(11) bold）· 复制链接 `#a0a8b8`（闪绿 `#80f0a0` 1.2s）。
- 正文：fs(14) SimSun `#e8ecf4`（错误/中断变色同上）；流式期纯文本，完成后按 Markdown 渲染。
- thinking 块：底 `#19151044` 边 `#4a4232`，标题 `#c8b890` fs(12)（▼/▶ 前缀折叠），展开文 `#908878` fs(11)。
- tool 块：底 `#12151c44` 边 `#3a4a40`，标题 `#d0d6e0` fs(12)（`· 工具名 [状态]`），展开 payload `#8a93a5` fs(11) 等宽感纯文本。
- 图片附件：内嵌显示，宽 `min(280, 内容宽)`；其他文件为芯片：底 `#1a2334` 边 `#2c4a7a`，15px `icon-file.png` + 文件名 `#c0d0ec` fs(11)。

### 4.8 浮动条家族（SubagentPanel / 发送队列 / 附件条）

统一外观：`background: #0d1116f0`，1px 边 `#6a5a3f`，圆角 3。
SubagentPanel：26px 折叠头（`#f2cf6b` 12px）+ 列表；状态点 8px 圆：执行中 `#57d977`（0.55s 呼吸闪烁）、等待 `#f2cf6b`、失败 `#d08070`、其余 `#4a5265`；标题 `#d0d6e0`（完成 `#8a92a2`），meta `#6d7688`。

---

## 5. 布局与窗口

### 5.1 窗口

- 默认 **1280×720**，最小 **960×600**，窗口底色 `#05080a`。
- 主菜单 UI 缩放：`uiScale = max(0.45, min(windowW / 1280, windowH / 720))`——小窗整体等比缩小（以左/右上角为变换原点），Vue 里用 `transform: scale()` 实现。

### 5.2 永久铁轨边框（permanentRails）

- `frame_edge_left.png` / `frame_edge_right.png`（120×720）各拉伸为 **58px 宽** 通高竖条，固定在窗口左右，`z-index: 40`。
- **创建一次、永不随页面切换销毁或滑动**；页面内容嵌在其下（见下）。首帧就要可见（同步解码等价物：`<img>` 直接放 HTML 而非懒加载）。
- `frame_edge_top/bottom.png` 在 qrc 登记但旧版未使用，备用。

### 5.3 ShellFrame 与页面嵌入

- 内容带左右各让 `contentLeft = max(0, edgeW − embed)`：`edgeW = 58`，`embed` 默认 50；
  实际页面取值：**ChatPage embed 34，ConfigPage / SessionSelectPage / TodoPage embed 52**（面板伸进铁轨下方的深度）。
- 内容宽 = 窗口宽 − 2 × contentLeft。

### 5.4 页面分栏公式（抄自各页）

| 页面 | gap | 上/下分 | 左/右分 |
|---|---|---|---|
| ChatPage | 8 | botH = max(menuBtnH+67, …)，topH = 余量 | railW = 196（会话栏）+ leftW = 剩余×0.72 + rightW = 余量 |
| ConfigPage | 10 | botH ≈ 两按钮+48，topH = 余量 | leftW = (宽−gap)×0.48 |
| SessionSelectPage | 10 | botH = max(188, 高×0.20) | leftW = (宽−gap)×0.62 |
| TodoPage | 10 | 不分 | leftW = (宽−gap)×0.62 |

### 5.5 主菜单布局

- 左轨：`RecentProjectsPanel` 360×460，位于 (76, 96)（未缩放坐标系），`frame_popup_small.png` 九宫格 `44/50/45/50`；
  内容内边距 左31 右33 上23 下24（金边内缘 L28 T20 R30 B21 + 缝隙）；
  标题"最近项目" Georgia 17 `#cfd6e4` 黑描边；条目高 52：名称 `#f2cf6b` 15 bold / 日期 `#6d7688` 11 / 路径 `#8b93a6` 11 中间省略；
  条目底 `#10141dcc`，hover `#1a2334` + KeyboardHighlight(opacity 0.55) + 边 `#2c4a7a`，编辑态边 `#8a6f24`；右键出 WarMenu（重命名/从列表移除）。
- 右轨：400 宽缩放容器，内放 menuPanel + exitPanel（见 §4.6）。
- 左下角版本文字：`#5a6472` 12px。

### 5.6 页面切换三段式动画（核心体验，务必复刻）

所有导航都是同一套「上拉 → 等音效 → 下拉」：

1. **上拉（770ms）**：当前页 `y: 0 → −2400`，`ease-in-quad`，同时播 `popUp` 音效。
2. **等音效余量（510ms）**：`popUpSoundMs(1280) − popUpDur(770) = 510ms`——上拉动画结束后等 popUp 音效放完（文件 1866ms，尾部 ~590ms 低于 2% 峰值不可闻，可闻长度按 1280ms 计）。
3. **下拉（770ms）**：新页面从 `y = −height`（停靠在视口上方）落到 `y = 0`，`ease-out-quad`，动画开始同时播 `popDown` 音效。

补充规则：
- 全程 `uiBusy = true`：所有 WarButton 变灰禁用、快捷键失效，直到下落结束。
- 新页面在变得可见**之前**必须先停靠到视口上方（prepareEnter），防止闪一帧最终位置。
- 永久铁轨不动；主菜单 ↔ 覆盖页是「菜单上拉/大框下拉」，覆盖页 ↔ 覆盖页是「当前上拉/换下拉」。
- 恢复路径（菜单已在屏外）无上拉动画：从点击起等满 1280ms 再下拉。
- 页面实例首次访问后常驻（缓存），只有可见性切换。

```ts
// transition 时序骨架
const POP_UP_DUR = 770, POP_DOWN_DUR = 770, POP_UP_SOUND_MS = 1280
const POP_UP_GAP = POP_UP_SOUND_MS - POP_UP_DUR  // 510
async function goOverlay(next: string) {
  uiBusy.value = true
  sfx.play('popUp')
  await slideY(currentEl, 0, -2400, POP_UP_DUR, easeInQuad)   // 上拉
  await delay(POP_UP_GAP)                                     // 等音效
  parkAboveViewport(nextEl)                                   // 先停靠
  showPage(next)
  sfx.play('popDown')
  await slideY(nextEl, -viewportH, 0, POP_DOWN_DUR, easeOutQuad) // 下拉
  uiBusy.value = false
}
// easeInQuad: t => t*t   easeOutQuad: t => 1-(1-t)*(1-t)
```

ShellFrame 单独下落（页面内铁框从上方落入）用 **750ms ease-out-quad**（`dropDuration`）。

---

## 6. 光标

- `cursor.png`（128×99）：默认光标，缩到 32px 宽使用，**热点 (5, 0)**（指针尖端）。
- `cursor_green.png`（128×99）：绿色版，旧版仅用于预览弹窗缩放手柄（resize 变体），同样热点 (5, 0)。
- 旧版行为：app 级 override——**整个应用内所有区域（含弹窗、遮罩）都用 cursor.png**，可点击区不换手型（没有单独 hand 素材），离开窗口恢复系统光标。

CSS 实现与注意点：

```css
html, body, * { cursor: url('/assets/ui/misc/cursor_32.png') 1 0, auto; }
.war-resize-handle { cursor: url('/assets/ui/misc/cursor_green_32.png') 1 0, row-resize; }
```

- `url()` 后的 `5 0` 是热点坐标，**必须**跟一个回退关键字（`auto`/`pointer`）否则整条声明无效。
- 浏览器对 cursor 图片有 ≤128×128 限制，128×99 合规；但建议发布时额外生成一张 **32×25 的预缩放 PNG**，高分屏更锐利且免去浏览器缩放差异。
- CSS cursor 对 `*` 生效后无需像 Qt 那样逐元素"盖章"；Tauri webview 内不会掉回系统箭头。
- 文本输入框若保留 `cursor: text`，会显示系统 I 型光标——旧版未特殊处理，新版同样不处理即可。

---

## 7. 音效

三个事件（`AppSound.cpp`，Win32 PlaySound 单通道语义）：

| 事件名 | 文件（`public/assets/Sound/`） | 触发点 |
|---|---|---|
| `click` | `BigButtonClick.wav` | 每次 WarButton 触发 |
| `popUp` | `RightGlueScreenPopUp.wav` | 页面切换上拉开始（文件 1866ms，**可闻长度 1280ms**，尾部 ~590ms 低于 2% 峰值） |
| `popDown` | `RightGlueScreenPopDown.wav` | 页面切换/入场下拉开始 |

行为规则：
- **同名节流 200ms**：距上次播放不足 200ms 的同名请求直接丢弃。
- **播新停旧**：PlaySound 单通道——新播放前显式停掉旧声；同一 WAV 可立即重播。
- 启动时预载全部三个（避免首点延迟）。
- 音画时序：popUp 放满可闻长度后才允许下拉（见 §5.6 的 510ms gap）。

Web 实现建议：

```ts
// composables/useSfx.ts —— PlaySound 单通道 + 200ms 节流等价物
const files: Record<string, string> = {
  click: '/assets/Sound/BigButtonClick.wav',
  popUp: '/assets/Sound/RightGlueScreenPopUp.wav',
  popDown: '/assets/Sound/RightGlueScreenPopDown.wav',
}
let current: HTMLAudioElement | null = null
const lastPlay: Record<string, number> = {}
export function play(name: keyof typeof files) {
  const now = Date.now()
  if (now - (lastPlay[name] ?? 0) < 200) return   // 同名节流
  lastPlay[name] = now
  current?.pause()                                 // 播新停旧（单通道）
  current = new Audio(files[name])
  current.play().catch(() => {})                   // 首次用户手势前可能 reject
}
```

> 浏览器自动播放策略：首次用户交互前的 `play()` 会被拒——在第一个 click 事件里"解锁"一次即可，
> 此后所有页面切换音效都发生在手势之后，无问题。

---

## 8. 背景系统

### 8.1 配置格式（`background.json`，exe 旁读取）

```json
{
  "_comment": "Copy to background.json next to wardex.exe. type: image | model (video removed — no FFmpeg).",
  "type": "image",
  "source": "qrc:/qt/qml/WarDex/assets/background/LodolonFall.jpg"
}
```

旧版 main.cpp（95–139 行）解析规则，新版语义等价映射：

| 旧版 source 形式 | 旧版解析 | 新版（Tauri）等价 |
|---|---|---|
| 缺省 / 文件不存在 | qrc 内置 `LodolonFall.jpg`（1366×768） | 打包内 `/assets/background/LodolonFall.jpg` |
| `qrc:...` | 内置资源 | 打包内路径 |
| `file:...` / 绝对路径 | 本地文件 URL | Tauri asset protocol / `convertFileSrc` |
| 相对路径 | 相对 exe 所在目录 | 相对 exe 所在目录（Rust 侧解析后转 URL） |
| `type: "video"` | 已移除，强制回退 image + 默认图 | 同样回退 |

### 8.2 image 类型

- 静态图：`<img>` 全屏 `object-fit: cover`（Qt PreserveAspectCrop 等价）。
- **gif / webp 自动按动图处理**（Qt 里按扩展名切 AnimatedImage）；Web 下 `<img>` 天然支持，无需分支。
- 叠加层顺序：渐变底（`#0e2a22 → #0a1a16 60% → #04070a`）→ 背景图 → 压暗渐变（`transparent → #00000020 55% → #00000090`）。

```vue
<template>
  <div class="bg-base" />
  <img class="bg-img" :src="bgSource" />
  <div class="bg-dim" />
</template>
<style scoped>
.bg-base { position: fixed; inset: 0; background: linear-gradient(#0e2a22, #0a1a16 60%, #04070a); }
.bg-img  { position: fixed; inset: 0; width: 100%; height: 100%; object-fit: cover; }
.bg-dim  { position: fixed; inset: 0; background: linear-gradient(transparent, #00000020 55%, #00000090); }
</style>
```

### 8.3 model 类型（可后置）

旧版 BgModel.qml（Qt Quick3D RuntimeLoader）的 Three.js + GLTFLoader 等价实现，**标注为可后置项**：

- 场景背景色 `#0a1a16`；MSAA 高。
- 相机挂在 pivot 上绕原点公转：`from 0 → 360°`，**45000ms 无限循环**；
  相机位置 `(0, 60, 260)`，俯角 `x = −12°`，`clipFar = 10000`。
- 灯光：DirectionalLight `(-35°, −30°)` 强度 1.4；PointLight 位置 `(0, 120, 0)` 色 `#3d7bff` 强度 0.6。
- 用 GLTFLoader 加载 `source` 指向的 glTF/GLB。

---

## 9. 蓝色高亮贴图的加色混合

`GlueScreen-Button-KeyboardHighlight.png`（128×64，wc3_extracted）是 WC3 的蓝色辉光选中条，
用于：WarDropdown 选项 hover、WarMenu 高亮项、FolderBrowserDialog 选中行、RecentProjectsPanel hover（opacity 0.55）。

- Qt 里是普通 alpha 混合拉伸绘制；在 CSS 中直接 `<img>` 拉伸即可。
- **建议**加 `mix-blend-mode: screen`（Chromium 对 `plus-lighter` 支持较新，`screen` 更稳）让辉光在深色底上呈现加色感，比 Qt 原版更接近游戏内效果：

```css
.war-highlight {
  position: absolute; inset: 0 8px;             /* dropdown 行左右各缩 8 */
  background: url('/assets/wc3_extracted/ui/GlueScreen-Button-KeyboardHighlight.png')
              0 0 / 100% 100% no-repeat;
  mix-blend-mode: screen;                        /* 或 plus-lighter */
  pointer-events: none;
}
```

> RecentProjectsPanel 的 hover 高亮额外带 `opacity: 0.55`，其余场景满透明度。

---

## 10. 杂项全局件

- **Banner 通知条**（Main.qml）：顶部居中顶 margin 24，高 40，底 `#201018c0`，边 `#f2cf6b`，文字 `#f2cf6b` 14px SimSun，3.5s 自动消失，`z-index: 50`。
- **模态遮罩**：`#000000b0` 全屏。
- **文字描边**：Qt `Text.Outline` → CSS 四向 `text-shadow`（见 §1.3）；金色文字描边色 `#241500`，Georgia 标题描边色 `#000`。
- **禁用态**：WarButton `opacity: 0.38`；滚动条不可滚时隐藏轨道、箭头降至 `opacity: 0.35`。

---

## 实现检查清单

- [ ] CSS 变量色板按 §1.2 建立（注意 Qt `#AARRGGBB` → CSS `#RRGGBBAA` 换算已做好，直接抄 CSS 列）
- [ ] 字体三类（SimSun / Georgia / Consolas）与文字描边工具类
- [ ] `fontScale`（0.85~1.30，四档）+ `fs()` 工具注入全部阅读类文字
- [ ] `WarFrame` 九宫格组件，§2.2 十张贴图参数逐一核对（slice 顺序 T R B L、bubble_body 用 repeat）
- [ ] `WarIronFrame` 三层开孔组件（glass tuck 8 / contentGap 2 / extras），三张带孔贴图 hole 值核对
- [ ] WarButton 双皮肤（4.87 / 5.34）、宽驱动高、三态换图、禁用 0.38、标签字号公式
- [ ] WarDialog 标题板 12/14.5/76/35.5% 与按钮区 10/56/80/30% 分数定位
- [ ] WarDropdown（bar slice 12/46/12/29、右 margin 44、dropUp）与 WarMenu
- [ ] WarScrollBar 四贴图组装（thumb 九宫格 18/14）+ 内容装得下时隐藏轨道
- [ ] SteelPanel 双链（0.2305/0.658、链宽 0.186、tall/short 分数开孔）
- [ ] ChatBubble 槽外头像（64×58、用户镜像）、气泡最大宽 0.82、thinking/tool 折叠块配色
- [ ] 窗口 1280×720 / min 960×600、uiScale 公式、永久铁轨 58px z40 常驻
- [ ] 三段式切换动画 770/510/770ms + ShellFrame 750ms 下落 + uiBusy 门控
- [ ] 自定义光标（热点 5,0 + 回退关键字），弹窗/遮罩不掉系统箭头
- [ ] 三事件音效：200ms 同名节流、播新停旧、预载、popUp 1280ms 音画时序
- [ ] background.json 解析（image 默认 LodolonFall.jpg、gif/webp 动图、video 回退）
- [ ] （可后置）model 背景：Three.js GLTFLoader + 45s 环绕相机 + 双灯
- [ ] KeyboardHighlight 高亮条 `mix-blend-mode: screen`（RecentProjects 0.55 透明度）
- [ ] Banner / 模态遮罩 / 浮动条家族外观

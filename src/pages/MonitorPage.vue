<script setup lang="ts">
// 战场监控页（原型 tools/monitor-mockup/monitor.html 的正式移植）：
// 左 rail 项目列表（已部署置灰）+ 右 field 土地沙盘。项目部署为兵营（比例
// 坐标持久化在 prefs.monitorLayout），会话为步兵（2 行 × 4 列、最多 8 个）。
// field 内是 WORLD_SCALE 倍大的"世界层"（兵营/步兵/部署 ghost 都在世界层内，比例
// 坐标相对世界层），可视区 overflow hidden，按住鼠标中键拖动平移（clamp
// 到世界边界，边缘不露空白）；右下角小地图实时镜像世界层，左键按住拖动
// 可把视口中心跳到对应世界坐标。
// RTS 式交互：左键步兵 = 选中（图标绿色光环；待审批的例外，仍直接开审批弹窗），
// 双击步兵 = 开迷你会话窗；左键点地面空白 = 取消选中；有选中步兵时右键地面 =
// 命令其走过去（平滑移动）。右键兵营/步兵出内联菜单（新会话选 Agent +
// 权限模式 / 销毁二次确认 / 搁置恢复 / 重命名 / 删除）。
// Esc/快捷键照 SessionSelectPage 模式。
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import PageShell from '../components/PageShell.vue';
import WarButton from '../components/war/WarButton.vue';
import WarFrame from '../components/war/WarFrame.vue';
import WarScrollBar from '../components/war/WarScrollBar.vue';
import MonitorChatWin from '../components/monitor/MonitorChatWin.vue';
import MonitorPermDialog from '../components/monitor/MonitorPermDialog.vue';
import { useNavStore } from '../stores/nav';
import { usePrefsStore, DEFAULT_MONITOR_ZONES, type MonitorZone } from '../stores/prefs';
import { useSessionsStore, type SessionIndexRow } from '../stores/sessions';
import { useProjectsStore } from '../stores/projects';
import { useAgentsStore } from '../stores/agents';
import { useMonitorStore } from '../stores/monitor';
import { useChatStore } from '../stores/chat';
import { useUiStore } from '../stores/ui';
import { cmd } from '../lib/tauri';

const nav = useNavStore();
const prefs = usePrefsStore();
const sessions = useSessionsStore();
const projects = useProjectsStore();
const agents = useAgentsStore();
const monitor = useMonitorStore();
const chat = useChatStore();
const ui = useUiStore();

// ---------------------------------------------------------------------------
// 工作区动态：hover 项目的最近文件改动（collab 广播的可视化）
// ---------------------------------------------------------------------------

interface CollabChange {
  id: number;
  project_dir: string;
  session_id: string;
  path: string;
  summary: string;
  ts: number;
}

const hoverProj = ref('');
const collabChanges = ref<CollabChange[]>([]);
let collabTimer: ReturnType<typeof setInterval> | null = null;

async function loadCollab(): Promise<void> {
  if (!hoverProj.value) {
    collabChanges.value = [];
    return;
  }
  try {
    collabChanges.value = await cmd<CollabChange[]>(
      'collab_recent',
      { projectDir: hoverProj.value, limit: 30 },
      [],
    );
  } catch {
    /* 监控页非 Tauri 环境下静默 */
  }
}

function collabWho(id: string): string {
  return id.length > 6 ? id.slice(0, 6) : id;
}

function collabWhen(ts: number): string {
  const mins = Math.max(0, Math.floor((Date.now() - ts) / 60_000));
  return mins < 1 ? '刚刚' : `${mins} 分钟前`;
}

watch(hoverProj, loadCollab);

// ---------------------------------------------------------------------------
// 数据加载（页面常驻：onMounted 一次 + 每次切入重拉）
// ---------------------------------------------------------------------------

onMounted(() => {
  void projects.load();
  void agents.refresh();
  void monitor.initListeners();
  void monitor.refresh().then(restoreFootmanPositions);
  collabTimer = setInterval(() => void loadCollab(), 5000);
});

watch(
  () => nav.page,
  (p) => {
    if (p === 'monitor') {
      void projects.load();
      void monitor.refresh().then(restoreFootmanPositions);
      centerView(); // 每次进入战场监控，视角回到地图正中央
    }
  },
);

// ---------------------------------------------------------------------------
// 左 rail：项目列表（最近项目；已部署置灰）
// ---------------------------------------------------------------------------

/** 项目列表滚动容器（WarScrollBar 的 target）。 */
const projListEl = ref<HTMLElement | null>(null);

/** monitorLayout 的 key 是部署时的 canonical dir；大小写不敏感比较。 */
function layoutEntryOf(dir: string): { x: number; y: number } | undefined {
  const lower = dir.toLowerCase();
  for (const [k, v] of Object.entries(prefs.monitorLayout)) {
    if (k.toLowerCase() === lower) return v;
  }
  return undefined;
}

function isDeployed(dir: string): boolean {
  return layoutEntryOf(dir) !== undefined;
}

const deployedList = computed(() =>
  Object.entries(prefs.monitorLayout).map(([dir, pos]) => ({ dir, pos })),
);

// ---------------------------------------------------------------------------
// 右 field：尺寸跟踪（比例坐标 → px）、部署 ghost
// ---------------------------------------------------------------------------

const fieldEl = ref<HTMLElement | null>(null);
const fieldW = ref(800);
const fieldH = ref(600);
let resizeObs: ResizeObserver | null = null;

// ---- 世界层（WORLD_SCALE 倍大地图）+ 中键拖动平移 ----
// 兵营/步兵/ghost 渲染在世界层内，比例坐标相对世界层；可视区只露出一块，
// 世界层 transform: translate(panX, panY)，pan clamp 到 [field-world, 0]。

/** 世界相对可视区的倍数。越大可平移范围越大、滚轮放大细节越多；
 *  但地图贴图（map.webp 1024²）会被拉伸得越糊。1.6 → 1.8：平移范围
 *  从 0.6 倍 field 提到 ~1.3 倍，配合默认 zoom 1.3 看更大地图仍清晰。 */
const WORLD_SCALE = 1.8;
const worldW = computed(() => Math.round(fieldW.value * WORLD_SCALE));
const worldH = computed(() => Math.round(fieldH.value * WORLD_SCALE));
/** 滚轮缩放范围。屏幕坐标 = 世界坐标 × zoom + pan（origin 0 0）。 */
const MIN_ZOOM = 0.5;
const MAX_ZOOM = 2.5;
/** 初始缩放 >1：进页就看到更大的地图（视野 = 世界/zoom，拉伸反而更小更清晰）。 */
const zoom = ref(1.3);
const panX = ref(0);
const panY = ref(0);
/** 世界大于可视区才有得拖（此时显示"中键拖动地图"提示）。 */
const canPan = computed(() => worldW.value * zoom.value > fieldW.value || worldH.value * zoom.value > fieldH.value);

function clampPan(): void {
  const minX = Math.min(0, fieldW.value - worldW.value * zoom.value);
  const minY = Math.min(0, fieldH.value - worldH.value * zoom.value);
  panX.value = Math.max(minX, Math.min(0, panX.value));
  panY.value = Math.max(minY, Math.min(0, panY.value));
}

/** 滚轮缩放：以鼠标位置为锚点（鼠标下的世界点保持不动），clamp 范围。 */
function onFieldWheel(e: WheelEvent): void {
  const r = fieldEl.value?.getBoundingClientRect();
  if (!r) return;
  const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
  const nz = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom.value * factor));
  if (nz === zoom.value) return;
  const mx = e.clientX - r.left;
  const my = e.clientY - r.top;
  // 保持鼠标下世界点不动：W = (mx - panX)/zoom → newPan = mx - W*nz
  panX.value = mx - ((mx - panX.value) / zoom.value) * nz;
  panY.value = my - ((my - panY.value) / zoom.value) * nz;
  zoom.value = nz;
  clampPan();
}

/** 把视角对准世界正中央（进入页面时调用）。 */
function centerView(): void {
  panX.value = (fieldW.value - worldW.value) / 2;
  panY.value = (fieldH.value - worldH.value) / 2;
  clampPan();
}

/** 初次测量后把世界中心对到可视区中心（既有居中部署的兵营保持在视野内）。 */
let panInited = false;
function initPan(): void {
  if (panInited) return;
  panInited = true;
  centerView();
}

watch([fieldW, fieldH], clampPan);

let panning = false;
let panStartX = 0;
let panStartY = 0;
let panBaseX = 0;
let panBaseY = 0;

function onFieldMouseDown(e: MouseEvent): void {
  if (e.button === 1) {
    e.preventDefault(); // 防浏览器中键自动滚动图标
    panning = true;
    panStartX = e.clientX;
    panStartY = e.clientY;
    panBaseX = panX.value;
    panBaseY = panY.value;
    window.addEventListener('mousemove', onPanMove);
    window.addEventListener('mouseup', onPanUp);
    return;
  }
  if (e.button !== 0) return;
  // 左键从空白处起手 = 框选多选；命中兵营/步兵/小地图/右键菜单时不框选
  if (monitor.deploying || towerPlacing.value || !fieldEl.value) return;
  if ((e.target as HTMLElement).closest('.mon__footman, .mon__barracks, .mon__mm, .mon__pop')) return;
  const r = fieldEl.value.getBoundingClientRect();
  const sx = (e.clientX - r.left - panX.value) / zoom.value;
  const sy = (e.clientY - r.top - panY.value) / zoom.value;
  boxStart.value = { x: sx, y: sy };
  boxCur.value = { x: sx, y: sy };
  boxDrag.value = false;
  suppressClick = false;
  boxSelect.value = true;
  window.addEventListener('mousemove', onBoxMove);
  window.addEventListener('mouseup', onBoxUp);
}

function onPanMove(e: MouseEvent): void {
  if (!panning) return;
  panX.value = panBaseX + (e.clientX - panStartX);
  panY.value = panBaseY + (e.clientY - panStartY);
  clampPan();
}

function onPanUp(): void {
  if (!panning) return;
  panning = false;
  window.removeEventListener('mousemove', onPanMove);
  window.removeEventListener('mouseup', onPanUp);
}
onBeforeUnmount(onPanUp);

// ---- 框选（左键空白处拖出绿色选择框，多选步兵；点击与拖拽用阈值区分） ----

const boxSelect = ref(false);
const boxStart = ref({ x: 0, y: 0 });
const boxCur = ref({ x: 0, y: 0 });
/** 是否已拖出框（超过阈值）；ref 保证模板里的选择框及时渲染。 */
const boxDrag = ref(false);
/** 框选松手后，抑制随后的 click（否则会立刻取消选中）。 */
let suppressClick = false;

function onBoxMove(e: MouseEvent): void {
  if (!boxSelect.value || !fieldEl.value) return;
  const r = fieldEl.value.getBoundingClientRect();
  boxCur.value = {
    x: (e.clientX - r.left - panX.value) / zoom.value,
    y: (e.clientY - r.top - panY.value) / zoom.value,
  };
  if (!boxDrag.value && Math.hypot(boxCur.value.x - boxStart.value.x, boxCur.value.y - boxStart.value.y) > 5) {
    boxDrag.value = true;
  }
}

function onBoxUp(): void {
  window.removeEventListener('mousemove', onBoxMove);
  window.removeEventListener('mouseup', onBoxUp);
  if (!boxSelect.value) return;
  boxSelect.value = false;
  if (!boxDrag.value) return; // 纯点击，交给 onFieldClick 处理
  // 框选结束：命中步兵盒子（52×54）的加入选中集合
  const minX = Math.min(boxStart.value.x, boxCur.value.x);
  const maxX = Math.max(boxStart.value.x, boxCur.value.x);
  const minY = Math.min(boxStart.value.y, boxCur.value.y);
  const maxY = Math.max(boxStart.value.y, boxCur.value.y);
  selectedIds.clear();
  for (const b of deployedList.value) {
    slotsOf(b.dir).forEach((s, i) => {
      const p = footmanPos(s.id, b.dir, i);
      if (p.x + 52 >= minX && p.x <= maxX && p.y + 54 >= minY && p.y <= maxY) {
        selectedIds.add(s.id);
      }
    });
  }
  suppressClick = true;
}
onBeforeUnmount(() => {
  window.removeEventListener('mousemove', onBoxMove);
  window.removeEventListener('mouseup', onBoxUp);
});

// ---- 小地图（minimap）：兵营方块 / 步兵点 / 视口白框，左键按住拖动跳视口 ----

const mmEl = ref<HTMLElement | null>(null);
const mmW = 180;
/** 高度按世界层宽高比等比（world 两边同比放大，即 field 宽高比）。 */
const mmH = computed(() => Math.round((mmW * worldH.value) / worldW.value));
/** 世界 px → 小地图 px 缩放比。 */
const mmSx = computed(() => mmW / worldW.value);
const mmSy = computed(() => mmH.value / worldH.value);

/** 视口白框：pan 是响应式的，中键拖地图本体时天然实时跟随；缩放后可视世界 = field/zoom。 */
const mmVpStyle = computed(() => ({
  left: (-panX.value / worldW.value) * mmW + 'px',
  top: (-panY.value / worldH.value) * mmH.value + 'px',
  width: (fieldW.value / zoom.value / worldW.value) * mmW + 'px',
  height: (fieldH.value / zoom.value / worldH.value) * mmH.value + 'px',
}));

/** 小地图上一点 → 视口中心跳到对应世界坐标（复用 clampPan）。 */
function mmPanTo(e: MouseEvent): void {
  if (!mmEl.value) return;
  const r = mmEl.value.getBoundingClientRect();
  const wx = ((e.clientX - r.left) / r.width) * worldW.value;
  const wy = ((e.clientY - r.top) / r.height) * worldH.value;
  panX.value = -(wx - fieldW.value / 2);
  panY.value = -(wy - fieldH.value / 2);
  clampPan();
}

let mmDragging = false;

function onMmMouseDown(e: MouseEvent): void {
  if (e.button !== 0) return;
  e.preventDefault();
  mmDragging = true;
  mmPanTo(e);
  window.addEventListener('mousemove', onMmMove);
  window.addEventListener('mouseup', onMmUp);
}

function onMmMove(e: MouseEvent): void {
  if (!mmDragging) return;
  mmPanTo(e);
}

function onMmUp(): void {
  if (!mmDragging) return;
  mmDragging = false;
  window.removeEventListener('mousemove', onMmMove);
  window.removeEventListener('mouseup', onMmUp);
}
onBeforeUnmount(onMmUp);

onMounted(() => {
  if (fieldEl.value) {
    fieldW.value = fieldEl.value.clientWidth;
    fieldH.value = fieldEl.value.clientHeight;
    initPan();
    resizeObs = new ResizeObserver(() => {
      if (!fieldEl.value) return;
      fieldW.value = fieldEl.value.clientWidth;
      fieldH.value = fieldEl.value.clientHeight;
    });
    resizeObs.observe(fieldEl.value);
  }
});
onBeforeUnmount(() => resizeObs?.disconnect());

/** 兵营锚点 px（世界层内坐标；底座中心 = 原型 translate(-50%,-100%) 的落点）。 */
function anchorOf(dir: string): { x: number; y: number } {
  const pos = layoutEntryOf(dir) ?? { x: 0.5, y: 0.5 };
  return { x: pos.x * worldW.value, y: pos.y * worldH.value };
}

const ghost = ref<{ x: number; y: number } | null>(null);
/** 塔放置模式：'' = 未放置；'new' = 部署新塔；zoneId = 移动已有塔。 */
const towerPlacing = ref<string>('');
/** 塔部署面板（右上角工具栏「🗼 部署塔」打开，选择要部署/移动的塔）。 */
const towerPanelOpen = ref(false);

function onFieldMouseMove(e: MouseEvent): void {
  if ((!monitor.deploying && !towerPlacing.value) || !fieldEl.value) {
    ghost.value = null;
    return;
  }
  const r = fieldEl.value.getBoundingClientRect();
  // 世界坐标 = 屏幕坐标 - pan
  ghost.value = {
    x: (e.clientX - r.left - panX.value) / zoom.value,
    y: (e.clientY - r.top - panY.value) / zoom.value,
  };
}

function onFieldClick(e: MouseEvent): void {
  // 框选刚松手产生的 click 不当作"点空白取消选中"（选中已在框选结束时确定）
  if (suppressClick) {
    suppressClick = false;
    return;
  }
  if (!fieldEl.value) return;
  if (towerPlacing.value) {
    // 塔部署/移动：圆心 = 点击点（比例坐标，clamp 到世界内）
    const r = fieldEl.value.getBoundingClientRect();
    const x = Math.max(0.02, Math.min(0.98, (e.clientX - r.left - panX.value) / zoom.value / worldW.value));
    const y = Math.max(0.02, Math.min(0.98, (e.clientY - r.top - panY.value) / zoom.value / worldH.value));
    const id = towerPlacing.value;
    const wasDeployed = prefs.monitorZones.some((z) => z.id === id && z.deployed);
    ghost.value = null;
    prefs.monitorZones = prefs.monitorZones.map((z) => (z.id === id ? { ...z, x, y, deployed: true } : z));
    void prefs.setMonitorZones(prefs.monitorZones);
    const z = prefs.monitorZones.find((zz) => zz.id === id);
    toast(z ? (wasDeployed ? `🚚 ${z.name} 已移动` : `🗼 ${z.name} 已部署（右键塔可改名/换色/调半径/移动/拆除）`) : '');
    towerPlacing.value = '';
    return;
  }
  if (!monitor.deploying) {
    towerPanelOpen.value = false; // 点地面空白关塔面板
    selectedIds.clear(); // RTS：点地面空白取消选中
    closePop(); // 原型：点空白处关闭右键菜单
    return;
  }
  const dir = monitor.deploying;
  const r = fieldEl.value.getBoundingClientRect();
  const x = Math.max(0, Math.min(1, (e.clientX - r.left - panX.value) / zoom.value / worldW.value));
  const y = Math.max(0, Math.min(1, (e.clientY - r.top - panY.value) / zoom.value / worldH.value));
  ghost.value = null;
  void monitor.deploy(dir, x, y).then(() => {
    toast(`已部署「${projects.displayName(dir)}」兵营`);
  });
}

function onFieldContextMenu(e: MouseEvent): void {
  e.preventDefault();
  if (monitor.deploying || towerPlacing.value) {
    monitor.cancelDeploy();
    towerPlacing.value = '';
    ghost.value = null;
    toast('已取消部署');
    closePop();
    return;
  }
  if (!fieldEl.value) return;
  const r = fieldEl.value.getBoundingClientRect();
  // 世界坐标 = 屏幕坐标 - pan（同 ghost / 部署落点换算），clamp 到世界边界
  const wx = Math.max(0, Math.min(worldW.value, (e.clientX - r.left - panX.value) / zoom.value));
  const wy = Math.max(0, Math.min(worldH.value, (e.clientY - r.top - panY.value) / zoom.value));
  // 命中圆内区域；再兜底查隐藏区域的塔本体（避免隐藏后无法右键恢复的死锁）
  let zone = zoneAt(wx, wy);
  if (!zone) zone = prefs.monitorZones.find((z) => z.enabled === false && pointInTower(z, wx, wy));
  if (zone) {
    if (selectedIds.size > 0) {
      // 右键点哪走哪：区域只是地形，落点以右键点为基准（批量散开并 clamp 到圆内）
      addMoveMarker(wx, wy);
      assignToZone([...selectedIds], zone, wx, wy);
    } else if (pointInTower(zone, wx, wy)) {
      openZoneMenu(zone, e); // 只有右键塔本体才出菜单
    } else {
      closePop(); // 圆内但没点中塔本体 → 当普通地面，不弹菜单
    }
    return;
  }
  // RTS：有选中步兵时右键地面 = 命令其走过去（不弹菜单，目标 = 右键的世界坐标）
  if (selectedIds.size > 0) {
    addMoveMarker(wx, wy);
    // 多选批量移动：沿对角线小扇形散开，避免全部叠在同一点
    const ids = [...selectedIds];
    ids.forEach((id, k) => {
      const off = k - (ids.length - 1) / 2;
      const tx = wx + off * 30;
      const ty = wy + off * 30;
      commandMove(id, tx, ty);
      // 地面移动也持久化（重启还原）；标记由每个步兵自己的落点判定（落点在
      // 区域内 → 打该区标，区域外 → 自动清除），与右键点击点无关
      persistFootmanTarget(id, tx, ty, priorityAt(tx, ty));
    });
    closePop();
    return;
  }
  closePop();
}

function startDeploy(dir: string): void {
  monitor.startDeploy(dir);
  toast(`部署「${projects.displayName(dir)}」：在土地上左键点击放置兵营，右键取消`);
}

async function raze(dir: string): Promise<void> {
  await monitor.raze(dir);
  toast(`已销毁「${projects.displayName(dir)}」兵营（会话保留，重新部署后为空地）`);
}

// ---------------------------------------------------------------------------
// 步兵（会话）：2×4 栏位、状态边框、金叹号、NEW
// ---------------------------------------------------------------------------

const FW = 62; // 步兵格子步进（原型 slotPos）
const FH = 56;

function slotPos(dir: string, i: number): { x: number; y: number } {
  const b = anchorOf(dir);
  const col = i % 4;
  const row = Math.floor(i / 4);
  // 兵营左下角起，每行 4 个向右，两行；row0 在前（贴近底座），row1 在其后
  return { x: b.x - 95 + col * FW, y: b.y - 38 + (1 - row) * FH };
}

/** 每个兵营的栏位会话缓存：只在 sessions/布局/部署过渡标记变化时重算，
 *  行走动画每帧触发的重渲染直接复用（否则每个兵营每次渲染都 filter+sort 全量会话）。
 *  部署过渡期（落盘+搁置重拉中）返回空，避免旧会话闪一下再消失。 */
const slotsByDir = computed(() => {
  const m: Record<string, SessionIndexRow[]> = {};
  for (const b of deployedList.value) {
    m[b.dir] = monitor.deploySettling.includes(b.dir)
      ? []
      : monitor.sessionsOf(b.dir).slice(0, 8);
  }
  return m;
});

function slotsOf(dir: string): SessionIndexRow[] {
  return slotsByDir.value[dir] ?? [];
}

function isPermPending(id: string): boolean {
  return monitor.isPermPending(id);
}

function isBusy(id: string): boolean {
  return sessions.runtimeStates[id]?.busy === true;
}

function hasUnread(id: string): boolean {
  return sessions.unreadIds.includes(id) && !monitor.readLocal.includes(id);
}

function footmanClass(s: SessionIndexRow): string {
  if (isPermPending(s.id)) return 'perm';
  if (isBusy(s.id)) return 'run';
  return '';
}

/** 悬停气泡的状态行。 */
function footmanStatus(s: SessionIndexRow): string {
  if (isPermPending(s.id)) return '⚠ 等待权限审批';
  if (isBusy(s.id)) return '⚙ 正在运行…';
  return hasUnread(s.id) ? '💬 已完成 · 有新回复' : '💤 空闲（已完成）';
}

function trunc(s: string, max: number): string {
  const chars = [...s];
  return chars.length <= max ? s : chars.slice(0, max - 1).join('') + '…';
}

function barracksNeed(dir: string): boolean {
  return slotsOf(dir).some((s) => isPermPending(s.id));
}

/** 左键 = 选中（例外：等审批的仍直接开审批弹窗）；双击 = 开迷你会话窗。 */
function onFootmanClick(s: SessionIndexRow): void {
  if (isPermPending(s.id)) {
    void monitor.openPermDialog(s.id);
    return;
  }
  selectedIds.clear();
  selectedIds.add(s.id);
}

function onFootmanDblclick(s: SessionIndexRow): void {
  if (isPermPending(s.id)) return; // 等审批的走左键审批，不在双击里开窗
  monitor.openChatWin(s.id);
}

// ---------------------------------------------------------------------------
// RTS：选中 + 右键移动（位置不持久化，重启回栏位）
// ---------------------------------------------------------------------------

/** 当前选中的步兵（会话 id 集合；多选）。 */
const selectedIds = reactive(new Set<string>());

/** 世界坐标覆盖层：被命令移动过的步兵不再跟随栏位，left/top 优先取这里。
 *  存**比例坐标**（相对世界层宽高），渲染/判定时再乘回 px——窗口缩放时
 *  塔/兵营按比例跟随 worldW/H，步兵也必须同源换算，否则相对位置漂移。 */
const posOverride = ref<Record<string, { x: number; y: number }>>({});

/** 移动指令落点标记：右键下令时在基准点放一个扩散光圈，动画结束自动移除。 */
const moveMarkers = ref<{ id: number; x: number; y: number }[]>([]);
let moveMarkerSeq = 0;
function addMoveMarker(wx: number, wy: number): void {
  const m = { id: ++moveMarkerSeq, x: wx, y: wy };
  moveMarkers.value.push(m);
  setTimeout(() => {
    moveMarkers.value = moveMarkers.value.filter((v) => v.id !== m.id);
  }, 650);
}

/** 渲染用位置：覆盖值（比例→px）优先，否则回退栏位计算值（小地图同源）。 */
function footmanPos(id: string, dir: string, i: number): { x: number; y: number } {
  const o = posOverride.value[id];
  return o ? { x: o.x * worldW.value, y: o.y * worldH.value } : slotPos(dir, i);
}

const MOVE_SPEED = 170; // 世界像素/秒

/** 正在移动的步兵目标（世界坐标，非响应式；rAF 循环消费）。 */
const moveTargets = new Map<string, { tx: number; ty: number }>();
let rafId = 0;
let lastTs = 0;

/** 会话是否还在某个兵营栏位里（移动中被删除/搁置时停掉循环）。 */
function sessionAlive(id: string): boolean {
  return deployedList.value.some((b) => slotsOf(b.dir).some((s) => s.id === id));
}

/** 右键地面下令：目标点 wx/wy 是图标中心落点，换算成盒子左上角存入覆盖层（比例）。 */
function commandMove(id: string, wx: number, wy: number): void {
  if (!sessionAlive(id)) return;
  if (!posOverride.value[id]) {
    const cur = footmanPosCurrent(id); // px 左上角
    posOverride.value[id] = { x: cur.x / worldW.value, y: cur.y / worldH.value };
  }
  moveTargets.set(id, { tx: wx - 26, ty: wy - 27 }); // 盒子 52×54（px 目标）
  if (!rafId) {
    lastTs = performance.now();
    rafId = requestAnimationFrame(tick);
  }
}

/** 当前实际位置（px）：覆盖值（比例→px）优先，否则栏位。 */
function footmanPosCurrent(id: string): { x: number; y: number } {
  const o = posOverride.value[id];
  if (o) return { x: o.x * worldW.value, y: o.y * worldH.value };
  for (const b of deployedList.value) {
    const i = slotsOf(b.dir).findIndex((s) => s.id === id);
    if (i >= 0) return slotPos(b.dir, i);
  }
  return { x: 0, y: 0 };
}

/** 统一 rAF 循环：处理所有正在走的步兵，全到位后自停。
 *  移动计算在 px 空间（速度 px/s），结果写回比例覆盖层。 */
function tick(ts: number): void {
  const dt = Math.min(0.05, (ts - lastTs) / 1000);
  lastTs = ts;
  for (const [id, t] of moveTargets) {
    const p = posOverride.value[id];
    if (!p || !sessionAlive(id)) {
      moveTargets.delete(id);
      continue;
    }
    const px = { x: p.x * worldW.value, y: p.y * worldH.value };
    const dx = t.tx - px.x;
    const dy = t.ty - px.y;
    const dist = Math.hypot(dx, dy);
    const step = MOVE_SPEED * dt;
    if (dist <= step) {
      p.x = t.tx / worldW.value;
      p.y = t.ty / worldH.value;
      moveTargets.delete(id);
    } else {
      p.x = (px.x + (dx / dist) * step) / worldW.value;
      p.y = (px.y + (dy / dist) * step) / worldH.value;
    }
  }
  rafId = moveTargets.size > 0 ? requestAnimationFrame(tick) : 0;
}

onBeforeUnmount(() => {
  if (rafId) cancelAnimationFrame(rafId);
});

// ---------------------------------------------------------------------------
// 右键弹出菜单（原型 .pop，含内联面板；一个实例，点别处关闭）
// ---------------------------------------------------------------------------

type Pop =
  | { kind: 'building'; dir: string; x: number; y: number }
  | { kind: 'footman'; session: SessionIndexRow; x: number; y: number }
  | { kind: 'zone'; zoneId: string; x: number; y: number };

const pop = ref<Pop | null>(null);
// 建筑菜单子状态
const newOpen = ref(false);
const pickAgent = ref('');
const pickMode = ref('default');
const razeArmed = ref(false);
const shelvedQuery = ref('');

const PERM_MODES: [string, string][] = [
  ['default', '默认'],
  ['plan', '计划'],
  ['auto', '自动'],
  ['yolo', '放任'],
];

// ---------------------------------------------------------------------------
// 优先级区域（艾森豪威尔四宫格）：区域是优先级标记的可视化。步兵被归位到
// 某区域 = 打上该区域序号（1..4）的优先级标记 + 位置持久化（重启还原）。
// 标记是数据、位置是表象：步兵移出区域不丢标记（可单独「清除优先级标记」）。
// ---------------------------------------------------------------------------

/** 换色面板预设（含四宫格默认色）。 */
const ZONE_COLORS = ['#e14b43', '#e8983d', '#d4b94a', '#57a55f', '#5b8de8', '#9a6bc4'];

const zoneRenaming = ref(false);
const zoneNameText = ref('');
/** 步兵菜单「送往区域」子面板开关。 */
const zonePickOpen = ref(false);

const visibleZones = computed(() => prefs.monitorZones.filter((z) => z.deployed && z.enabled !== false));
const hiddenZones = computed(() => prefs.monitorZones.filter((z) => z.deployed && z.enabled === false));

/** 塔放置/移动时跟随鼠标的预显圆圈（新塔用默认半径，移动用该塔半径；颜色跟随塔色）。 */
const ghostZoneStyle = computed(() => {
  const g = ghost.value;
  if (!g) return {};
  const z = prefs.monitorZones.find((zz) => zz.id === towerPlacing.value);
  const r = z?.r ?? 0.09;
  const color = z?.color ?? '#7ec97a';
  const rPx = r * worldW.value;
  return {
    left: g.x - rPx + 'px',
    top: g.y - rPx + 'px',
    width: rPx * 2 + 'px',
    height: rPx * 2 + 'px',
    borderColor: color,
    background: color + '18',
  };
});

/** 塔 ghost 的光晕颜色（跟随塔色；兵营 ghost 保持默认绿光）。 */
const ghostTowerGlow = computed(() => {
  const z = prefs.monitorZones.find((zz) => zz.id === towerPlacing.value);
  return `drop-shadow(0 0 12px ${(z?.color ?? '#7ec97a') + '88'})`;
});

/** 区域圆心世界 px + 半径 px（r 比例 × 世界宽 → 屏幕正圆）。 */
function zoneCircle(z: MonitorZone): { cx: number; cy: number; r: number } {
  return { cx: z.x * worldW.value, cy: z.y * worldH.value, r: z.r * worldW.value };
}

function pointInZone(z: MonitorZone, wx: number, wy: number): boolean {
  const c = zoneCircle(z);
  const dx = wx - c.cx;
  const dy = wy - c.cy;
  return dx * dx + dy * dy <= c.r * c.r;
}

/** 右键点是否命中塔本体（区域图标，约 34px 半径）——只有塔本体才出菜单。 */
function pointInTower(z: MonitorZone, wx: number, wy: number): boolean {
  const c = zoneCircle(z);
  const dx = wx - c.cx;
  const dy = wy - c.cy;
  const hr = 34;
  return dx * dx + dy * dy <= hr * hr;
}

/** 右键点落在哪个启用区域（总开关关闭时一律 undefined → 当普通地面）。 */
function zoneAt(wx: number, wy: number): MonitorZone | undefined {
  if (!prefs.monitorZonesOn) return undefined;
  return visibleZones.value.find((z) => pointInZone(z, wx, wy));
}

/** 落点命中启用区域 → 该区域序号（1..N）；否则 0（无标记）。
 * 颜色/标记只由步兵自身落点决定，与右键点击点无关。 */
function priorityAt(wx: number, wy: number): number {
  const z = zoneAt(wx, wy);
  return z ? prefs.monitorZones.findIndex((zz) => zz.id === z.id) + 1 : 0;
}

// ---- 步兵优先级标记 - -

function footmanPriority(id: string): number {
  return prefs.monitorFootmen[id]?.priority ?? 0;
}

/** priority 1..N → 塔列表中的第 p 座（与 priorityAt 的 findIndex+1 同源；
 * 颜色/名字直接取该塔本身，保证左上角标记始终跟随塔色，与部署状态无关）。 */
function zoneOfPriority(p: number): MonitorZone | undefined {
  return prefs.monitorZones[p - 1];
}

function priorityName(id: string): string {
  return zoneOfPriority(footmanPriority(id))?.name ?? '';
}

function priorityColor(p: number): string {
  return zoneOfPriority(p)?.color ?? ZONE_COLORS[p - 1] ?? '#8a93a5';
}

/** 已被归位（有持久化位置）且站在该区域圆内的步兵 id。 */
function zoneFootmenIn(zoneId: string): string[] {
  const zone = prefs.monitorZones.find((z) => z.id === zoneId);
  if (!zone) return [];
  const out: string[] = [];
  for (const b of deployedList.value) {
    for (const s of slotsOf(b.dir)) {
      const o = posOverride.value[s.id];
      if (!o) continue; // 还在兵营栏位里的不算归位
      if (pointInZone(zone, o.x * worldW.value + 26, o.y * worldH.value + 27)) out.push(s.id);
    }
  }
  return out;
}

function zoneCount(zoneId: string): number {
  return zoneFootmenIn(zoneId).length;
}

// ---- 位置持久化（重启后还原） ----

/** 移动指令下达时按目标点持久化：rx/ry 是世界比例坐标，priority 由调用方
 * 用 priorityAt 按每个步兵自己的落点判定（落点命中区域 → 该区序号，否则 0）。 */
function persistFootmanTarget(id: string, wx: number, wy: number, priority?: number): void {
  if (!sessionAlive(id)) return;
  const prev = prefs.monitorFootmen[id] ?? {};
  void prefs.setMonitorFootman(id, {
    rx: Math.round((wx / worldW.value) * 10000) / 10000,
    ry: Math.round((wy / worldH.value) * 10000) / 10000,
    priority: priority ?? prev.priority ?? 0,
  });
}

/** 清除步兵的持久化位置（回栏位）：删除会话 / 清空区域时用。 */
function clearFootmanPos(id: string): void {
  delete posOverride.value[id];
  moveTargets.delete(id);
  void prefs.setMonitorFootman(id, null);
}

/** 重启还原：持久化存的本来就是比例坐标，直接进覆盖层（未搁置会话；
 * 搁置中的在恢复时被清掉位置 → 回栏位）。 */
function restoreFootmanPositions(): void {
  const live = new Set(sessions.all.filter((s) => !s.shelved).map((s) => s.id));
  for (const [id, e] of Object.entries(prefs.monitorFootmen)) {
    if (live.has(id) && e.rx != null && e.ry != null) {
      posOverride.value[id] = { x: e.rx, y: e.ry };
    }
  }
}

/** 框选多选步兵 → 归位到区域（圆内散开站立 + 按各自落点打标记）。 */
function assignToZone(ids: string[], zone: MonitorZone, bx?: number, by?: number): void {
  const c = zoneCircle(zone);
  // 基准点：右键区域 = 右键点；菜单「送往区域」无右键点 → 圆心
  const bx0 = bx ?? c.cx;
  const by0 = by ?? c.cy;
  // 落点 clamp 到圆内（避开塔图标占的中间一块）
  const inner = Math.max(26, c.r - 34);
  ids.forEach((id, k) => {
    if (!sessionAlive(id)) return;
    const off = k - (ids.length - 1) / 2;
    const cx = bx0 + off * 44;
    const cy = by0 - Math.floor(k / 4) * 34;
    const dx = cx - c.cx;
    const dy = cy - c.cy;
    const d = Math.hypot(dx, dy);
    const wx = d > inner ? c.cx + (dx / d) * inner : cx;
    const wy = d > inner ? c.cy + (dy / d) * inner : cy;
    commandMove(id, wx, wy);
    // 标记由每个步兵自己的落点判定（散开压线到圆外 → 不打标）
    persistFootmanTarget(id, wx, wy, priorityAt(wx, wy));
  });
  // 不清选中：与右键地面移动一致（RTS 惯例），步兵进塔后保持选中，
  // 方便紧接着再下指令/再选中。
  toast(`已归位 ${ids.length} 个会话到「${zone.name}」`);
}

function toggleZones(): void {
  void prefs.setMonitorZonesOn(!prefs.monitorZonesOn);
}

/** 部署塔按钮：打开/关闭塔面板（选择要部署/移动的塔）。 */
function startTowerDeploy(): void {
  closePop();
  towerPanelOpen.value = !towerPanelOpen.value;
  if (towerPanelOpen.value) towerPlacing.value = '';
}

/** 面板选中某塔 → 进入部署/移动模式（ghost 跟随，左键放置）。 */
function pickTower(id: string): void {
  const z = prefs.monitorZones.find((zz) => zz.id === id);
  if (!z) return;
  towerPanelOpen.value = false;
  towerPlacing.value = id;
  toast(z.deployed ? `🚚 移动「${z.name}」：左键点击新位置，右键取消` : `🗼 部署「${z.name}」：左键点击放置，右键取消`);
}

/** 面板底部：添加一座新塔（追加到列表末尾并进入部署模式）。 */
function addNewTower(): void {
  towerPanelOpen.value = false;
  const id = `tz${Date.now().toString(36)}`;
  prefs.monitorZones = [
    ...prefs.monitorZones,
    {
      id,
      name: `区域 ${prefs.monitorZones.length + 1}`,
      color: ZONE_COLORS[prefs.monitorZones.length % ZONE_COLORS.length],
      x: 0.5,
      y: 0.5,
      r: 0.09,
      enabled: true,
      deployed: false,
    },
  ];
  void prefs.setMonitorZones(prefs.monitorZones);
  towerPlacing.value = id;
  toast('🗼 部署新塔：左键点击放置，右键取消');
}

/** 拆除此塔：回到未部署状态（可在面板中重新部署）。 */
function removeTower(id: string): void {
  prefs.monitorZones = prefs.monitorZones.map((z) => (z.id === id ? { ...z, deployed: false } : z));
  void prefs.setMonitorZones(prefs.monitorZones);
  closePop();
  toast('🗼 塔已拆除，可在右上角「部署塔」重新部署');
}

/** 移动已有塔（放置模式下左键点新位置）。 */
function startTowerMove(id: string): void {
  closePop();
  towerPlacing.value = id;
  toast('🗼 点击新位置移动塔，右键取消');
}

/** 调整塔区域半径（比例步长 0.015 ≈ 19 世界像素）。 */
function adjustZoneRadius(id: string, delta: number): void {
  prefs.monitorZones = prefs.monitorZones.map((z) =>
    z.id === id ? { ...z, r: Math.max(0.03, Math.min(0.35, Math.round((z.r + delta) * 1000) / 1000)) } : z,
  );
  void prefs.setMonitorZones(prefs.monitorZones);
}

/** 复位默认布局：整表写回默认四宫格（保持 id 顺序 → 已有优先级标记不乱）。 */
function resetZones(): void {
  closePop();
  const defs = DEFAULT_MONITOR_ZONES.map((z) => ({ ...z }));
  void prefs.setMonitorZones(defs);
  toast('已复位默认区域布局');
}

function commitZoneRename(): void {
  const p = pop.value;
  const id = p?.kind === 'zone' ? p.zoneId : '';
  const name = zoneNameText.value.trim().slice(0, 12);
  zoneRenaming.value = false;
  if (!id || !name) return;
  prefs.monitorZones = prefs.monitorZones.map((z) => (z.id === id ? { ...z, name } : z));
  void prefs.setMonitorZones(prefs.monitorZones);
  toast(`区域已重命名为「${name}」`);
}

function recolorZone(id: string, color: string): void {
  prefs.monitorZones = prefs.monitorZones.map((z) => (z.id === id ? { ...z, color } : z));
  void prefs.setMonitorZones(prefs.monitorZones);
}

function toggleZoneEnabled(id: string): void {
  prefs.monitorZones = prefs.monitorZones.map((z) =>
    z.id === id ? { ...z, enabled: z.enabled === false } : z,
  );
  void prefs.setMonitorZones(prefs.monitorZones);
}

/** 清空本区：区内归位的步兵回各自兵营栏位，优先级标记一并清除。 */
function clearZone(zoneId: string): void {
  const zone = prefs.monitorZones.find((z) => z.id === zoneId);
  if (!zone) return;
  const ids = zoneFootmenIn(zoneId);
  for (const id of ids) clearFootmanPos(id);
  closePop();
  toast(ids.length > 0 ? `已清空「${zone.name}」，${ids.length} 个会话回栏位` : `「${zone.name}」内没有归位步兵`);
}

/** 步兵菜单「送往区域」：单步兵（或批量框选后对某个步兵操作？不——菜单只
 * 作用于右键的那个步兵；批量走框选 + 右键区域）。 */
function onZonePick(id: string, z: MonitorZone): void {
  closePop();
  assignToZone([id], z);
}

const usableAgents = computed(() => agents.agents.filter((a) => a.enabled));

const filteredShelved = computed(() => {
  if (pop.value?.kind !== 'building') return [];
  const q = shelvedQuery.value.trim().toLowerCase();
  const all = monitor.shelvedOf(pop.value.dir);
  if (!q) return all;
  return all.filter((s) => (s.title ?? '').toLowerCase().includes(q));
});

function popPos(e: MouseEvent): { x: number; y: number } {
  const r = fieldEl.value?.getBoundingClientRect();
  const x = r ? e.clientX - r.left : e.clientX;
  const y = r ? e.clientY - r.top : e.clientY;
  return {
    x: Math.max(0, Math.min(x, fieldW.value - 360)),
    y: Math.max(0, Math.min(y, fieldH.value - 320)),
  };
}

function openBuildingMenu(dir: string, e: MouseEvent): void {
  e.preventDefault();
  e.stopPropagation();
  const p = popPos(e);
  pop.value = { kind: 'building', dir, x: p.x, y: p.y };
  newOpen.value = false;
  razeArmed.value = false;
  shelvedQuery.value = '';
  pickAgent.value =
    agents.defaultAgentId && usableAgents.value.some((a) => a.id === agents.defaultAgentId)
      ? agents.defaultAgentId
      : (usableAgents.value[0]?.id ?? '');
  pickMode.value = 'default';
}

const zoneMenuZone = computed(() => {
  const p = pop.value;
  return p?.kind === 'zone' ? (prefs.monitorZones.find((z) => z.id === p.zoneId) ?? null) : null;
});

function openZoneMenu(zone: MonitorZone, e: MouseEvent): void {
  e.preventDefault();
  e.stopPropagation();
  const p = popPos(e);
  pop.value = { kind: 'zone', zoneId: zone.id, x: p.x, y: p.y };
  zoneRenaming.value = false;
  zoneNameText.value = zone.name;
}

function openFootmanMenu(s: SessionIndexRow, e: MouseEvent): void {
  e.preventDefault();
  e.stopPropagation();
  const p = popPos(e);
  pop.value = { kind: 'footman', session: s, x: p.x, y: p.y };
  zonePickOpen.value = false;
}

function closePop(): void {
  pop.value = null;
}

async function createSession(dir: string): Promise<void> {
  closePop();
  const id = await monitor.newSession(dir, pickAgent.value, pickMode.value);
  if (!id) {
    ui.showBanner('无法创建会话（请检查 Agent 配置）');
    return;
  }
  const ag = agents.byId(pickAgent.value);
  const modeLabel = PERM_MODES.find(([v]) => v === pickMode.value)?.[1] ?? pickMode.value;
  toast(`已在「${projects.displayName(dir)}」创建会话（${ag?.name ?? '默认 Agent'} · ${modeLabel}）`);
}

function onRazeClick(dir: string): void {
  if (!razeArmed.value) {
    razeArmed.value = true;
    return;
  }
  closePop();
  void raze(dir);
}

async function restoreShelved(dir: string, id: string): Promise<void> {
  if (monitor.sessionsOf(dir).length >= 8) {
    toast('栏位已满（8 个），请先搁置其他会话');
    return;
  }
  closePop();
  // 恢复后回栏位：清掉持久化位置（优先级标记保留）
  const prev = prefs.monitorFootmen[id];
  delete posOverride.value[id];
  moveTargets.delete(id);
  if (prev?.priority) void prefs.setMonitorFootman(id, { priority: prev.priority });
  else void prefs.setMonitorFootman(id, null);
  await monitor.setShelved(id, false);
  toast('已恢复会话');
}

// ---- 步兵菜单动作 ----

const entering = ref(false);

async function enterSession(id: string): Promise<void> {
  if (nav.phase !== 'idle' || entering.value) return;
  entering.value = true;
  try {
    const ok = await chat.openSession(id);
    if (!ok) {
      ui.showBanner('无法打开会话');
      return;
    }
    await nav.goOverlay('chat');
  } finally {
    entering.value = false;
  }
}

function onFootmanAct(act: string, s: SessionIndexRow): void {
  closePop();
  switch (act) {
    case 'enter':
      void enterSession(s.id);
      break;
    case 'detail':
      monitor.openChatWin(s.id);
      break;
    case 'rename':
      startRename(s);
      break;
    case 'shelve':
      void monitor.setShelved(s.id, true).then(() => toast(`已搁置「${s.title}」（进程已关闭，会话保留）`));
      break;
    case 'clearmark':
      // 清除优先级标记：位置（如有）保留，priority 归 0
      {
        const cur = prefs.monitorFootmen[s.id];
        void prefs.setMonitorFootman(
          s.id,
          cur && (cur.rx != null || cur.ry != null) ? { rx: cur.rx, ry: cur.ry, priority: 0 } : null,
        );
      }
      break;
    case 'del':
      clearFootmanPos(s.id);
      void monitor.remove(s.id).then(() => toast(`已删除「${s.title}」`));
      break;
  }
}

// ---------------------------------------------------------------------------
// 步兵内联重命名（原型 startFootmanRename）
// ---------------------------------------------------------------------------

const renamingId = ref('');
const renameText = ref('');

function startRename(s: SessionIndexRow): void {
  renamingId.value = s.id;
  renameText.value = s.title;
}

async function commitRename(): Promise<void> {
  const id = renamingId.value;
  const title = renameText.value.trim().slice(0, 24);
  renamingId.value = '';
  if (!id || !title) return;
  const cur = sessions.all.find((s) => s.id === id);
  if (cur && cur.title === title) return;
  await monitor.rename(id, title);
  toast(`已重命名为「${title}」`);
}

// ---------------------------------------------------------------------------
// toast（原型 #toast）
// ---------------------------------------------------------------------------

const toastText = ref('');
let toastTimer: ReturnType<typeof setTimeout> | null = null;

function toast(msg: string): void {
  toastText.value = msg;
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (toastText.value = ''), 2200);
}
onBeforeUnmount(() => {
  if (toastTimer) clearTimeout(toastTimer);
  if (collabTimer) clearInterval(collabTimer);
});

// ---------------------------------------------------------------------------
// Esc / 快捷键（SessionSelectPage 模式；优先级：小窗 → 审批 → 部署 → 菜单 → 返回）
// ---------------------------------------------------------------------------

function onPageKey(e: KeyboardEvent): void {
  if (nav.page !== 'monitor') return;
  // Delete/Backspace：批量删除选中的步兵（输入框聚焦时让位给文本编辑）
  if (e.key === 'Delete' || e.key === 'Backspace') {
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA')) return;
    if (selectedIds.size > 0) {
      e.preventDefault();
      const ids = [...selectedIds];
      selectedIds.clear();
      for (const id of ids) {
        clearFootmanPos(id);
        void monitor.remove(id).then(() => toast('已删除选中的会话'));
      }
    }
    return;
  }
  if (e.key !== 'Escape') return;
  if (renamingId.value) return; // 内联输入自己处理 Esc
  if (monitor.chatWins.length > 0) {
    monitor.closeTopChatWin(); // 关最上层小窗
    return;
  }
  if (monitor.permDialogSessionId) {
    monitor.closePermDialog();
    return;
  }
  if (towerPanelOpen.value) {
    towerPanelOpen.value = false; // 先关塔面板
    return;
  }
  if (monitor.deploying || towerPlacing.value) {
    monitor.cancelDeploy();
    towerPlacing.value = '';
    ghost.value = null;
    return;
  }
  if (pop.value) {
    closePop();
    return;
  }
  if (selectedIds.size > 0) {
    selectedIds.clear(); // RTS：Esc 取消选中
    return;
  }
  void nav.goMain();
}
onMounted(() => window.addEventListener('keydown', onPageKey));
onBeforeUnmount(() => window.removeEventListener('keydown', onPageKey));

const pageKeysOn = computed(() => nav.page === 'monitor');

const permDialogRequest = computed(() =>
  monitor.permDialogSessionId ? (monitor.permPayloads[monitor.permDialogSessionId] ?? null) : null,
);
</script>

<template>
  <!-- embed=0：监控页左右内容不嵌到窗框铁轨下（不透明 rail 被铁轨压住会裁字） -->
  <PageShell :embed="0">
    <div class="mon">
      <!-- 左 rail：项目列表（frame_popup_small 框，照 RecentProjectsPanel） -->
      <div class="mon__rail">
        <WarFrame
          class="mon__rail-frame"
          src="/assets/ui/frames/frame_popup_small.png"
          :slice="[44, 50, 45, 50]"
          :inset="[23, 33, 24, 31]"
          fill
        >
          <div class="mon__rail-col">
            <div class="mon__rail-head" :style="{ fontSize: prefs.fs(18) + 'px' }">
              项目列表
              <span v-if="monitor.permPendingCount > 0" class="mon__perm-count" :style="{ fontSize: prefs.fs(12) + 'px' }">
                ⚠ {{ monitor.permPendingCount }} 个会话等待审批
              </span>
            </div>
            <div class="mon__rail-hint" :style="{ fontSize: prefs.fs(12) + 'px' }">
              点击未部署的项目，在右侧土地上部署兵营
            </div>
            <div class="mon__proj-wrap">
              <div ref="projListEl" class="mon__proj-list">
                <div
                  v-for="p in projects.recent"
                  :key="p.path"
                  class="mon__proj"
                  :class="{ deployed: isDeployed(p.path) }"
                  @mouseenter="hoverProj = p.path"
                  @click="!isDeployed(p.path) && startDeploy(p.path)"
                >
                  <span class="mon__proj-glow"></span>
                  <img class="mon__proj-icon" src="/assets/wc3_extracted/ui/icon-folder.png" draggable="false" />
                  <span class="mon__proj-name" :style="{ fontSize: prefs.fs(14) + 'px' }">
                    {{ projects.displayName(p.path) }}
                  </span>
                  <span class="mon__proj-path" :style="{ fontSize: prefs.fs(10) + 'px' }">{{ p.path }}</span>
                  <span v-if="isDeployed(p.path)" class="mon__proj-tag" :style="{ fontSize: prefs.fs(10) + 'px' }">
                    已部署
                  </span>
                </div>
                <div v-if="projects.recent.length === 0" class="mon__proj-empty" :style="{ fontSize: prefs.fs(12) + 'px' }">
                  暂无最近项目
                  <br />
                  请先在主菜单「打开项目」
                </div>
              </div>
              <WarScrollBar :target="projListEl" :scale="0.8" />
            </div>
            <div v-if="hoverProj && collabChanges.length > 0" class="mon__collab">
              <div class="mon__collab-title" :style="{ fontSize: prefs.fs(12) + 'px' }">
                ⚙ 工作区动态 · {{ projects.displayName(hoverProj) }}
              </div>
              <div class="mon__collab-list">
                <div
                  v-for="c in collabChanges"
                  :key="c.id"
                  class="mon__collab-row"
                  :style="{ fontSize: prefs.fs(11) + 'px' }"
                >
                  <span class="mon__collab-path">{{ c.path }}</span>
                  <span class="mon__collab-meta">
                    <span v-if="c.session_id" class="mon__collab-who">会话{{ collabWho(c.session_id) }}</span>
                    <span class="mon__collab-when">{{ collabWhen(c.ts) }}</span>
                  </span>
                  <span v-if="c.summary" class="mon__collab-sum">{{ c.summary }}</span>
                </div>
              </div>
            </div>
            <div class="mon__rail-foot">
              <WarButton
                :width="190"
                text="返回(B)"
                shortcut-key="B"
                :shortcut-active="pageKeysOn && monitor.chatWins.length === 0 && !monitor.permDialogSessionId"
                @activated="nav.goMain()"
              />
            </div>
          </div>
        </WarFrame>
      </div>

      <!-- 右 field：土地沙盘（可视区；世界层 WORLD_SCALE 倍大，中键平移） -->
      <div
        ref="fieldEl"
        class="mon__field"
        @mousemove="onFieldMouseMove"
        @mousedown="onFieldMouseDown"
        @click="onFieldClick"
        @contextmenu="onFieldContextMenu"
        @wheel.prevent="onFieldWheel"
      >
        <div class="mon__field-hint" :style="{ fontSize: prefs.fs(15) + 'px' }">
          — 旷 野 —（滚轮缩放 / 中键平移 / 左键拖框多选 / 双击直聊 / 框选后右键区域归位 / Delete 删除选中）
        </div>
        <div class="mon__field-tools" :style="{ fontSize: prefs.fs(12) + 'px' }" @click.stop>
          <div class="mon__zones-toggle" @click="toggleZones">
            {{ prefs.monitorZonesOn ? '⛳ 隐藏区域' : '⛳ 显示区域' }}
          </div>
          <div class="mon__zones-toggle" @click="startTowerDeploy">🗼 部署塔</div>
        </div>

        <!-- 塔部署面板：选择要部署/移动的塔（点击 → ghost 跟随，左键放置） -->
        <div v-if="towerPanelOpen" class="mon__tower-panel" :style="{ fontSize: prefs.fs(13) + 'px' }" @click.stop>
          <div class="mon__tower-panel-title">🗼 塔部署</div>
          <div
            v-for="z in prefs.monitorZones"
            :key="'tp-' + z.id"
            class="mon__tower-panel-row"
            @click="pickTower(z.id)"
          >
            <span class="mon__tower-panel-dot" :style="{ background: z.color }"></span>
            <span class="mon__tower-panel-name">{{ z.name }}</span>
            <span class="mon__tower-panel-status" :class="{ on: z.deployed }">
              {{ z.deployed ? '已部署' : '未部署' }}
            </span>
          </div>
          <div class="mon__tower-panel-row add" @click="addNewTower">＋ 添加新塔</div>
          <div class="mon__tower-panel-hint">点击塔后在地图上左键放置，右键取消</div>
        </div>

        <!-- 世界层：兵营/步兵/部署 ghost 都在其中，随 pan 平移 -->
        <div
          class="mon__world"
          :style="{
            width: worldW + 'px',
            height: worldH + 'px',
            transform: `translate(${panX}px, ${panY}px) scale(${zoom})`,
            transformOrigin: '0 0',
          }"
        >
          <!-- 优先级区域（塔为圆心的圆；pointer-events:none 不拦截事件，
               右键命中由 field 级坐标判定，只有塔本体出菜单） -->
          <template v-if="prefs.monitorZonesOn">
            <div
              v-for="z in visibleZones"
              :key="'zone-' + z.id"
              class="mon__zone"
              :style="{
                left: z.x * worldW - z.r * worldW + 'px',
                top: z.y * worldH - z.r * worldW + 'px',
                width: z.r * worldW * 2 + 'px',
                height: z.r * worldW * 2 + 'px',
                borderColor: z.color,
                background: z.color + '14',
              }"
            >
              <div class="mon__zone-name" :style="{ fontSize: prefs.fs(13) + 'px' }">
                {{ z.name }}
                <span class="mon__zone-count" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ zoneCount(z.id) }}</span>
              </div>
            </div>
            <div
              v-for="z in visibleZones"
              :key="'tw-' + z.id"
              class="mon__tower"
              :style="{ left: z.x * worldW + 'px', top: z.y * worldH + 'px' }"
            >
              <img src="/assets/ui/monitor/tower.png" draggable="false" />
              <!-- 塔内染色区（原绿幕部分）染成区域色 -->
              <div class="mon__tower-tint" :style="{ background: z.color }"></div>
            </div>
            <!-- 隐藏区域的塔本体仍显示（半透明），可右键恢复显示，避免死锁 -->
            <div
              v-for="z in hiddenZones"
              :key="'twh-' + z.id"
              class="mon__tower off"
              :style="{ left: z.x * worldW + 'px', top: z.y * worldH + 'px' }"
            >
              <img src="/assets/ui/monitor/tower.png" draggable="false" />
              <div class="mon__tower-tint" :style="{ background: z.color }"></div>
            </div>
          </template>

          <!-- 步兵 ↔ 兵营 归属连线（绿色虚线，随步兵移动实时跟随） -->
          <svg class="mon__links" :width="worldW" :height="worldH">
            <template v-for="b in deployedList" :key="'ln-' + b.dir">
              <line
                v-for="(s, i) in slotsOf(b.dir)"
                :key="'ln-' + s.id"
                class="mon__link"
                :x1="anchorOf(b.dir).x"
                :y1="anchorOf(b.dir).y - 20"
                :x2="footmanPos(s.id, b.dir, i).x + 26"
                :y2="footmanPos(s.id, b.dir, i).y + 27"
              />
            </template>
          </svg>

          <!-- 兵营 -->
          <div
            v-for="b in deployedList"
            :key="b.dir"
            class="mon__barracks"
            :class="{ need: barracksNeed(b.dir) }"
            :style="{ left: anchorOf(b.dir).x + 'px', top: anchorOf(b.dir).y + 'px' }"
            @contextmenu="openBuildingMenu(b.dir, $event)"
          >
            <div class="mon__barracks-label" :style="{ fontSize: prefs.fs(14) + 'px' }">
              {{ projects.displayName(b.dir) }}
            </div>
            <img src="/assets/ui/monitor/barracks.png" draggable="false" />
          </div>

          <!-- 步兵 -->
          <template v-for="b in deployedList" :key="'f-' + b.dir">
            <div
              v-for="(s, i) in slotsOf(b.dir)"
              :key="s.id"
              class="mon__footman"
              :class="[footmanClass(s), { sel: selectedIds.has(s.id) }]"
              :style="{ left: footmanPos(s.id, b.dir, i).x + 'px', top: footmanPos(s.id, b.dir, i).y + 'px' }"
              @click.stop="onFootmanClick(s)"
              @dblclick.stop="onFootmanDblclick(s)"
              @contextmenu="openFootmanMenu(s, $event)"
            >
              <img src="/assets/ui/monitor/footman.png" draggable="false" />
              <div v-if="isPermPending(s.id)" class="mon__bang" :style="{ fontSize: prefs.fs(13) + 'px' }">!</div>
              <div v-else-if="hasUnread(s.id)" class="mon__unread" :style="{ fontSize: prefs.fs(9) + 'px' }">NEW</div>
              <div v-if="isBusy(s.id)" class="mon__talk">•••</div>
              <!-- 优先级标记（区域序号 → 色块菱形） -->
              <div
                v-if="footmanPriority(s.id) > 0"
                class="mon__prio"
                :style="{ background: priorityColor(footmanPriority(s.id)) }"
              ></div>
              <!-- 悬停状态气泡：优先级行 + 状态行 + 最后一条消息摘要 -->
              <div class="mon__fbub" :style="{ fontSize: prefs.fs(11) + 'px' }">
                <div
                  v-if="footmanPriority(s.id) > 0"
                  class="mon__fbub-prio"
                  :style="{ color: priorityColor(footmanPriority(s.id)) }"
                >
                  ⚑ {{ priorityName(s.id) }}
                </div>
                <div class="mon__fbub-status">{{ footmanStatus(s) }}</div>
                <div v-if="s.lastMessage" class="mon__fbub-msg">{{ trunc(s.lastMessage, 60) }}</div>
              </div>
              <div class="mon__fname" :style="{ fontSize: prefs.fs(10) + 'px' }">
                <input
                  v-if="renamingId === s.id"
                  v-model="renameText"
                  class="mon__rename"
                  :style="{ fontSize: prefs.fs(10) + 'px' }"
                  maxlength="24"
                  v-focus
                  @keydown.enter.prevent="commitRename"
                  @keydown.esc.stop.prevent="renamingId = ''"
                  @click.stop
                  @blur="commitRename"
                />
                <template v-else>{{ s.title }}</template>
              </div>
            </div>
          </template>

          <!-- 部署 ghost：塔带预显圆圈 / 兵营，跟随鼠标 -->
          <div v-if="ghost && towerPlacing" class="mon__ghost-zone" :style="ghostZoneStyle"></div>
          <img
            v-if="ghost && (monitor.deploying || towerPlacing)"
            class="mon__ghost"
            :class="towerPlacing ? 'mon__ghost-tower' : ''"
            :src="towerPlacing ? '/assets/ui/monitor/tower.png' : '/assets/ui/monitor/barracks.png'"
            :style="{
              left: ghost.x + 'px',
              top: ghost.y + 'px',
              ...(towerPlacing ? { filter: ghostTowerGlow } : {}),
            }"
            draggable="false"
          />

          <!-- 框选：绿色选择框（世界层内，随 pan 平移） -->
          <div
            v-if="boxSelect && boxDrag"
            class="mon__selbox"
            :style="{
              left: Math.min(boxStart.x, boxCur.x) + 'px',
              top: Math.min(boxStart.y, boxCur.y) + 'px',
              width: Math.abs(boxCur.x - boxStart.x) + 'px',
              height: Math.abs(boxCur.y - boxStart.y) + 'px',
            }"
          ></div>

          <!-- 移动指令落点标记：扩散光圈（世界层内） -->
          <div
            v-for="m in moveMarkers"
            :key="'mv-' + m.id"
            class="mon__mvmk"
            :style="{ left: m.x + 'px', top: m.y + 'px' }"
          ></div>
        </div>

        <div
          v-if="canPan"
          class="mon__pan-hint"
          :style="{ fontSize: prefs.fs(11) + 'px', bottom: mmH + 22 + 'px' }"
        >
          中键拖动地图
        </div>

        <!-- 小地图：兵营金色方块 / 步兵状态点 / 视口白框；左键按住拖动跳视口 -->
        <div
          ref="mmEl"
          class="mon__mm"
          :style="{ width: mmW + 'px', height: mmH + 'px' }"
          @mousedown="onMmMouseDown"
          @click.stop
          @contextmenu.stop.prevent
        >
          <div
            v-for="b in deployedList"
            :key="'mm-b-' + b.dir"
            class="mon__mm-b"
            :class="{ need: barracksNeed(b.dir) }"
            :style="{ left: b.pos.x * mmW + 'px', top: b.pos.y * mmH + 'px' }"
          ></div>
          <div
            v-for="z in visibleZones"
            :key="'mm-z-' + z.id"
            class="mon__mm-z"
            :style="{
              left: z.x * mmW - z.r * mmW + 'px',
              top: z.y * mmH - z.r * mmW + 'px',
              width: z.r * mmW * 2 + 'px',
              height: z.r * mmW * 2 + 'px',
              borderColor: z.color,
              borderRadius: '50%',
            }"
          ></div>
          <template v-for="b in deployedList" :key="'mm-f-' + b.dir">
            <div
              v-for="(s, i) in slotsOf(b.dir)"
              :key="'mm-s-' + s.id"
              class="mon__mm-f"
              :class="footmanClass(s)"
              :style="{
                left: footmanPos(s.id, b.dir, i).x * mmSx + 'px',
                top: footmanPos(s.id, b.dir, i).y * mmSy + 'px',
              }"
            ></div>
          </template>
          <div class="mon__mm-vp" :style="mmVpStyle"></div>
        </div>

        <!-- 右键弹出菜单 -->
        <div
          v-if="pop"
          class="mon__pop"
          :style="{ left: pop.x + 'px', top: pop.y + 'px' }"
          @click.stop
          @contextmenu.stop.prevent
        >
          <!-- 兵营菜单 -->
          <template v-if="pop.kind === 'building'">
            <div class="mon__ptitle" :style="{ fontSize: prefs.fs(13) + 'px' }">
              ⚔ {{ projects.displayName(pop.dir) }}（{{ monitor.sessionsOf(pop.dir).length }}/8）
            </div>
            <div
              v-if="monitor.sessionsOf(pop.dir).length >= 8"
              class="mon__item disabled"
              :style="{ fontSize: prefs.fs(13) + 'px' }"
            >
              ＋ 新会话（已满 8 个）
            </div>
            <template v-else>
              <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="newOpen = !newOpen">
                ＋ 新会话 ▸
              </div>
              <div v-if="newOpen" class="mon__newbox">
                <div class="mon__nsub" :style="{ fontSize: prefs.fs(11) + 'px' }">选择 Agent（★ 为默认）</div>
                <div
                  v-for="a in usableAgents"
                  :key="a.id"
                  class="mon__agent-row"
                  :class="{ on: pickAgent === a.id }"
                  :style="{ fontSize: prefs.fs(13) + 'px' }"
                  @click="pickAgent = a.id"
                >
                  <span>{{ a.name }}</span>
                  <span v-if="a.id === agents.defaultAgentId" class="mon__star">★</span>
                </div>
                <div v-if="usableAgents.length === 0" class="mon__nsub" :style="{ fontSize: prefs.fs(11) + 'px' }">
                  （无可用 Agent，将使用默认）
                </div>
                <div class="mon__nsub" style="margin-top: 8px" :style="{ fontSize: prefs.fs(11) + 'px' }">权限模式</div>
                <div class="mon__mode-row">
                  <span
                    v-for="[v, t] in PERM_MODES"
                    :key="v"
                    class="mon__mode-btn"
                    :class="{ on: pickMode === v, yolo: v === 'yolo' }"
                    :style="{ fontSize: prefs.fs(12) + 'px' }"
                    @click="pickMode = v"
                    >{{ t }}</span
                  >
                </div>
                <div class="mon__create-btn" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="createSession(pop.dir)">
                  ⚒ 创建会话
                </div>
              </div>
            </template>
            <div
              class="mon__item danger"
              :class="{ armed: razeArmed }"
              :style="{ fontSize: prefs.fs(13) + 'px' }"
              @click="onRazeClick(pop.dir)"
            >
              {{ razeArmed ? '⚠ 再次点击确认销毁（会话保留）' : '💥 销毁兵营' }}
            </div>
            <template v-if="monitor.shelvedOf(pop.dir).length > 0">
              <div class="mon__ptitle" style="margin-top: 6px" :style="{ fontSize: prefs.fs(13) + 'px' }">
                已搁置（{{ monitor.shelvedOf(pop.dir).length }}）
              </div>
              <input
                v-model="shelvedQuery"
                class="mon__search"
                type="text"
                placeholder="搜索搁置会话…"
                :style="{ fontSize: prefs.fs(12) + 'px' }"
                @click.stop
                @keydown.stop
              />
              <div class="mon__shelved-list">
                <div
                  v-for="s in filteredShelved"
                  :key="s.id"
                  class="mon__shelved-row"
                  :style="{ fontSize: prefs.fs(12) + 'px' }"
                >
                  <span class="mon__shelved-title">🗃 {{ s.title }}</span>
                  <span class="mon__restore" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="restoreShelved(pop.dir, s.id)">
                    恢复
                  </span>
                </div>
                <div v-if="filteredShelved.length === 0" class="mon__nsub" :style="{ fontSize: prefs.fs(11) + 'px' }">
                  （无匹配会话）
                </div>
              </div>
            </template>
          </template>

          <!-- 区域菜单 -->
          <template v-else-if="pop.kind === 'zone'">
            <template v-if="zoneMenuZone">
              <div class="mon__ptitle" :style="{ fontSize: prefs.fs(13) + 'px' }">
                ⛳ {{ zoneMenuZone.name }}
                <span class="mon__ptitle-count">{{ zoneCount(zoneMenuZone.id) }} 人</span>
              </div>
              <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="zoneRenaming = !zoneRenaming">
                ✎ 重命名
              </div>
              <div v-if="zoneRenaming" class="mon__newbox">
                <input
                  v-model="zoneNameText"
                  class="mon__search"
                  type="text"
                  maxlength="12"
                  :style="{ fontSize: prefs.fs(12) + 'px' }"
                  @keydown.enter.prevent="commitZoneRename"
                  @keydown.stop
                />
                <div class="mon__create-btn" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="commitZoneRename">
                  ✓ 确定
                </div>
              </div>
              <div class="mon__nsub" style="margin-top: 8px" :style="{ fontSize: prefs.fs(11) + 'px' }">区域颜色</div>
              <div class="mon__zone-colors">
                <span
                  v-for="c in ZONE_COLORS"
                  :key="c"
                  class="mon__zone-swatch"
                  :class="{ on: zoneMenuZone.color === c }"
                  :style="{ background: c }"
                  @click="recolorZone(zoneMenuZone.id, c)"
                ></span>
              </div>
              <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="startTowerMove(zoneMenuZone.id)">
                🚚 移动此塔
              </div>
              <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="removeTower(zoneMenuZone.id)">
                🗼 拆除此塔（回部署列表）
              </div>
              <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="adjustZoneRadius(zoneMenuZone.id, -0.015)">
                ➖ 缩小范围
              </div>
              <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="adjustZoneRadius(zoneMenuZone.id, 0.015)">
                ➕ 扩大范围
              </div>
              <div
                class="mon__item"
                :style="{ fontSize: prefs.fs(13) + 'px' }"
                @click="toggleZoneEnabled(zoneMenuZone.id)"
              >
                {{ zoneMenuZone.enabled === false ? '👁 显示此区' : '🙈 隐藏此区' }}
              </div>
              <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="clearZone(zoneMenuZone.id)">
                🧹 清空本区（步兵回栏位）
              </div>
              <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="resetZones">
                🔄 复位默认布局
              </div>
            </template>
          </template>

          <!-- 步兵菜单 -->
          <template v-else>
            <div class="mon__ptitle" :style="{ fontSize: prefs.fs(13) + 'px' }">🛡 {{ pop.session.title }}</div>
            <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="onFootmanAct('enter', pop.session)">
              ▶ 进入会话（完整页面）
            </div>
            <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="onFootmanAct('detail', pop.session)">
              💬 会话详情（小窗直接聊）
            </div>
            <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="onFootmanAct('rename', pop.session)">
              ✎ 重命名
            </div>
            <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="zonePickOpen = !zonePickOpen">
              ⛳ 送往区域 ▸
            </div>
            <div v-if="zonePickOpen" class="mon__zone-pick">
              <div
                v-for="z in visibleZones"
                :key="z.id"
                class="mon__zone-row"
                :style="{ fontSize: prefs.fs(13) + 'px' }"
                @click="onZonePick(pop.session.id, z)"
              >
                <span class="mon__zone-dot" :style="{ background: z.color }"></span>{{ z.name }}
              </div>
              <div v-if="visibleZones.length === 0" class="mon__nsub" :style="{ fontSize: prefs.fs(11) + 'px' }">
                （无启用区域）
              </div>
            </div>
            <div
              v-if="footmanPriority(pop.session.id) > 0"
              class="mon__item"
              :style="{ fontSize: prefs.fs(13) + 'px' }"
              @click="onFootmanAct('clearmark', pop.session)"
            >
              ✕ 清除优先级标记
            </div>
            <div class="mon__item" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="onFootmanAct('shelve', pop.session)">
              🗃 搁置（保留会话，不在此显示）
            </div>
            <div
              class="mon__item danger"
              :style="{ fontSize: prefs.fs(13) + 'px' }"
              @click="onFootmanAct('del', pop.session)"
            >
              ✕ 删除会话
            </div>
          </template>
        </div>
      </div>

      <!-- 迷你会话窗（多开，叠放序在窗口自身 z 上） -->
      <MonitorChatWin
        v-for="w in monitor.chatWins"
        :key="w.sessionId"
        :session-id="w.sessionId"
        :x="w.x"
        :y="w.y"
        :z="w.z"
        @close="monitor.closeChatWin(w.sessionId)"
        @toast="toast"
      />

      <!-- 权限审批弹窗 -->
      <MonitorPermDialog
        v-if="monitor.permDialogSessionId"
        :session-id="monitor.permDialogSessionId"
        :request="permDialogRequest"
      />

      <!-- toast -->
      <div v-if="toastText" class="mon__toast" :style="{ fontSize: prefs.fs(13) + 'px' }">{{ toastText }}</div>
    </div>
  </PageShell>
</template>

<script lang="ts">
// v-focus: 内联重命名输入框挂载时聚焦 + 全选（照 SessionSelectPage）。
export default {
  directives: {
    focus: {
      mounted(el: HTMLElement) {
        el.focus();
        (el as HTMLInputElement).select?.();
      },
    },
  },
};
</script>

<style scoped>
.mon {
  position: absolute;
  inset: 0;
  display: flex;
  font-family: SimSun, serif;
}

/* ---- 左 rail（frame_popup_small 框，照 RecentProjectsPanel） ---- */
.mon__rail {
  flex: none;
  width: 264px;
  z-index: 10;
  filter: drop-shadow(4px 0 14px #000a);
}

.mon__rail-frame {
  height: 100%;
}

.mon__rail-col {
  display: flex;
  flex-direction: column;
  height: 100%;
}

.mon__rail-head {
  padding: 6px 8px 8px;
  color: var(--war-gold);
  font-weight: bold;
  text-shadow: 1px 1px 0 #000;
  border-bottom: 1px solid #2a3344;
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
}

.mon__perm-count {
  color: var(--war-gold-dim);
  white-space: nowrap;
}

.mon__rail-hint {
  padding: 7px 8px;
  color: var(--war-text-muted);
  border-bottom: 1px solid #2a3344;
}

/* 列表 + WC3 滚动条并排 */
.mon__proj-wrap {
  flex: 1;
  min-height: 0;
  display: flex;
}

.mon__proj-list {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none; /* 原生条隐藏，WC3 WarScrollBar 替代 */
  padding: 6px 2px;
}

.mon__proj {
  position: relative;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 9px 8px;
  border: 1px solid #1a2230;
  border-radius: 3px;
  background: #10141dcc;
  margin-bottom: 5px;
}

.mon__proj:hover {
  border-color: #2c4a7a;
}

/* 行悬停辉光（KeyboardHighlight，照 RecentProjectsPanel recent__glow） */
.mon__proj-glow {
  position: absolute;
  inset: 0;
  background: url('/assets/wc3_extracted/ui/GlueScreen-Button-KeyboardHighlight.png') 0 0 / 100% 100% no-repeat;
  mix-blend-mode: screen;
  opacity: 0;
  pointer-events: none;
}

.mon__proj:hover:not(.deployed) .mon__proj-glow {
  opacity: 0.55;
}

.mon__proj.deployed {
  opacity: 0.45;
}

.mon__proj.deployed:hover {
  border-color: #1a2230;
}

.mon__proj-icon {
  position: relative;
  flex: none;
  width: 18px;
  height: 14px;
}

.mon__proj-name {
  position: relative;
  flex: 1;
  min-width: 0;
  color: #e8d9a0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  text-shadow: 1px 1px 0 #000;
}

.mon__proj.deployed .mon__proj-name {
  color: var(--war-text-muted);
}

.mon__proj-path {
  position: relative;
  flex: none;
  max-width: 62px;
  color: var(--war-text-faint);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.mon__proj-tag {
  position: relative;
  flex: none;
  color: #7ec97a;
  border: 1px solid #7ec97a55;
  padding: 0 4px;
  border-radius: 2px;
}

.mon__proj-empty {
  padding: 16px 10px;
  color: var(--war-text-faint);
  white-space: pre-line;
  text-align: center;
}

.mon__rail-foot {
  flex: none;
  padding: 8px 0 4px;
  border-top: 1px solid #2a3344;
  display: flex;
  justify-content: center;
}

/* ---- 工作区动态（hover 项目最近的文件改动） ---- */
.mon__collab {
  flex: none;
  max-height: 150px;
  border-top: 1px solid #2a3344;
  display: flex;
  flex-direction: column;
}

.mon__collab-title {
  flex: none;
  padding: 6px 8px;
  color: var(--war-gold);
  font-weight: bold;
  text-shadow: 1px 1px 0 #000;
  border-bottom: 1px solid #2a3344;
}

.mon__collab-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
  padding: 4px 2px;
}

.mon__collab-row {
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding: 4px 8px;
  border-bottom: 1px dashed #2a334455;
}

.mon__collab-path {
  color: #b9c4dc;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mon__collab-meta {
  display: flex;
  gap: 8px;
  color: var(--war-text-faint);
}

.mon__collab-who {
  color: #7ec97a;
}

.mon__collab-sum {
  color: #b9ad8a;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ---- 右 field（可视区） ---- */
.mon__field {
  position: relative;
  flex: 1;
  min-width: 0;
  overflow: hidden;
}

/* ---- 世界层（WORLD_SCALE 倍大地图，随 pan 平移；整张地图铺满世界层） ---- */
.mon__world {
  position: absolute;
  left: 0;
  top: 0;
  background: url('/assets/ui/monitor/map.webp') center / cover no-repeat;
}

/* 步兵归属连线（在 DOM 中先于兵营/步兵，绘制在它们之下） */
.mon__links {
  position: absolute;
  left: 0;
  top: 0;
  overflow: visible;
  pointer-events: none;
}

.mon__link {
  stroke: #7ec97a;
  stroke-width: 2;
  stroke-opacity: 0.55;
  stroke-dasharray: 5 4;
}

.mon__field::before {
  content: '';
  position: absolute;
  inset: 0;
  background: radial-gradient(ellipse at center, transparent 55%, #0009 100%);
  pointer-events: none;
  z-index: 6;
}

.mon__field-hint {
  position: absolute;
  top: 18px;
  width: 100%;
  text-align: center;
  color: #5a4a28;
  text-shadow: 0 1px 0 #ffffff30;
  pointer-events: none;
  z-index: 7;
}

.mon__pan-hint {
  position: absolute;
  right: 14px;
  bottom: 12px;
  color: #5a4a28;
  text-shadow: 0 1px 0 #ffffff30;
  pointer-events: none;
  z-index: 7;
}

/* 区域总开关 + 部署塔（悬浮按钮组；区域隐藏后仍可点回） */
.mon__field-tools {
  position: absolute;
  top: 18px;
  right: 14px;
  display: flex;
  gap: 8px;
  z-index: 7;
}

.mon__zones-toggle {
  padding: 4px 12px;
  color: var(--war-gold);
  background: #10141fcc;
  border: 1px solid #3a4a63;
  border-radius: 3px;
  cursor: pointer;
  user-select: none;
  text-shadow: 1px 1px 0 #000;
}

.mon__zones-toggle:hover {
  border-color: var(--war-gold-dim);
  background: #1a2230;
}

/* ---- 塔部署面板（右上角工具栏下方） ---- */
.mon__tower-panel {
  position: absolute;
  top: 62px;
  right: 14px;
  width: 216px;
  z-index: 8;
  background: #0e1320f2;
  border: 1px solid #3a4a63;
  border-radius: 4px;
  box-shadow: 0 6px 18px #000c;
  overflow: hidden;
}

.mon__tower-panel-title {
  padding: 6px 12px;
  color: var(--war-gold);
  font-weight: bold;
  border-bottom: 1px solid #26334a;
  text-shadow: 1px 1px 0 #000;
}

.mon__tower-panel-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px;
  color: #dfe6f0;
  cursor: pointer;
  user-select: none;
}

.mon__tower-panel-row:hover {
  background: #1a2230;
}

.mon__tower-panel-row.add {
  color: #7ec97a;
  border-top: 1px solid #26334a;
}

.mon__tower-panel-dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  flex: none;
}

.mon__tower-panel-name {
  flex: 1;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.mon__tower-panel-status {
  flex: none;
  padding: 1px 8px;
  border-radius: 8px;
  font-size: 11px;
  color: #9aa4b5;
  background: #ffffff14;
}

.mon__tower-panel-status.on {
  color: #7ec97a;
  background: #7ec97a1f;
}

.mon__tower-panel-hint {
  padding: 5px 12px 7px;
  color: #7d8798;
  border-top: 1px solid #26334a;
  font-size: 11px;
}

/* ---- 优先级区域（塔为圆心的圆） ---- */
.mon__zone {
  position: absolute;
  border: 2px dashed;
  border-radius: 50%;
  box-sizing: border-box;
  pointer-events: none; /* 不拦截框选/点击；右键命中由 field 坐标判定 */
  z-index: 0; /* 地图之上、步兵/连线之下 */
}

.mon__zone-name {
  position: absolute;
  top: 12px;
  left: 0;
  width: 100%;
  text-align: center;
  color: #fff;
  font-weight: bold;
  white-space: nowrap;
  text-shadow: 1px 1px 0 #000;
  opacity: 0.92;
}

.mon__zone-count {
  display: inline-block;
  min-width: 18px;
  text-align: center;
  margin-left: 6px;
  padding: 0 4px;
  border-radius: 8px;
  background: #000a;
  border: 1px solid #ffffff40;
  color: #e8d9a0;
}

/* 塔（区域圆心标记）：居中锚；塔身图 + 染色层（原绿幕部分染成区域色） */
.mon__tower {
  position: absolute;
  transform: translate(-50%, -50%);
  width: 56px;
  height: 158px; /* 源图 200:566 比例 */
  pointer-events: none;
  z-index: 1;
  filter: drop-shadow(4px 6px 6px #0008);
}

.mon__tower img {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  display: block;
}

/* 染色层：区域色 + 塔内染色区蒙版（tower-mask.png 白=可见） */
.mon__tower-tint {
  position: absolute;
  inset: 0;
  -webkit-mask-image: url('/assets/ui/monitor/tower-mask.png');
  mask-image: url('/assets/ui/monitor/tower-mask.png');
  -webkit-mask-size: 100% 100%;
  mask-size: 100% 100%;
}

.mon__tower.off {
  opacity: 0.35;
  filter: grayscale(0.8) drop-shadow(4px 6px 6px #0008);
}

/* ---- 小地图（装饰件固定像素，不走 prefs.fs） ---- */
.mon__mm {
  position: absolute;
  right: 12px;
  bottom: 12px;
  z-index: 8; /* 世界层/暗角之上，右键菜单(40)/小窗(70)之下 */
  background: linear-gradient(#10141fb3, #10141fb3), url('/assets/ui/monitor/map.webp') center / 100% 100% no-repeat;
  border: 2px solid #3a4a63;
  border-radius: 3px;
  box-shadow: 0 4px 16px #000a;
  cursor: pointer;
}

/* 兵营 = 金色小方块（等审批的项目呼吸闪烁） */
.mon__mm-b {
  position: absolute;
  width: 6px;
  height: 6px;
  background: var(--war-gold);
  border: 1px solid #a8840a;
  transform: translate(-50%, -50%);
  box-sizing: border-box;
}

.mon__mm-b.need {
  animation: mon-mm-breathe 1.2s infinite;
}

@keyframes mon-mm-breathe {
  0%,
  100% {
    box-shadow: 0 0 2px #ffd93b;
  }
  50% {
    background: #ffd93b;
    box-shadow: 0 0 9px #ffd93b;
  }
}

/* 步兵 = 2px 小点：run 绿 / perm 金 / idle 灰 */
.mon__mm-f {
  position: absolute;
  width: 2px;
  height: 2px;
  border-radius: 50%;
  background: #8a93a5;
  transform: translate(-50%, -50%);
}

.mon__mm-f.run {
  background: #7ec97a;
}

.mon__mm-f.perm {
  background: #ffd93b;
}

/* 视口白框 */
.mon__mm-vp {
  position: absolute;
  border: 1px solid #fff;
  box-sizing: border-box;
  pointer-events: none;
}

/* 区域在小地图上的虚线轮廓 */
.mon__mm-z {
  position: absolute;
  border: 1px dashed;
  opacity: 0.85;
  box-sizing: border-box;
  pointer-events: none;
}

/* ---- 兵营 ---- */
.mon__barracks {
  position: absolute;
  transform: translate(-50%, -100%);
}

.mon__barracks img {
  width: 190px;
  display: block;
  filter: drop-shadow(6px 10px 8px #0008);
}

.mon__barracks-label {
  position: absolute;
  top: -26px;
  width: 100%;
  text-align: center;
  color: var(--war-gold);
  font-weight: bold;
  white-space: nowrap;
  text-shadow:
    1px 1px 0 #000,
    0 0 8px #000;
}

.mon__barracks.need {
  animation: mon-bshake 0.5s infinite;
}

@keyframes mon-bshake {
  0%,
  100% {
    margin-top: 0;
  }
  50% {
    margin-top: -3px;
  }
}

/* ---- 步兵（WC3 头像图标；盒子 52×54，图标即盒子） ---- */
.mon__footman {
  position: absolute;
  width: 52px;
  height: 54px;
  animation: mon-spawn 0.35s ease-out;
}

@keyframes mon-spawn {
  from {
    transform: scale(0);
  }
}

.mon__footman img {
  width: 100%;
  height: 100%;
  border-radius: 4px;
  border: 2px solid #4a5b75;
  box-shadow: 2px 3px 6px #000a;
  box-sizing: border-box;
}

/* 选中：图标外圈绿色光环（run/perm 状态被覆盖，真正显眼的选中样式在
 * .mon__footman.sel img（金环）——见下面 run/perm 之后的重载）。 */
.mon__footman.sel img {
  border-color: #7ec97a;
  box-shadow:
    0 0 10px #7ec97acc,
    2px 3px 6px #000a;
}

/* 运行中：绿边框呼吸 */
.mon__footman.run img {
  border-color: #7ec97a;
  box-shadow:
    0 0 10px #7ec97a88,
    2px 3px 6px #000a;
  animation: mon-run-glow 1.4s infinite;
}

@keyframes mon-run-glow {
  50% {
    box-shadow:
      0 0 18px #7ec97acc,
      2px 3px 6px #000a;
  }
}

/* 待审批：金边框 */
.mon__footman.perm img {
  border-color: var(--war-gold-dim);
}

/* 选中（重载）：金色外圈，置于 run/perm 之后——运行中步兵边框本来也是
 * 绿的，若无此覆盖，“选中”将看不出任何变化（点了没反应）。金环保证任何
 * 状态下选中都一眼可见，并压掉 run 的呼吸动画。 */
.mon__footman.sel img {
  border-color: var(--war-gold);
  box-shadow:
    0 0 10px #ffd93bcc,
    0 0 18px #ffd93b55,
    2px 3px 6px #000a;
  animation: none;
}

/* 优先级标记：图标左上角旋转色块菱形（区域序号 → 色） */
.mon__prio {
  position: absolute;
  top: -7px;
  left: -7px;
  width: 14px;
  height: 14px;
  transform: rotate(45deg);
  border: 1px solid #000a;
  box-shadow: 0 0 6px #000a;
  z-index: 2;
}

.mon__bang {
  position: absolute;
  top: -22px;
  left: 50%;
  margin-left: -9px;
  width: 18px;
  height: 18px;
  line-height: 18px;
  text-align: center;
  font-weight: bold;
  color: #1a1000;
  background: #ffd93b;
  border: 1px solid #a8840a;
  border-radius: 50%;
  animation: mon-bounce 0.8s infinite;
  box-shadow: 0 0 8px #ffd93b;
}

@keyframes mon-bounce {
  0%,
  100% {
    transform: translateY(0);
  }
  50% {
    transform: translateY(-6px);
  }
}

.mon__unread {
  position: absolute;
  top: -8px;
  right: -10px;
  color: #ff6b5e;
  font-weight: bold;
  text-shadow: 1px 1px 0 #000;
}

/* 运行中：头顶小气泡呼吸 */
.mon__talk {
  position: absolute;
  top: -18px;
  right: -10px;
  padding: 0 6px;
  line-height: 14px;
  font-size: 10px;
  color: #7ec97a;
  background: #10141f;
  border: 1px solid #7ec97a;
  border-radius: 8px;
  animation: mon-talk 1.2s infinite;
  pointer-events: none;
}

@keyframes mon-talk {
  50% {
    opacity: 0.35;
  }
}

/* 悬停状态气泡（状态行 + 最后一条消息摘要） */
.mon__fbub {
  position: absolute;
  bottom: calc(100% + 10px);
  left: 50%;
  transform: translateX(-50%);
  min-width: 150px;
  max-width: 230px;
  padding: 6px 8px;
  background: #10141ff2;
  border: 1px solid #3a4a63;
  border-radius: 4px;
  box-shadow: 0 4px 14px #000c;
  display: none;
  z-index: 20;
  pointer-events: none;
}

.mon__footman:hover .mon__fbub {
  display: block;
}

.mon__fbub::after {
  content: '';
  position: absolute;
  top: 100%;
  left: 50%;
  margin-left: -5px;
  border: 5px solid transparent;
  border-top-color: #3a4a63;
}

.mon__fbub-status {
  color: var(--war-gold);
  font-weight: bold;
  margin-bottom: 2px;
  white-space: nowrap;
}

.mon__fbub-prio {
  font-weight: bold;
  margin-bottom: 2px;
  white-space: nowrap;
  text-shadow: 1px 1px 0 #000;
}

.mon__fbub-msg {
  color: var(--war-text-muted);
  word-break: break-all;
  line-height: 1.4;
}

.mon__fname {
  position: absolute;
  top: 100%;
  width: 76px;
  left: -12px;
  text-align: center;
  color: #e8d9a0;
  text-shadow: 1px 1px 0 #000;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.mon__rename {
  width: 76px;
  font-family: SimSun, serif;
  color: var(--war-gold);
  background: #0a0d14;
  border: 1px solid var(--war-gold-dim);
  outline: none;
  text-align: center;
  padding: 1px 2px;
  box-sizing: border-box;
}

/* ---- 部署 ghost ---- */
.mon__ghost {
  position: absolute;
  width: 190px;
  opacity: 0.55;
  transform: translate(-50%, -100%);
  filter: drop-shadow(0 0 12px #7ec97a88);
  pointer-events: none;
  z-index: 5;
}

/* 塔 ghost：居中锚 + 细图 */
.mon__ghost-tower {
  width: 56px;
  transform: translate(-50%, -50%);
  filter: drop-shadow(0 0 12px #7ec97a88);
}

/* 塔部署预显圆圈 */
.mon__ghost-zone {
  position: absolute;
  border: 2px dashed #7ec97a;
  border-radius: 50%;
  background: #7ec97a18;
  pointer-events: none;
  z-index: 4;
}

/* ---- 框选：绿色描边选择框（内部透明，仅边框可见） ---- */
.mon__selbox {
  position: absolute;
  border: 2px solid #7ec97a;
  background: transparent;
  box-shadow: 0 0 10px #7ec97a55;
  pointer-events: none;
  z-index: 9;
}

/* ---- 移动指令落点：扩散光圈（居中于右键点，扩散并淡出） ---- */
.mon__mvmk {
  position: absolute;
  width: 46px;
  height: 46px;
  margin: -23px 0 0 -23px;
  border: 2px solid #7ec97a;
  border-radius: 50%;
  box-shadow: 0 0 10px #7ec97a66;
  animation: mon-mvmk 0.6s ease-out forwards;
  pointer-events: none;
  z-index: 9;
}

@keyframes mon-mvmk {
  0% {
    transform: scale(0.3);
    opacity: 0.9;
  }
  100% {
    transform: scale(1.6);
    opacity: 0;
  }
}

/* ---- 右键弹出菜单（原型 .pop；dropdown_panel2 九宫格面板，照 WarDropdown） ---- */
.mon__pop {
  position: absolute;
  z-index: 40;
  border-style: solid;
  border-color: transparent;
  border-width: 13px 14px 12px 14px; /* T R B L（slice 21/23/20/23） */
  border-image: url('/assets/ui/dropdown/dropdown_panel2.png') 21 23 20 23 fill stretch;
  /* WebView2 不一定绘制 border-image 中心切片，深蓝兜底层（照 WarDropdown） */
  background: #060d33d9 padding-box;
  box-sizing: border-box;
  box-shadow: 0 6px 24px #000d;
  padding: 10px;
  min-width: 190px;
  max-width: 340px;
}

.mon__ptitle {
  color: var(--war-gold);
  font-weight: bold;
  padding-bottom: 7px;
  margin-bottom: 7px;
  border-bottom: 1px solid #2a3344;
  text-shadow: 1px 1px 0 #000;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.mon__ptitle-count {
  margin-left: 6px;
  color: var(--war-text-muted);
  font-weight: normal;
}

/* 步兵菜单「送往区域」子面板 / 区域换色 */
.mon__zone-pick {
  border: 1px solid #2a3344;
  background: #0a0d14;
  margin: 4px 2px 8px;
  padding: 6px;
}

.mon__zone-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 8px;
  color: #b9c4dc;
  border-radius: 2px;
}

.mon__zone-row:hover {
  background: #32509640;
  color: var(--war-gold);
}

.mon__zone-dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  flex: none;
}

.mon__zone-colors {
  display: flex;
  gap: 6px;
  padding: 4px 2px 8px;
}

.mon__zone-swatch {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  border: 2px solid transparent;
  cursor: pointer;
}

.mon__zone-swatch:hover {
  transform: scale(1.15);
}

.mon__zone-swatch.on {
  border-color: #fff;
  box-shadow: 0 0 6px #fff8;
}

.mon__item {
  padding: 7px 12px;
  color: #e8d9a0;
  border-radius: 2px;
  white-space: nowrap;
  text-shadow: 1px 1px 0 #000;
}

.mon__item:hover {
  background: #32509660;
  color: var(--war-gold);
}

.mon__item.danger:hover {
  background: #6b2d2d80;
  color: #ff9b8a;
}

.mon__item.danger.armed {
  background: #6b2d2d;
  color: #ff9b8a;
  border: 1px solid #b0552f;
}

.mon__item.disabled {
  color: var(--war-text-faint);
}

.mon__item.disabled:hover {
  background: none;
  color: var(--war-text-faint);
}

.mon__shelved-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 8px;
  color: #b9ad8a;
}

.mon__shelved-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mon__restore {
  margin-left: auto;
  color: #7ec97a;
  border: 1px solid #7ec97a55;
  padding: 0 5px;
  border-radius: 2px;
}

.mon__restore:hover {
  background: #7ec97a22;
}

.mon__search {
  width: 100%;
  box-sizing: border-box;
  margin: 4px 0 6px;
  padding: 4px 8px;
  background: #0a0d14;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: #e8d9a0;
  outline: none;
}

.mon__search:focus {
  border-color: #7ec97a66;
}

.mon__shelved-list {
  max-height: 168px;
  overflow-y: auto;
  overflow-x: hidden;
  padding-right: 2px;
}

.mon__shelved-list .mon__shelved-row {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 新会话内联面板 */
.mon__newbox {
  border: 1px solid #2a3344;
  background: #0a0d14;
  margin: 4px 2px 8px;
  padding: 8px;
}

.mon__nsub {
  color: var(--war-text-muted);
  margin: 4px 0 5px;
}

.mon__agent-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 8px;
  color: #b9c4dc;
  border-radius: 2px;
  border: 1px solid transparent;
}

.mon__agent-row:hover {
  background: #32509640;
}

.mon__agent-row.on {
  border-color: var(--war-gold-dim);
  color: var(--war-gold);
  background: #c9a22714;
}

.mon__star {
  color: var(--war-gold-dim);
  font-size: 11px;
}

.mon__mode-row {
  display: flex;
  gap: 5px;
}

.mon__mode-btn {
  flex: 1;
  text-align: center;
  padding: 4px 0;
  color: #b9c4dc;
  border: 1px solid #2a3344;
  border-radius: 2px;
  background: #10141f;
}

.mon__mode-btn:hover {
  border-color: #4a5b75;
}

.mon__mode-btn.on {
  border-color: var(--war-gold-dim);
  color: var(--war-gold);
  background: #c9a2271f;
}

.mon__mode-btn.on.yolo {
  border-color: #b0552f;
  color: #ff9b8a;
  background: #b0552f1f;
}

.mon__create-btn {
  margin-top: 8px;
  text-align: center;
  padding: 6px 0;
  color: #a8e6a0;
  border: 1px solid #7ec97a66;
  border-radius: 2px;
  background: #7ec97a12;
}

.mon__create-btn:hover {
  background: #7ec97a2a;
}

/* ---- toast ---- */
.mon__toast {
  position: absolute;
  left: 50%;
  bottom: 40px;
  transform: translateX(-50%);
  background: #10141fee;
  border: 1px solid #f5c45266;
  color: var(--war-gold);
  padding: 8px 18px;
  border-radius: 3px;
  z-index: 80;
  text-shadow: 1px 1px 0 #000;
  white-space: nowrap;
}
</style>

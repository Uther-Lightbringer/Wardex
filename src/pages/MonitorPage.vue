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
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import PageShell from '../components/PageShell.vue';
import WarButton from '../components/war/WarButton.vue';
import WarFrame from '../components/war/WarFrame.vue';
import WarScrollBar from '../components/war/WarScrollBar.vue';
import MonitorChatWin from '../components/monitor/MonitorChatWin.vue';
import MonitorPermDialog from '../components/monitor/MonitorPermDialog.vue';
import { useNavStore } from '../stores/nav';
import { usePrefsStore } from '../stores/prefs';
import { useSessionsStore, type SessionIndexRow } from '../stores/sessions';
import { useProjectsStore } from '../stores/projects';
import { useAgentsStore } from '../stores/agents';
import { useMonitorStore } from '../stores/monitor';
import { useChatStore } from '../stores/chat';
import { useUiStore } from '../stores/ui';

const nav = useNavStore();
const prefs = usePrefsStore();
const sessions = useSessionsStore();
const projects = useProjectsStore();
const agents = useAgentsStore();
const monitor = useMonitorStore();
const chat = useChatStore();
const ui = useUiStore();

// ---------------------------------------------------------------------------
// 数据加载（页面常驻：onMounted 一次 + 每次切入重拉）
// ---------------------------------------------------------------------------

onMounted(() => {
  void projects.load();
  void agents.refresh();
  void monitor.initListeners();
  void monitor.refresh();
});

watch(
  () => nav.page,
  (p) => {
    if (p === 'monitor') {
      void projects.load();
      void monitor.refresh();
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

/** 世界相对可视区的倍数。越大可平移范围越大，但地图图被放大越多越糊。 */
const WORLD_SCALE = 1.6;
const worldW = computed(() => Math.round(fieldW.value * WORLD_SCALE));
const worldH = computed(() => Math.round(fieldH.value * WORLD_SCALE));
const panX = ref(0);
const panY = ref(0);
/** 世界大于可视区才有得拖（此时显示"中键拖动地图"提示）。 */
const canPan = computed(() => worldW.value > fieldW.value || worldH.value > fieldH.value);

function clampPan(): void {
  const minX = Math.min(0, fieldW.value - worldW.value);
  const minY = Math.min(0, fieldH.value - worldH.value);
  panX.value = Math.max(minX, Math.min(0, panX.value));
  panY.value = Math.max(minY, Math.min(0, panY.value));
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
  if (e.button !== 1) return;
  e.preventDefault(); // 防浏览器中键自动滚动图标
  panning = true;
  panStartX = e.clientX;
  panStartY = e.clientY;
  panBaseX = panX.value;
  panBaseY = panY.value;
  window.addEventListener('mousemove', onPanMove);
  window.addEventListener('mouseup', onPanUp);
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

// ---- 小地图（minimap）：兵营方块 / 步兵点 / 视口白框，左键按住拖动跳视口 ----

const mmEl = ref<HTMLElement | null>(null);
const mmW = 180;
/** 高度按世界层宽高比等比（world 两边同比放大，即 field 宽高比）。 */
const mmH = computed(() => Math.round((mmW * worldH.value) / worldW.value));
/** 世界 px → 小地图 px 缩放比。 */
const mmSx = computed(() => mmW / worldW.value);
const mmSy = computed(() => mmH.value / worldH.value);

/** 视口白框：pan 是响应式的，中键拖地图本体时天然实时跟随。 */
const mmVpStyle = computed(() => ({
  left: (-panX.value / worldW.value) * mmW + 'px',
  top: (-panY.value / worldH.value) * mmH.value + 'px',
  width: (fieldW.value / worldW.value) * mmW + 'px',
  height: (fieldH.value / worldH.value) * mmH.value + 'px',
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

function onFieldMouseMove(e: MouseEvent): void {
  if (!monitor.deploying || !fieldEl.value) {
    ghost.value = null;
    return;
  }
  const r = fieldEl.value.getBoundingClientRect();
  // 世界坐标 = 屏幕坐标 - pan
  ghost.value = { x: e.clientX - r.left - panX.value, y: e.clientY - r.top - panY.value };
}

function onFieldClick(e: MouseEvent): void {
  if (!monitor.deploying || !fieldEl.value) {
    selectedId.value = null; // RTS：点地面空白取消选中
    closePop(); // 原型：点空白处关闭右键菜单
    return;
  }
  const dir = monitor.deploying;
  const r = fieldEl.value.getBoundingClientRect();
  const x = Math.max(0, Math.min(1, (e.clientX - r.left - panX.value) / worldW.value));
  const y = Math.max(0, Math.min(1, (e.clientY - r.top - panY.value) / worldH.value));
  ghost.value = null;
  void monitor.deploy(dir, x, y).then(() => {
    toast(`已部署「${projects.displayName(dir)}」兵营`);
  });
}

function onFieldContextMenu(e: MouseEvent): void {
  e.preventDefault();
  if (monitor.deploying) {
    monitor.cancelDeploy();
    ghost.value = null;
    toast('已取消部署');
    closePop();
    return;
  }
  // RTS：有选中步兵时右键地面 = 命令其走过去（不弹菜单，目标 = 右键的世界坐标）
  if (selectedId.value && fieldEl.value) {
    const r = fieldEl.value.getBoundingClientRect();
    // 世界坐标 = 屏幕坐标 - pan（同 ghost / 部署落点换算），clamp 到世界边界
    const wx = Math.max(0, Math.min(worldW.value, e.clientX - r.left - panX.value));
    const wy = Math.max(0, Math.min(worldH.value, e.clientY - r.top - panY.value));
    commandMove(selectedId.value, wx, wy);
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
  selectedId.value = s.id;
}

function onFootmanDblclick(s: SessionIndexRow): void {
  if (isPermPending(s.id)) return; // 等审批的走左键审批，不在双击里开窗
  monitor.openChatWin(s.id);
}

// ---------------------------------------------------------------------------
// RTS：选中 + 右键移动（位置不持久化，重启回栏位）
// ---------------------------------------------------------------------------

/** 当前选中的步兵（会话 id）。 */
const selectedId = ref<string | null>(null);

/** 世界坐标覆盖层：被命令移动过的步兵不再跟随栏位，left/top 优先取这里。 */
const posOverride = ref<Record<string, { x: number; y: number }>>({});

/** 渲染用位置：覆盖值优先，否则回退栏位计算值（小地图同源）。 */
function footmanPos(id: string, dir: string, i: number): { x: number; y: number } {
  return posOverride.value[id] ?? slotPos(dir, i);
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

/** 右键地面下令：目标点 wx/wy 是图标中心落点，换算成盒子左上角存入覆盖层。 */
function commandMove(id: string, wx: number, wy: number): void {
  if (!sessionAlive(id)) return;
  if (!posOverride.value[id]) posOverride.value[id] = { ...footmanPosCurrent(id) };
  moveTargets.set(id, { tx: wx - 26, ty: wy - 27 }); // 盒子 52×54
  if (!rafId) {
    lastTs = performance.now();
    rafId = requestAnimationFrame(tick);
  }
}

/** 当前实际位置（覆盖值优先，否则栏位）。 */
function footmanPosCurrent(id: string): { x: number; y: number } {
  const o = posOverride.value[id];
  if (o) return o;
  for (const b of deployedList.value) {
    const i = slotsOf(b.dir).findIndex((s) => s.id === id);
    if (i >= 0) return slotPos(b.dir, i);
  }
  return { x: 0, y: 0 };
}

/** 统一 rAF 循环：处理所有正在走的步兵，全到位后自停。 */
function tick(ts: number): void {
  const dt = Math.min(0.05, (ts - lastTs) / 1000);
  lastTs = ts;
  for (const [id, t] of moveTargets) {
    const p = posOverride.value[id];
    if (!p || !sessionAlive(id)) {
      moveTargets.delete(id);
      continue;
    }
    const dx = t.tx - p.x;
    const dy = t.ty - p.y;
    const dist = Math.hypot(dx, dy);
    const step = MOVE_SPEED * dt;
    if (dist <= step) {
      p.x = t.tx;
      p.y = t.ty;
      moveTargets.delete(id);
    } else {
      p.x += (dx / dist) * step;
      p.y += (dy / dist) * step;
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
  | { kind: 'footman'; session: SessionIndexRow; x: number; y: number };

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

function openFootmanMenu(s: SessionIndexRow, e: MouseEvent): void {
  e.preventDefault();
  e.stopPropagation();
  const p = popPos(e);
  pop.value = { kind: 'footman', session: s, x: p.x, y: p.y };
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
      void monitor.setShelved(s.id, true).then(() => toast(`已搁置「${s.title}」（会话保留）`));
      break;
    case 'del':
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
});

// ---------------------------------------------------------------------------
// Esc / 快捷键（SessionSelectPage 模式；优先级：小窗 → 审批 → 部署 → 菜单 → 返回）
// ---------------------------------------------------------------------------

function onPageKey(e: KeyboardEvent): void {
  if (nav.page !== 'monitor') return;
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
  if (monitor.deploying) {
    monitor.cancelDeploy();
    ghost.value = null;
    return;
  }
  if (pop.value) {
    closePop();
    return;
  }
  if (selectedId.value) {
    selectedId.value = null; // RTS：Esc 取消选中
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
      >
        <div class="mon__field-hint" :style="{ fontSize: prefs.fs(15) + 'px' }">
          — 旷 野 —（左键选中步兵 / 双击直聊 / 右键地面命令移动 / 右键兵营·步兵出菜单）
        </div>

        <!-- 世界层：兵营/步兵/部署 ghost 都在其中，随 pan 平移 -->
        <div
          class="mon__world"
          :style="{
            width: worldW + 'px',
            height: worldH + 'px',
            transform: `translate(${panX}px, ${panY}px)`,
          }"
        >
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
              :class="[footmanClass(s), { sel: selectedId === s.id }]"
              :style="{ left: footmanPos(s.id, b.dir, i).x + 'px', top: footmanPos(s.id, b.dir, i).y + 'px' }"
              @click.stop="onFootmanClick(s)"
              @dblclick.stop="onFootmanDblclick(s)"
              @contextmenu="openFootmanMenu(s, $event)"
            >
              <img src="/assets/ui/monitor/footman.png" draggable="false" />
              <div v-if="isPermPending(s.id)" class="mon__bang" :style="{ fontSize: prefs.fs(13) + 'px' }">!</div>
              <div v-else-if="hasUnread(s.id)" class="mon__unread" :style="{ fontSize: prefs.fs(9) + 'px' }">NEW</div>
              <div v-if="isBusy(s.id)" class="mon__talk">•••</div>
              <!-- 悬停状态气泡：状态行 + 最后一条消息摘要 -->
              <div class="mon__fbub" :style="{ fontSize: prefs.fs(11) + 'px' }">
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

          <!-- 部署 ghost（半透明兵营跟随鼠标） -->
          <img
            v-if="monitor.deploying && ghost"
            class="mon__ghost"
            src="/assets/ui/monitor/barracks.png"
            :style="{ left: ghost.x + 'px', top: ghost.y + 'px' }"
            draggable="false"
          />
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
                  🗃 {{ s.title }}
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

/* 选中：图标外圈绿色光环 */
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

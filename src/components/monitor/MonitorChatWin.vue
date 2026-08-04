<script setup lang="ts">
// 迷你会话窗（原型 #chatwin）：不切 active 的后台会话小窗，可多开。
//   - 外观：frame_popup_small 九宫格窗框（照 RecentProjectsPanel 的 half-scale
//     popup 框）；气泡套 frame_chat_bubble_body 石框（照 ChatBubble）；滚动条
//     换 WarScrollBar；底栏 Agent/模型/权限换 WarDropdown（dropUp），发送换
//     WarButton(dialog 皮)。标题栏按住拖动换位（pointer capture 照 WarPanel，
//     clamp 在可视区内；位置不持久化），右下角金色斜纹 grip 拖动拉伸窗口
//     （尺寸持久化在 prefs.monitorChatWidth/Height，松手落盘一次；双击 grip
//     复原默认并清除持久化）。多开/叠放序/位置在 monitor store chatWins，
//     点击窗口任意处置前。
//   - 历史：session_messages 整拉，保留完整 segments。ai 气泡 markdown 渲染
//     （复用 lib/markdown.ts renderMarkdown，content-addressed 缓存照
//     ChatBubble；流式中的尾段先纯文本，turn 落盘整拉后才上 markdown）；
//     user 气泡纯文本优先、嵌图片 markdown 时走 renderUserMarkdown；附件图
//     内嵌（fileSrc，点击系统打开）、非图片附件文件名芯片。
//   - 流式：组件内自行 listen 全局事件并按 sessionId 过滤——acp://chunk
//     （text/thinking 增量拼尾段，需要时新段）、acp://tool（按 toolCallId
//     upsert 工具段）、acp://turn（整拉落盘收尾）、chat://messageAppended /
//     chat://bubbleSet（整行追加/替换）。打开时若最后一行是流式中
//     （pending/streaming）就接它继续拼（ensureStreamRow），不另起气泡。
//     量小，直接响应式更新，不做主窗的 DOM 级 streamTarget 优化。
//   - 工具/思考：每条助手消息的非 text 段折叠为「⚙ N 个步骤（思考×n ·
//     工具×n）」一行（procSummary 逻辑移植自 ChatBubble），点击开复用的
//     ProcessDialog（segments 传该消息完整 segments，流式中也实时）。
//   - 发送：ensure_runtime → send_prompt；user 气泡靠 chat://messageAppended
//     回来渲染（不本地插，与主窗一致），失败才本地补一条提示行。
//   - 底栏：Agent 下拉（可切换，monitor.switchAgent → 后端 switch_agent）
//     + 模型（只读展示当前模型——ChatPage composer 本就无模型切换，
//     模型挂在 Agent 配置上）+ 权限模式下拉（set_session_perm_mode）+ 发送。
//   - 打开期间该会话有权限请求（isPermPending false→true，含打开时已为真）
//     自动弹审批弹窗；acp://permissionCleared 由 monitor store 关弹窗。
//   - 「在完整页面打开 →」走 chat.openSession + nav.goOverlay('chat')。
//   - Esc 由 MonitorPage 页级处理（monitor.closeTopChatWin 关最上层）。
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { cmd, fileSrc, openPath } from '../../lib/tauri';
import { renderMarkdown, renderUserMarkdown, handleMdLinkClick } from '../../lib/markdown';
import { copyText } from '../../lib/clipboard';
import { visibleText, useSessionsStore, type StoredMessage } from '../../stores/sessions';
import { useMonitorStore } from '../../stores/monitor';
import { useChatStore, type ChatMessage, type ChatSegment, type SessionMeta } from '../../stores/chat';
import { useAgentsStore } from '../../stores/agents';
import { useNavStore } from '../../stores/nav';
import { useUiStore } from '../../stores/ui';
import { usePrefsStore } from '../../stores/prefs';
import WarButton from '../war/WarButton.vue';
import WarDropdown from '../war/WarDropdown.vue';
import WarScrollBar from '../war/WarScrollBar.vue';
import ProcessDialog from '../chat/ProcessDialog.vue';

const props = defineProps<{ sessionId: string; x: number; y: number; z: number }>();
const emit = defineEmits<{ (e: 'close'): void; (e: 'toast', msg: string): void }>();

const sessions = useSessionsStore();
const monitor = useMonitorStore();
const chat = useChatStore();
const agents = useAgentsStore();
const nav = useNavStore();
const ui = useUiStore();
const prefs = usePrefsStore();

// ---- 消息模型：完整 ChatMessage（保留 thinking/tool 段），响应式直接更新 ----

function textOf(row: ChatMessage): string {
  return visibleText(row).trim();
}
function roleClass(row: ChatMessage): string {
  return row.role === 'user' ? 'user' : row.role === 'assistant' ? 'ai' : 'sys';
}
/** 流式中（pending/streaming）的助手行：chunk/tool 事件的追加目标。 */
function isStreaming(row: ChatMessage): boolean {
  return row.role === 'assistant' && (row.status === 'pending' || row.status === 'streaming');
}

// ---- 过程行（移植 ChatBubble procSummary：非 text 段折叠为一行） ----

function structSegsOf(row: ChatMessage): ChatSegment[] {
  return (row.segments ?? []).filter((s) => s.kind !== 'text');
}
function procSummary(row: ChatMessage): string {
  const segs = structSegsOf(row);
  const think = segs.filter((s) => s.kind === 'thinking').length;
  const tools = segs.filter((s) => s.kind === 'tool').length;
  const parts: string[] = [];
  if (think > 0) parts.push(`思考×${think}`);
  if (tools > 0) parts.push(`工具×${tools}`);
  return `⚙ ${segs.length} 个步骤（${parts.join(' · ')}）`;
}

/** 流式中的当前动作提示（尾段；照 ChatBubble activityHint）。 */
const TOOL_HINT_MAX = 24;
function elideToolName(s: string): string {
  return s.length <= TOOL_HINT_MAX ? s : s.slice(0, TOOL_HINT_MAX) + '…';
}
function activityHint(row: ChatMessage): string {
  const tail = row.segments?.[row.segments.length - 1];
  if (!tail) return '';
  if (tail.kind === 'thinking') return '思考中…';
  if (tail.kind === 'tool') {
    const st = tail.status ? ` [${String(tail.status)}]` : '';
    return `▶ ${elideToolName(String(tail.name || tail.title || 'tool'))}${st}`;
  }
  return '';
}

/** 过程详情弹窗（复用 ProcessDialog）：记录当前打开的行 id，内容随流式实时。 */
const procRowId = ref('');
const procOpen = ref(false);
const procRow = computed(() => rows.value.find((r) => r.id === procRowId.value) ?? null);
function openProc(row: ChatMessage): void {
  procRowId.value = row.id;
  procOpen.value = true;
}

// ---- markdown（照 ChatBubble：content-addressed 缓存 + 用户图判定） ----

const mdCache = new Map<string, string>();
function markdownOf(text: string): string {
  let html = mdCache.get(text);
  if (html === undefined) {
    html = renderMarkdown(text);
    if (mdCache.size > 500) mdCache.clear(); // bounded, content-addressed
    mdCache.set(text, html);
  }
  return html;
}

/** user 文本只有嵌了图片 markdown（Composer 粘贴图）才走渲染器（照 ChatBubble）。 */
const MD_IMG_RE = /!\[[^\]]*\]\(/;
function userHtml(text: string): string {
  return MD_IMG_RE.test(text) ? renderUserMarkdown(text) : '';
}

// ---- 附件（照 ChatBubble：图片内嵌 / 其余文件名芯片，点击系统打开） ----

const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'];
function isImagePath(p: string): boolean {
  const ext = p.split('.').pop()?.toLowerCase() ?? '';
  return IMAGE_EXTS.includes(ext);
}
function fileName(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}
/** 已嵌进正文 markdown 图片的附件不重复显示（照 ChatBubble visibleAtts）。 */
function visibleAtts(row: ChatMessage): string[] {
  const text = textOf(row);
  return (row.attachments ?? []).filter((p) => !text.includes(p.replace(/\\/g, '/')));
}

/** v-html 委托点击（照 ChatBubble.onBodyClick）：代码块复制按钮 + 链接
 * （http 系统浏览器 / 本地路径 OS 打开；webview 自身绝不导航）。 */
function onBodyClick(e: MouseEvent): void {
  const t = e.target as HTMLElement;
  if (!t.closest('.md-body')) return;
  const btn = t.closest<HTMLElement>('.codeblock__copy');
  if (btn) {
    const text = btn.parentElement?.querySelector('pre')?.innerText ?? '';
    if (!text.trim()) return;
    void copyText(text).then((ok) => {
      if (!ok) return;
      btn.textContent = '已复制';
      setTimeout(() => (btn.textContent = '复制'), 1200);
    });
    return;
  }
  handleMdLinkClick(e);
}

const meta = ref<SessionMeta | null>(null);
const rows = ref<ChatMessage[]>([]);
const input = ref('');
const sending = ref(false);
const bodyEl = ref<HTMLElement | null>(null);

const indexRow = computed(() => sessions.all.find((s) => s.id === props.sessionId));
const title = computed(() => meta.value?.title ?? indexRow.value?.title ?? '会话');
const agentLine = computed(() => {
  const ag = meta.value?.agentName ?? indexRow.value?.agentName ?? '';
  const dir = meta.value?.projectDir ?? indexRow.value?.projectDir ?? '';
  return [ag, dir].filter(Boolean).join(' · ');
});
const modelLabel = computed(() => meta.value?.model || '默认');
const permMode = computed(() => indexRow.value?.permMode ?? 'default');
const busy = computed(() => sessions.runtimeStates[props.sessionId]?.busy === true);

// ---- 窗口拉伸（右下角 grip；left/top 定位 → 位移直接跟手） ----
// 尺寸持久化在 prefs.monitorChatWidth/Height（0 = 未拖过，用默认），松手
// 落盘一次；双击 grip 复原默认并清除持久化。

const WIN_DEF_W = 560;
const WIN_MIN_W = 480;
const WIN_MIN_H = 360;

function defaultH(): number {
  return Math.min(Math.max(Math.round(window.innerHeight * 0.74), WIN_MIN_H), 780);
}

function clampWin(w: number, h: number): [number, number] {
  const maxW = Math.round(window.innerWidth * 0.94);
  const maxH = Math.round(window.innerHeight * 0.92);
  return [Math.max(WIN_MIN_W, Math.min(maxW, w)), Math.max(WIN_MIN_H, Math.min(maxH, h))];
}

const [initW, initH] = clampWin(
  prefs.monitorChatWidth > 0 ? prefs.monitorChatWidth : WIN_DEF_W,
  prefs.monitorChatHeight > 0 ? prefs.monitorChatHeight : defaultH(),
);
const winW = ref(initW);
const winH = ref(initH);
const resizing = ref(false);
let rsX = 0;
let rsY = 0;
let rsW = 0;
let rsH = 0;

function onGripDown(e: PointerEvent): void {
  resizing.value = true;
  rsX = e.clientX;
  rsY = e.clientY;
  rsW = winW.value;
  rsH = winH.value;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onGripMove(e: PointerEvent): void {
  if (!resizing.value) return;
  if (!(e.buttons & 1)) {
    onGripUp();
    return;
  }
  const [w, h] = clampWin(rsW + (e.clientX - rsX), rsH + (e.clientY - rsY));
  winW.value = w;
  winH.value = h;
}

function onGripUp(): void {
  if (!resizing.value) return;
  resizing.value = false;
  void prefs.setMonitorChatSize(winW.value, winH.value);
}

/** 双击 grip 复原默认尺寸（同时清掉持久化，下次打开仍是默认）。 */
function onGripReset(): void {
  winW.value = WIN_DEF_W;
  winH.value = defaultH();
  void prefs.setMonitorChatSize(0, 0);
}

// ---- 标题栏拖动换位（pointer capture 照 WarPanel；位置写回 store，不持久化） ----

const moving = ref(false);
let mvX = 0;
let mvY = 0;
let mvBaseX = 0;
let mvBaseY = 0;

/** 点击窗口任意处置前（叠放序由 store 维护）。 */
function raise(): void {
  monitor.raiseChatWin(props.sessionId);
}

function clampPos(x: number, y: number): [number, number] {
  // 不拖出可视区：至少留住标题栏一条可抓回。
  const maxX = Math.max(0, window.innerWidth - 160);
  const maxY = Math.max(0, window.innerHeight - 60);
  return [Math.min(Math.max(x, 0), maxX), Math.min(Math.max(y, 0), maxY)];
}

function onHeadDown(e: PointerEvent): void {
  if (e.button !== 0) return;
  const t = e.target as HTMLElement;
  if (t.closest('.cw__full, .cw__close')) return; // 交互元素不触发拖动
  moving.value = true;
  mvX = e.clientX;
  mvY = e.clientY;
  mvBaseX = props.x;
  mvBaseY = props.y;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onHeadMove(e: PointerEvent): void {
  if (!moving.value) return;
  if (!(e.buttons & 1)) {
    onHeadUp();
    return;
  }
  const [x, y] = clampPos(mvBaseX + (e.clientX - mvX), mvBaseY + (e.clientY - mvY));
  monitor.moveChatWin(props.sessionId, x, y);
}

function onHeadUp(): void {
  moving.value = false;
}

// ---- Agent 切换（照 MonitorPage 新会话面板的 usableAgents 口径：enabled） ----

const usableAgents = computed(() => agents.agents.filter((a) => a.enabled));
const currentAgentId = computed(() => meta.value?.agentId ?? '');
const switchingAgent = ref(false);

const agentOptions = computed(() =>
  usableAgents.value.map((a) => a.name + (a.id === agents.defaultAgentId ? ' ★' : '')),
);
const agentIndex = computed(() => usableAgents.value.findIndex((a) => a.id === currentAgentId.value));
/** 当前 Agent 不在可用列表里时，下拉条上仍显示它的名字。 */
const agentDisplay = computed(() =>
  agentIndex.value < 0 && currentAgentId.value ? (meta.value?.agentName ?? '当前') : undefined,
);

async function onAgentPick(i: number): Promise<void> {
  const a = usableAgents.value[i];
  if (!a || a.id === currentAgentId.value || switchingAgent.value) return;
  switchingAgent.value = true;
  try {
    const ok = await monitor.switchAgent(props.sessionId, a.id);
    if (!ok) {
      emit('toast', '切换 Agent 失败');
      return;
    }
    await loadMeta(); // 刷新头部 agent 名与模型只读显示
  } finally {
    switchingAgent.value = false;
  }
}

// ---- 权限请求自动弹审批 ----

const permPending = computed(() => monitor.isPermPending(props.sessionId));
watch(
  permPending,
  (v) => {
    if (v) void monitor.openPermDialog(props.sessionId);
  },
  { immediate: true }, // 打开时已 pending 也立即弹
);

const PERM_MODES: [string, string][] = [
  ['default', '默认'],
  ['plan', '计划'],
  ['auto', '自动'],
  ['yolo', '放任'],
];
const PERM_LABELS = PERM_MODES.map(([, t]) => t);
const permIndex = computed(() => {
  const i = PERM_MODES.findIndex(([v]) => v === permMode.value);
  return i < 0 ? 0 : i;
});

function onPermPick(i: number): void {
  void monitor.setPermMode(props.sessionId, PERM_MODES[i][0]);
}

async function scrollToEnd(): Promise<void> {
  await nextTick();
  if (bodyEl.value) bodyEl.value.scrollTop = bodyEl.value.scrollHeight;
}

async function reload(): Promise<void> {
  const got = await cmd<StoredMessage[]>('session_messages', { sessionId: props.sessionId }, []);
  rows.value = got.map((r) => r as unknown as ChatMessage);
  await scrollToEnd();
}

// ---- 流式事件合并（口径照 chat store onChunk/onTool/onTurn，按 sessionId 过滤；
//      量小直接响应式更新，不做主窗的 DOM 级 streamTarget 优化） ----

/** chunk/tool 的追加目标：进行中的助手行（含打开时整拉回来的 streaming 行），
 *  没有就补一行合成行（chunk 可能先于 busy 状态到达）。 */
function ensureStreamRow(): ChatMessage {
  for (let i = rows.value.length - 1; i >= 0; i--) {
    const r = rows.value[i];
    if (r.role === 'assistant' && (r.status === 'pending' || r.status === 'streaming')) return r;
  }
  const row: ChatMessage = {
    id: `synth-${Date.now()}-${rows.value.length}`,
    role: 'assistant',
    content: '',
    createdAt: Date.now(),
    provider: '',
    status: 'pending',
    thinking: '',
    toolCalls: [],
    segments: [],
    attachments: [],
  };
  rows.value.push(row);
  return row;
}

function onChunk(p: { sessionId: string; kind: string; text: string }): void {
  if (p.sessionId !== props.sessionId) return;
  const last = ensureStreamRow();
  const kind = p.kind === 'thinking' ? 'thinking' : 'text';
  const tail = last.segments[last.segments.length - 1];
  if (tail && tail.kind === kind) tail.text = (tail.text ?? '') + p.text;
  else last.segments.push({ kind, text: p.text });
  void scrollToEnd();
}

function onTool(p: { sessionId: string; tool: Record<string, unknown> }): void {
  if (p.sessionId !== props.sessionId) return;
  const last = ensureStreamRow();
  const id = String(p.tool.toolCallId ?? '');
  const seg = { ...p.tool, kind: 'tool' } as ChatSegment;
  const i = id ? last.segments.findIndex((s) => s.kind === 'tool' && s.toolCallId === id) : -1;
  if (i >= 0) last.segments[i] = seg;
  else last.segments.push(seg);
  void scrollToEnd();
}

function onAppended(p: { sessionId: string; row: ChatMessage }): void {
  if (p.sessionId !== props.sessionId) return;
  const i = rows.value.findIndex((r) => r.id === p.row.id);
  if (i >= 0) rows.value[i] = p.row;
  else rows.value.push(p.row);
  void scrollToEnd();
}

function onBubbleSet(p: { sessionId: string; row: ChatMessage }): void {
  if (p.sessionId !== props.sessionId) return;
  const i = rows.value.findIndex((r) => r.id === p.row.id);
  if (i >= 0) rows.value[i] = p.row;
}

const unlisteners: UnlistenFn[] = [];

async function loadMeta(): Promise<void> {
  meta.value = await cmd<SessionMeta | null>('session_meta', { sessionId: props.sessionId }, null);
}

onMounted(async () => {
  void loadMeta();
  await reload();
  unlisteners.push(
    await listen<{ sessionId: string; kind: string; text: string }>('acp://chunk', (e) => onChunk(e.payload)),
    await listen<{ sessionId: string; tool: Record<string, unknown> }>('acp://tool', (e) => onTool(e.payload)),
    // turn 完成 → 终态已落盘，整拉收尾（含最终 segments/状态）。
    await listen<{ sessionId: string }>('acp://turn', (e) => {
      if (e.payload.sessionId === props.sessionId) void reload();
    }),
    await listen<{ sessionId: string; row: ChatMessage }>('chat://messageAppended', (e) =>
      onAppended(e.payload),
    ),
    await listen<{ sessionId: string; row: ChatMessage }>('chat://bubbleSet', (e) => onBubbleSet(e.payload)),
  );
});
onBeforeUnmount(() => {
  for (const u of unlisteners) u();
});

async function send(): Promise<void> {
  const text = input.value.trim();
  if (!text || sending.value) return;
  sending.value = true;
  try {
    // user 气泡靠 chat://messageAppended 回来渲染（不本地插，与主窗一致）。
    input.value = '';
    const ok = await monitor.sendTo(props.sessionId, text);
    if (!ok) {
      rows.value.push({
        id: `sys-${Date.now()}`,
        role: 'sys',
        content: '⚠ 发送失败，请重试',
        createdAt: Date.now(),
        provider: '',
        status: 'done',
        thinking: '',
        toolCalls: [],
        segments: [],
        attachments: [],
      });
      await scrollToEnd();
    }
  } finally {
    sending.value = false;
  }
}

async function openFull(): Promise<void> {
  emit('close');
  const ok = await chat.openSession(props.sessionId);
  if (!ok) {
    ui.showBanner('无法打开会话');
    return;
  }
  await nav.goOverlay('chat');
}
</script>

<template>
  <div
    class="cw"
    :class="{ resizing, moving }"
    :style="{ left: x + 'px', top: y + 'px', width: winW + 'px', height: winH + 'px', zIndex: z }"
    @pointerdown.capture="raise"
  >
    <!-- frame_popup_small 九宫格窗框（half-scale popup，照 RecentProjectsPanel） -->
    <div class="cw__frame"></div>

    <div class="cw__inner">
      <div
        class="cw__head"
        @pointerdown="onHeadDown"
        @pointermove="onHeadMove"
        @pointerup="onHeadUp"
        @pointercancel="onHeadUp"
      >
        <span class="cw__title" :style="{ fontSize: prefs.fs(15) + 'px' }">🛡 {{ title }}</span>
        <span class="cw__agent" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ agentLine }}</span>
        <span class="cw__full" :style="{ fontSize: prefs.fs(12) + 'px' }" @click="openFull">在完整页面打开 →</span>
        <span class="cw__close" :style="{ fontSize: prefs.fs(14) + 'px' }" @click="emit('close')">✕</span>
      </div>

      <div class="cw__body-wrap">
        <div ref="bodyEl" class="cw__body" @click="onBodyClick">
          <template v-for="row in rows" :key="row.id">
            <div
              v-if="textOf(row) || (row.attachments ?? []).length > 0 || structSegsOf(row).length > 0"
              class="cw__bubble"
              :class="roleClass(row)"
              :style="{ fontSize: prefs.fs(13) + 'px' }"
            >
              <!-- 过程行：非 text 段（思考/工具）折叠为一行，点击开 ProcessDialog -->
              <div
                v-if="structSegsOf(row).length > 0"
                class="cw__proc"
                :style="{ fontSize: prefs.fs(12) + 'px' }"
                @click="openProc(row)"
              >
                <span class="cw__proc-summary">{{ procSummary(row) }}</span>
                <span v-if="isStreaming(row) && activityHint(row)" class="cw__proc-hint">
                  · {{ activityHint(row) }}</span
                >
              </div>
              <!-- 正文：ai 落盘后 markdown，流式中/其它纯文本；user 嵌图才走渲染器 -->
              <template v-if="textOf(row)">
                <div
                  v-if="row.role === 'assistant' && !isStreaming(row)"
                  class="cw__md md-body"
                  v-html="markdownOf(textOf(row))"
                ></div>
                <div
                  v-else-if="row.role === 'user' && userHtml(textOf(row))"
                  class="cw__md md-body"
                  v-html="userHtml(textOf(row))"
                ></div>
                <template v-else>{{ textOf(row) }}</template>
              </template>
              <!-- 附件：图片内嵌（点击系统打开），其余文件名芯片 -->
              <div v-if="visibleAtts(row).length" class="cw__atts">
                <template v-for="(p, j) in visibleAtts(row)" :key="j">
                  <img
                    v-if="isImagePath(p)"
                    class="cw__atts-img"
                    :src="fileSrc(p)"
                    :title="p"
                    draggable="false"
                    @click.stop="openPath(p)"
                  />
                  <div v-else class="cw__atts-chip" :title="p" @click.stop="openPath(p)">
                    <img src="/assets/wc3_extracted/ui/icon-file.png" draggable="false" />
                    <span :style="{ fontSize: prefs.fs(11) + 'px' }">{{ fileName(p) }}</span>
                  </div>
                </template>
              </div>
            </div>
          </template>
          <div v-if="busy" class="cw__bubble sys" :style="{ fontSize: prefs.fs(11) + 'px' }">生成中…</div>
          <div v-if="rows.length === 0 && !busy" class="cw__bubble sys" :style="{ fontSize: prefs.fs(11) + 'px' }">
            （暂无消息，直接输入开始）
          </div>
        </div>
        <WarScrollBar :target="bodyEl" :scale="0.8" />
      </div>

      <div class="cw__composer">
        <textarea
          v-model="input"
          class="cw__input"
          placeholder="输入消息，Enter 发送，Shift+Enter 换行…"
          :style="{ fontSize: prefs.fs(13) + 'px' }"
          @keydown.enter.exact.prevent="send"
        ></textarea>
        <div class="cw__bar">
          <span class="cw__bar-label" :style="{ fontSize: prefs.fs(11) + 'px' }">Agent</span>
          <WarDropdown
            class="cw__dd cw__dd--agent"
            :options="agentOptions"
            :model-value="agentIndex"
            :display-text="agentDisplay"
            :text-size="prefs.fs(12)"
            :row-height="26"
            drop-up
            @activated="onAgentPick"
          />
          <span class="cw__bar-label" :style="{ fontSize: prefs.fs(11) + 'px' }">模型</span>
          <WarDropdown
            class="cw__dd cw__dd--model"
            :options="[modelLabel]"
            :model-value="0"
            :text-size="prefs.fs(12)"
            :row-height="26"
            drop-up
          />
          <span class="cw__bar-label" :style="{ fontSize: prefs.fs(11) + 'px' }">权限</span>
          <WarDropdown
            class="cw__dd cw__dd--perm"
            :options="PERM_LABELS"
            :model-value="permIndex"
            :text-size="prefs.fs(12)"
            :row-height="26"
            drop-up
            @activated="onPermPick"
          />
          <WarButton
            class="cw__send"
            skin="dialog"
            :width="96"
            :art-aspect="5.34"
            text="发送"
            :enabled="!sending"
            @activated="send"
          />
        </div>
      </div>
    </div>

    <!-- 右下角拉伸 grip（拖动改尺寸，双击复原） -->
    <div
      class="cw__grip"
      title="拖动调整窗口大小（双击复原）"
      @pointerdown="onGripDown"
      @pointermove="onGripMove"
      @pointerup="onGripUp"
      @pointercancel="onGripUp"
      @dblclick="onGripReset"
    ></div>

    <!-- 过程详情弹窗（复用 ProcessDialog；流式中也实时） -->
    <ProcessDialog v-model:open="procOpen" :segments="procRow?.segments ?? []" :title="title" />
  </div>
</template>

<style scoped>
.cw {
  position: absolute;
  /* left/top/zIndex 由窗口实例（store chatWins）绑定；居中 translate 已移除 */
  filter: drop-shadow(0 10px 34px #000c);
  font-family: SimSun, serif;
}

/* frame_popup_small 九宫格（slice 44/50/45/50，fill：中心是深蓝纹理） */
.cw__frame {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 44px 50px 45px 50px;
  border-image: url('/assets/ui/frames/frame_popup_small.png') 44 50 45 50 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}

/* 内容落在框的金色内沿里（照 RecentProjectsPanel 的 inset 23/33/24/31） */
.cw__inner {
  position: absolute;
  inset: 25px 35px 26px 33px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.cw__head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 10px;
  padding-bottom: 6px;
  border-bottom: 1px solid #2a3344;
  cursor: move; /* 按住拖动换位 */
  user-select: none;
  touch-action: none;
}

/* 标题栏里的交互元素不触发拖动，保持常规指针 */
.cw__full,
.cw__close {
  cursor: pointer;
  touch-action: auto;
}

.cw__title {
  color: var(--war-gold);
  font-weight: bold;
  text-shadow: 1px 1px 0 #000;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 40%;
}

.cw__agent {
  color: var(--war-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}

.cw__full {
  margin-left: auto;
  flex: none;
  color: #7ec9e0;
  border: 1px solid #7ec9e044;
  padding: 2px 8px;
  border-radius: 2px;
}

.cw__full:hover {
  background: #7ec9e022;
}

.cw__close {
  flex: none;
  color: #b9c4dc;
  padding: 2px 8px;
  border: 1px solid #2a3344;
  border-radius: 2px;
}

.cw__close:hover {
  color: #ff9b8a;
  border-color: #b0552f;
}

/* 会话体 + WC3 滚动条并排 */
.cw__body-wrap {
  flex: 1;
  min-height: 0;
  display: flex;
  gap: 4px;
}

/* 拉伸/拖动中禁用内容交互，避免拖拽途中选中文字/触发点击 */
.cw.resizing .cw__body-wrap,
.cw.resizing .cw__composer,
.cw.moving .cw__body-wrap,
.cw.moving .cw__composer {
  pointer-events: none;
}

.cw__body {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none; /* 原生条隐藏，WC3 WarScrollBar 替代 */
  padding: 4px 6px 4px 2px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

/* 气泡：frame_chat_bubble_body 石框（照 ChatBubble .bubble-body，0.5 倍边框） */
.cw__bubble {
  max-width: 82%;
  padding: 1px 3px;
  line-height: 1.6;
  border-style: solid;
  border-color: transparent;
  border-width: 7px 8px;
  border-image: url('/assets/ui/frames/frame_chat_bubble_body.png') 14 16 14 16 fill repeat;
  box-sizing: border-box;
  background: var(--war-glass);
  background-clip: padding-box;
  user-select: text;
  white-space: pre-wrap;
  overflow-wrap: break-word;
}

.cw__bubble.user {
  align-self: flex-end;
  background: #2b3a5088;
  background-clip: padding-box;
  color: #e8d9a0;
}

.cw__bubble.ai {
  align-self: flex-start;
  color: #b9c4dc;
}

.cw__bubble.sys {
  align-self: center;
  background: none;
  border: none;
  color: var(--war-text-muted);
  font-size: 11px;
  padding: 0;
}

/* ---- 过程行（移植 ChatBubble .seg-proc 风格，单行摘要 + 流式动作提示） ---- */
.cw__proc {
  display: flex;
  align-items: baseline;
  min-width: 0;
  margin: 0 0 4px;
  padding: 2px 10px;
  background: #12151c44;
  border: 1px solid #3a4a40;
  border-radius: 2px;
  color: #d0d6e0;
  user-select: none;
  cursor: pointer;
  overflow: hidden;
  white-space: nowrap;
}

.cw__proc:hover {
  color: var(--war-gold);
  border-color: var(--war-gold-dim);
}

.cw__proc-summary,
.cw__proc-hint {
  flex: 0 1 auto;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.cw__composer {
  flex: none;
  border-top: 1px solid #2a3344;
  padding-top: 8px;
}

.cw__input {
  width: 100%;
  height: 56px;
  background: #0a0d14;
  border: 1px solid #2a3344;
  border-radius: 3px;
  color: #e8d9a0;
  font-family: SimSun, serif;
  padding: 7px;
  resize: none;
  outline: none;
  user-select: text;
  box-sizing: border-box;
  scrollbar-width: none;
}

.cw__input:focus {
  border-color: #c9a22766;
}

.cw__bar {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 8px;
  align-items: center;
}

.cw__bar-label {
  color: var(--war-text-muted);
  flex: none;
}

/* WarDropdown 默认 140×32，这里按底栏空间收窄 */
.cw__dd {
  flex: none;
  height: 30px;
}

.cw__dd--agent {
  width: 112px;
}

.cw__dd--model {
  width: 96px;
}

.cw__dd--perm {
  width: 76px;
}

.cw__send {
  margin-left: auto;
  flex: none;
}

/* 右下角拉伸 grip：金色斜纹 */
.cw__grip {
  position: absolute;
  right: 10px;
  bottom: 9px;
  width: 24px;
  height: 24px;
  z-index: 5;
  cursor: nwse-resize;
  touch-action: none;
  border-bottom-right-radius: 6px;
  background: repeating-linear-gradient(
    135deg,
    transparent 0 5px,
    #c9a22766 5px 7px,
    transparent 7px 12px
  );
  opacity: 0.75;
}

.cw__grip:hover {
  opacity: 1;
  background: repeating-linear-gradient(
    135deg,
    transparent 0 5px,
    #f5c452aa 5px 7px,
    transparent 7px 12px
  );
}

/* ---- markdown 气泡体（v-html；内容规则在下面 unscoped 块） ---- */
.cw__md {
  white-space: normal; /* 气泡 pre-wrap 只服务纯文本路径 */
  user-select: text;
}

/* ---- 附件（照 ChatBubble bubble-atts） ---- */
.cw__atts {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 6px;
}

.cw__atts-img {
  max-width: 100%;
  max-height: 240px;
  object-fit: contain;
  border-radius: 2px;
  border: 1px solid #4a5b75;
}

.cw__atts-chip {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: 200px;
  padding: 4px 8px;
  background: #1a2334;
  border: 1px solid #2c4a7a;
  border-radius: 2px;
}

.cw__atts-chip:hover {
  border-color: #4a5b75;
}

.cw__atts-chip img {
  width: 15px;
  height: 15px;
  flex: none;
}

.cw__atts-chip span {
  color: #c0d0ec;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>

<style>
/* 小窗 markdown 内容（unscoped：v-html 渲染产物没有 scoped 属性）。规则
   照 ChatBubble 的全局 .md-body 块（war 金色调），命名空间收在 .cw 下避免
   与聊天页互相影响；pre 横向滚动、图片随气泡宽度自适应（窗口可拉伸）。 */
.cw .md-body > :first-child {
  margin-top: 0;
}
.cw .md-body > :last-child {
  margin-bottom: 0;
}
.cw .md-body p {
  margin: 4px 0;
  line-height: 1.5;
}
.cw .md-body strong,
.cw .md-body b {
  color: var(--war-gold-bright);
}
.cw .md-body em,
.cw .md-body i {
  color: var(--war-gold);
}
.cw .md-body pre {
  background: #00000070;
  border: 1px solid var(--war-gold-input);
  border-radius: 2px;
  padding: 5px 7px;
  overflow-x: auto;
  font-family: Consolas, monospace;
  font-size: 0.92em;
}
.cw .md-body .codeblock {
  position: relative;
}
.cw .md-body .codeblock pre {
  margin: 4px 0;
}
.cw .md-body .codeblock__copy {
  position: absolute;
  top: 8px;
  right: 4px;
  z-index: 1;
  padding: 0 6px;
  background: #12151cbb;
  border: 1px solid var(--war-gold-dim);
  border-radius: 2px;
  color: #a0a8b8;
  font-family: SimSun, serif;
  font-size: 11px;
  opacity: 0;
  transition: opacity 0.15s;
}
.cw .md-body .codeblock:hover .codeblock__copy {
  opacity: 1;
}
.cw .md-body .codeblock__copy:hover {
  color: var(--war-gold-bright);
  border-color: var(--war-gold);
}
.cw .md-body code {
  font-family: Consolas, monospace;
  background: #00000050;
  color: var(--war-gold-bright);
  padding: 0 3px;
  border-radius: 2px;
  font-size: 0.92em;
}
.cw .md-body pre code {
  background: none;
  color: var(--war-text);
  padding: 0;
}
.cw .md-body ul,
.cw .md-body ol {
  margin: 4px 0;
  padding-left: 20px;
}
.cw .md-body li::marker {
  color: var(--war-gold-dim);
}
.cw .md-body h1,
.cw .md-body h2,
.cw .md-body h3,
.cw .md-body h4 {
  margin: 8px 0 4px;
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-weight: bold;
}
.cw .md-body h1 { font-size: 1.25em; }
.cw .md-body h2 { font-size: 1.15em; }
.cw .md-body h3 { font-size: 1.05em; }
.cw .md-body blockquote {
  margin: 4px 0;
  padding-left: 8px;
  border-left: 3px solid var(--war-gold-dim);
  color: var(--war-text-muted);
}
.cw .md-body a {
  color: var(--war-gold-bright);
  text-decoration: underline;
}
.cw .md-body a:hover {
  color: var(--war-gold);
}
.cw .md-body img {
  max-width: 100%;
  max-height: 320px;
  object-fit: contain;
  border-radius: 2px;
}
.cw .md-body table {
  border-collapse: collapse;
  margin: 6px 0;
}
.cw .md-body th,
.cw .md-body td {
  border: 1px solid #2a3344;
  padding: 2px 8px;
}
.cw .md-body th {
  color: var(--war-gold);
}
.cw .md-body hr {
  border: none;
  border-top: 1px solid #2a3344;
  margin: 8px 0;
}
</style>

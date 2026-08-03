<script setup lang="ts">
// Chat bubble (ui-design.md §4.7, features/chat.md §2.2).
//
// R1 streaming contract: while `streaming` is true, text segments render as
// PLAIN TEXT and the trailing text segment's DOM node is registered with the
// chat store (registerStreamTarget) so chunks append directly to the text
// node — this component does NOT re-render per chunk (the segment text
// mutation is in-place and non-reactive). When the turn goes final the row
// object is replaced, the component re-renders once, and text segments
// switch to memoized markdown HTML.
//
// Streaming and final rows share ONE step display: thinking/tool segments
// collapse to a single "⚙ N 个步骤" line; clicking opens ProcessDialog with
// the live step list. While streaming the line appends a current-activity
// hint (思考中… / last tool) and a random flavor line (FLAVOR_LINES).
import { computed, onBeforeUnmount, onMounted, onUpdated, ref, watch } from 'vue';
import { fileSrc, openPath } from '../../lib/tauri';
import { renderMarkdown, renderUserMarkdown, renderQuoteHighlight, handleMdLinkClick } from '../../lib/markdown';
import { copyText } from '../../lib/clipboard';
import { usePrefsStore } from '../../stores/prefs';
import { useChatStore, type ChatMessage, type ChatSegment } from '../../stores/chat';
import { MAX_SUB_DEPTH } from '../../stores/chat';
import { formatTokens } from '../../lib/format';
import ProcessDialog from './ProcessDialog.vue';
import WarMenu from '../war/WarMenu.vue';

const props = defineProps<{
  row: ChatMessage;
  /** True while this row is the turn's streaming target (last assistant). */
  streaming: boolean;
  displayName: string;
  avatarUrl: string;
}>();

const prefs = usePrefsStore();
const chat = useChatStore();

const isUser = computed(() => props.row.role === 'user');
/** Reminder-triggered user rows render as a centered system line (no avatar,
 * no bubble frame, no branch button); copy is not offered there. */
const isReminder = computed(() => props.row.kind === 'reminder');
/** Terminal-command rows (`!` prefix, cmd.rs) render as a full-width block:
 * command header + streaming/final output, no avatar. */
const isCommand = computed(() => props.row.kind === 'command');
const fs = (n: number) => prefs.fs(n);

// ---- command row helpers ----
const termRunning = computed(() => isCommand.value && props.row.status === 'streaming');
/** Output = the row's text segments (content keeps the command itself).
 * Deliberately a METHOD, not a computed: segments are mutated in place by
 * the store (R1, non-reactive) so a computed would cache the mount-time
 * value and clobber the live DOM text on any re-render. Reading fresh per
 * render keeps the text node in sync with the store. */
function termOutputText(): string {
  return (props.row.segments ?? [])
    .filter((s) => s.kind === 'text')
    .map((s) => s.text ?? '')
    .join('');
}
const termCode = computed(() => props.row.exitCode ?? undefined);
/** runId of the live run (cancel button); undefined = no live run / stale. */
const runId = computed(() => (isCommand.value ? chat.runsByRow[props.row.id] : undefined));
const termStatusLabel = computed(() => {
  if (termRunning.value) return '运行中…';
  if (props.row.status === 'interrupted') return '已中断';
  if (props.row.status === 'error') return '失败';
  return '';
});
const termStatusCls = computed(() => {
  if (props.row.status === 'error') return 'err';
  if (props.row.status === 'interrupted') return 'int';
  return '';
});

function killTerm(): void {
  if (runId.value) void chat.killRun(runId.value);
}

/** Command output is collapsed by default; the header's "▶ 输出" toggles
 * it. While collapsed the output keeps accumulating (store + backend); the
 * <pre> renders the full text on expand. */
const outOpen = ref(false);

function toggleOut(): void {
  outOpen.value = !outOpen.value;
}

// ---- status / header ----

const isError = computed(() => props.row.status === 'error');
const isInterrupted = computed(() => props.row.status === 'interrupted');

const statusLabel = computed(() => {
  if (props.streaming) return '生成中…';
  if (isError.value) return '错误';
  if (isInterrupted.value) return '已中断';
  return '';
});
const statusColor = computed(() => {
  if (props.streaming) return 'var(--war-gold)';
  if (isError.value) return 'var(--war-error)';
  return 'var(--war-interrupted)';
});

const timeText = computed(() => {
  const d = new Date(props.row.createdAt);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
});

// ---- token usage (assistant, final turns only) ----
const usage = computed(() =>
  !isUser.value && !props.streaming ? props.row.usage : undefined,
);

// ---- segments ----

const segs = computed<ChatSegment[]>(() => props.row.segments ?? []);
const hasStructSegs = computed(() => segs.value.some((s) => s.kind !== 'text'));
/** Streaming or structured bubbles pin to max width (no frame-width jitter). */
const pinWide = computed(() => props.streaming || hasStructSegs.value);

// ---- plan card (ACP plan updates, toolCallId "plan") ----
// Plan segments ride the tool-segment channel but render as a visible
// checklist card — inline in streaming AND final rows, never folded into
// the process line.
const isPlan = (s: ChatSegment): boolean => s.toolCallId === 'plan';

interface PlanEntry {
  content?: string;
  status?: string;
  priority?: string;
}

function planEntries(s: ChatSegment): PlanEntry[] {
  return Array.isArray(s.entries) ? (s.entries as PlanEntry[]) : [];
}

function planIcon(status?: string): string {
  if (status === 'completed') return '✓';
  if (status === 'in_progress') return '▶';
  return '○'; // pending / unknown
}

const planSegs = computed(() =>
  segs.value.map((s, i) => ({ s, i })).filter(({ s }) => s.kind === 'tool' && isPlan(s)),
);

// ---- process line (streaming AND final rows share one step display) ----
// The FINAL text segment (when it is the tail) is the "result" shown inline;
// any earlier text is process prose and folds into the steps (⚙ line +
// ProcessDialog) together with thinking/tool segments. Clicking the line
// opens ProcessDialog with the step list (live while streaming). Streaming
// rows append a current-activity hint and a random flavor line.
const resultText = computed<ChatSegment | null>(() => {
  const tail = segs.value[segs.value.length - 1];
  return tail?.kind === 'text' ? tail : null;
});
const structSegs = computed(() =>
  segs.value.filter((s) => s !== resultText.value && !isPlan(s)),
);
const textSegs = computed(() =>
  resultText.value ? [{ s: resultText.value, i: segs.value.length - 1 }] : [],
);
const procOpen = ref(false);
const procSummary = computed(() => {
  const think = structSegs.value.filter((s) => s.kind === 'thinking').length;
  const tools = structSegs.value.filter((s) => s.kind === 'tool').length;
  const texts = structSegs.value.filter((s) => s.kind === 'text').length;
  const parts: string[] = [];
  if (think > 0) parts.push(`思考×${think}`);
  if (tools > 0) parts.push(`工具×${tools}`);
  if (texts > 0) parts.push(`过程正文×${texts}`);
  return `⚙ ${structSegs.value.length} 个步骤（${parts.join(' · ')}）`;
});

/** Streaming-only: what the agent is doing right now (tail segment). */
// Tool titles from ACP can carry the whole command (opencode titles often
// are the invocation) — elide them so the process line stays short; the CSS
// nowrap/ellipsis on .seg-proc is the hard backstop, the full name is in the
// process dialog.
const TOOL_HINT_MAX = 24;
function elideToolName(s: string): string {
  return s.length <= TOOL_HINT_MAX ? s : s.slice(0, TOOL_HINT_MAX) + '…';
}
const activityHint = computed(() => {
  if (!props.streaming) return '';
  const tail = segs.value[segs.value.length - 1];
  if (!tail) return '';
  if (tail.kind === 'thinking') return '思考中…';
  if (tail.kind === 'tool') {
    const st = tail.status ? ` [${tail.status}]` : '';
    return `▶ ${elideToolName(toolName(tail))}${st}`;
  }
  return '';
});

// Streaming-only flavor lines: a random quote loaded from the Warcraft quotes
// file (public/assets/Warcraft3-Quotes/…), shown on its OWN second line of the
// process pill, re-picked on every structural event (new segment / tool
// upsert) — its left edge never moves, so no layout jitter. Loaded once at
// runtime and cached module-wide; built-in lines cover the window while the
// file loads (or if it is missing).
const QUOTES_URL =
  '/assets/Warcraft3-Quotes/' + encodeURIComponent('魔兽争霸3角色台词-中文.md');
const FLAVOR_LINES = [
  '战争已经打响。',
  '力量与荣耀！',
  '为了联盟！',
  '为了部落！',
  '号角已吹响。',
];
let quotesCache: Promise<string[]> | null = null;
function loadQuotes(): Promise<string[]> {
  if (!quotesCache) {
    quotesCache = fetch(QUOTES_URL)
      .then((r) => (r.ok ? r.text() : Promise.reject(new Error(`HTTP ${r.status}`))))
      .then((text) =>
        text
          .split(/\r?\n/)
          .map((l) => l.trim())
          .filter((l) => l && !l.startsWith('#') && !l.startsWith('>') && l.includes('：')),
      );
  }
  return quotesCache;
}
const flavorLines = ref<string[]>(FLAVOR_LINES);
const flavor = ref('');
let lastFlavorIdx = -1;
function pickFlavor(): void {
  const lines = flavorLines.value;
  if (lines.length === 0) return;
  let idx = Math.floor(Math.random() * lines.length);
  if (lines.length > 1 && idx === lastFlavorIdx) idx = (idx + 1) % lines.length;
  lastFlavorIdx = idx;
  flavor.value = lines[idx];
}
void loadQuotes()
  .then((lines) => {
    if (lines.length === 0) return;
    flavorLines.value = lines;
    if (props.streaming && structSegs.value.length > 0) pickFlavor();
  })
  .catch(() => {
    /* keep built-in fallback lines */
  });
watch(
  () => segs.value,
  () => {
    if (!props.streaming || structSegs.value.length === 0) return;
    pickFlavor();
  },
  { immediate: true },
);

/** Placeholder "…" during streaming; treated as empty once final. */
const displayBody = computed(() => {
  const c = props.row.content ?? '';
  if (c === '…' && !props.streaming) return '';
  return c;
});

function toolName(s: ChatSegment): string {
  return String(s.name || s.title || s.kind || 'tool');
}

// ---- markdown: rendered once per finished segment, memoized by content ----
// Per-bubble markdown switch (一.9): rendered by default; 原文 shows the raw
// text. Component-local on purpose.
const mdEnabled = ref(true);

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

// ---- stream target registration (R1 incremental DOM append) ----
const bodyEl = ref<HTMLElement | null>(null);

function registerLive(): void {
  if (!props.streaming || !bodyEl.value) return;
  const el = bodyEl.value.querySelector<HTMLElement>('[data-live-seg]');
  const tail = segs.value[segs.value.length - 1];
  if (el && tail && (tail.kind === 'text' || tail.kind === 'thinking')) {
    chat.registerStreamTarget(props.row.id, tail.kind, el);
  }
}
onMounted(registerLive);
onUpdated(registerLive);
onBeforeUnmount(() => chat.registerStreamTarget(props.row.id, '', null));

// ---- command row live output registration (term://output append) ----
// While the run streams, the output <pre> is the DOM append target; on
// mount/re-render it is (re)registered, on unmount / row finalization it is
// released.
const termOutEl = ref<HTMLElement | null>(null);

function syncTermTarget(): void {
  if (isCommand.value && termRunning.value && outOpen.value) {
    chat.registerTermTarget(props.row.id, termOutEl.value);
  } else {
    chat.registerTermTarget(props.row.id, null);
  }
}
onMounted(syncTermTarget);
onUpdated(syncTermTarget);
watch(outOpen, syncTermTarget);
onBeforeUnmount(() => chat.registerTermTarget(props.row.id, null));

// ---- inline-image lightbox (markdown ![](…) embeds) ----
// v-html content can't carry Vue handlers, so clicks are delegated from the
// bubble body: any <img> inside .md-body opens the fullscreen preview.
const lightboxSrc = ref('');

function onBodyClick(e: MouseEvent): void {
  const t = e.target as HTMLElement;
  if (t.tagName === 'IMG' && t.closest('.md-body')) {
    lightboxSrc.value = (t as HTMLImageElement).src;
    return;
  }
  // code-block copy button (rendered into .md-body by markdown.ts)
  const btn = t.closest<HTMLElement>('.codeblock__copy');
  if (btn && t.closest('.md-body')) {
    const text = btn.parentElement?.querySelector('pre')?.innerText ?? '';
    if (!text.trim()) return;
    void copyText(text).then((ok) => {
      if (!ok) return;
      btn.textContent = '已复制';
      setTimeout(() => (btn.textContent = '复制'), 1200);
    });
    return;
  }
  // links (rendered by markdown.ts): never let the webview navigate — open
  // http(s) in the system browser, local paths with the OS handler.
  if (t.closest('.md-body') && handleMdLinkClick(e)) return;
}

function onLightboxKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.stopPropagation();
    lightboxSrc.value = '';
  }
}
watch(lightboxSrc, (v) => {
  if (v) window.addEventListener('keydown', onLightboxKey, true);
  else window.removeEventListener('keydown', onLightboxKey, true);
});
onBeforeUnmount(() => window.removeEventListener('keydown', onLightboxKey, true));

// ---- copy (lazy: full text assembled only on click, R1) ----
const copied = ref(false);
let copyTimer: ReturnType<typeof setTimeout> | null = null;
async function copyBody(): Promise<void> {
  const text =
    segs.value.length > 0
      ? segs.value
          .filter((s) => s.kind === 'text')
          .map((s) => s.text ?? '')
          .join('')
      : displayBody.value;
  if (!text.trim()) return;
  if (await copyText(text)) {
    copied.value = true;
    if (copyTimer) clearTimeout(copyTimer);
    copyTimer = setTimeout(() => (copied.value = false), 1200);
  }
}
onBeforeUnmount(() => {
  if (copyTimer) clearTimeout(copyTimer);
});

// ---- branch (fork the session at this user message) ----
function branchHere(): void {
  void chat.branchFromMessage(props.row.id);
}

// ---- selection context menu (右选 → 右键 → 基于选中文本提问) ----
// Applies to user AND assistant bubbles; streaming rows are dead (the
// streaming contract forbids touching the live text, and the selection is
// meaningless mid-generation). The selection must be non-empty and anchored
// INSIDE this bubble — a cross-bubble drag only enables 复制.
const ctxOpen = ref(false);
const ctxX = ref(0);
const ctxY = ref(0);
const ctxSel = ref('');
const ctxAskable = ref(false);

function onBubbleContextMenu(e: MouseEvent): void {
  if (props.streaming || isCommand.value || isReminder.value) return;
  const sel = window.getSelection();
  const text = sel?.toString().trim() ?? '';
  const body = bodyEl.value;
  if (!body) return;
  const inside = (n: Node | null): boolean =>
    n !== null && (n === body || body.contains(n));
  // 无选区 / 选区锚点不在本气泡内 / 已达子会话层级上限 → 提问禁用
  const askable =
    text.length > 0 &&
    !!sel &&
    inside(sel.anchorNode) &&
    inside(sel.focusNode) &&
    chat.sessionDepth() < MAX_SUB_DEPTH;
  ctxSel.value = text;
  ctxAskable.value = askable;
  ctxX.value = Math.min(e.clientX, window.innerWidth - 240);
  ctxY.value = Math.min(e.clientY, window.innerHeight - 110);
  ctxOpen.value = true;
}

function onCtxSelect(i: number): void {
  ctxOpen.value = false;
  if (i === 0) {
    if (ctxSel.value) void copyText(ctxSel.value);
    return;
  }
  if (i === 1 && ctxAskable.value) {
    void chat.askOnSelection(props.row.id, ctxSel.value);
  }
}

// ---- long user message fold (四.8) ----
// Pasted logs etc.: over ~15 lines or ~800 chars the bubble clamps to the
// first lines with a 展开全部 toggle. User bubbles only; component-local.
const USER_FOLD_LINES = 15;
const USER_FOLD_CHARS = 800;
const userExpanded = ref(false);
const userLong = computed(() => {
  if (!isUser.value) return false;
  const t = displayBody.value;
  return t.length > USER_FOLD_CHARS || t.split('\n').length > USER_FOLD_LINES;
});

// ---- attachments ----
const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'];
function isImagePath(p: string): boolean {
  const ext = p.split('.').pop()?.toLowerCase() ?? '';
  return IMAGE_EXTS.includes(ext);
}
function fileName(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

// ---- user pasted-image markdown (Composer inserts ![name](<path>)) ----
// User text stays plain EXCEPT when it embeds an image — only then does it
// go through the breaks:true user renderer (renderUserMarkdown).
const MD_IMG_RE = /!\[[^\]]*\]\(/;
const userMdHtml = computed(() =>
  isUser.value && MD_IMG_RE.test(displayBody.value) ? renderUserMarkdown(displayBody.value) : '',
);

// <selection>…</selection> quote blocks (sent by the sub-session composer):
// render as a gold highlight span, same look as the composer overlay.
const QUOTE_RE = /<selection>[\s\S]*?<\/selection>/;
const userQuoteHtml = computed(() =>
  isUser.value && QUOTE_RE.test(displayBody.value) ? renderQuoteHighlight(displayBody.value) : '',
);

// Attachments already embedded in the message text as markdown images are
// not repeated as thumbnails below the bubble. The embed normalizes '\' to
// '/' (markdown destinations), so compare slash-insensitively.
const visibleAtts = computed(() =>
  (props.row.attachments ?? []).filter(
    (p) => !displayBody.value.includes(p.replace(/\\/g, '/')),
  ),
);
</script>

<template>
  <!-- terminal-command row (`!` prefix): full-width block, no avatar -->
  <div v-if="isCommand" class="bubble-term">
    <div class="bubble-term__head">
      <span class="bubble-term__prompt" :style="{ fontSize: fs(12) + 'px' }">&gt;</span>
      <span class="bubble-term__cmd" :style="{ fontSize: fs(12) + 'px' }">{{ props.row.content }}</span>
      <span
        v-if="termStatusLabel"
        class="bubble-term__status"
        :class="termStatusCls"
        :style="{ fontSize: fs(11) + 'px' }"
        >{{ termStatusLabel }}</span
      >
      <span
        v-if="!termRunning && termCode !== undefined"
        class="bubble-term__code"
        :style="{ fontSize: fs(11) + 'px' }"
        >退出码 {{ termCode }}</span
      >
      <span
        v-if="termRunning && runId"
        class="bubble-term__kill"
        :style="{ fontSize: fs(11) + 'px' }"
        title="终止命令（含子进程）"
        @click="killTerm"
        >取消</span
      >
      <span
        class="bubble-term__toggle"
        :style="{ fontSize: fs(11) + 'px' }"
        title="展开/收起命令输出"
        @click="toggleOut"
        >{{ outOpen ? '▼ 收起' : '▶ 输出' }}</span
      >
      <span class="bubble-term__time" :style="{ fontSize: fs(11) + 'px' }">{{ timeText }}</span>
    </div>
    <pre v-if="outOpen" ref="termOutEl" class="bubble-term__out" :style="{ fontSize: fs(12) + 'px' }">{{
      termOutputText()
    }}</pre>
  </div>

  <!-- reminder-triggered user row: centered system line, no avatar/branch -->
  <div v-else-if="isReminder" class="bubble-sys">
    <span class="bubble-sys__text" :style="{ fontSize: fs(12) + 'px' }">
      <span class="bubble-sys__label">提醒</span>{{ displayBody }}
    </span>
    <span class="bubble-sys__time" :style="{ fontSize: fs(11) + 'px' }">{{ timeText }}</span>
  </div>
  <div v-else class="bubble-row" :class="{ user: isUser }">
    <div class="bubble-group" :class="{ 'pin-wide': pinWide }">
      <!-- avatar slot: OUTSIDE the body frame, pinned to its top (64×58) -->
      <div class="bubble-slot" :class="{ user: isUser }">
        <img class="bubble-slot__frame" src="/assets/ui/frames/frame_chat_bubble_slot.png" draggable="false" />
        <div class="bubble-slot__avatar">
          <img :src="avatarUrl" draggable="false" />
        </div>
      </div>

      <div
        ref="bodyEl"
        class="bubble-body"
        :class="{ error: isError, streaming: props.streaming }"
        :style="{ fontSize: fs(14) + 'px' }"
        @click="onBodyClick"
        @contextmenu.prevent="onBubbleContextMenu"
      >
        <!-- header: name · time · status · copy -->
        <div class="bubble-head" :class="{ user: isUser }">
          <span
            class="bubble-head__name war-outline-black"
            :class="{ user: isUser }"
            :style="{ fontSize: fs(12) + 'px' }"
            >{{ displayName }}</span
          >
          <span class="bubble-head__time" :style="{ fontSize: fs(11) + 'px' }">{{ timeText }}</span>
          <span
            v-if="statusLabel"
            class="bubble-head__status"
            :style="{ fontSize: fs(11) + 'px', color: statusColor }"
            >{{ statusLabel }}</span
          >
          <span
            v-if="usage"
            class="bubble-head__usage"
            :style="{ fontSize: fs(11) + 'px' }"
          >
            <span class="bubble-head__usage-in">↑</span
            ><span class="bubble-head__usage-num">{{ formatTokens(usage.inputTokens) }}</span>
            <span class="bubble-head__usage-out">↓</span
            ><span class="bubble-head__usage-num">{{ formatTokens(usage.outputTokens) }}</span>
          </span>
          <span
            v-if="!isUser && !streaming"
            class="bubble-head__md"
            :style="{ fontSize: fs(11) + 'px' }"
            @click="mdEnabled = !mdEnabled"
            >{{ mdEnabled ? '原文' : '渲染' }}</span
          >
          <span
            v-if="isUser"
            class="bubble-head__branch"
            :style="{ fontSize: fs(11) + 'px' }"
            title="从此消息分支新会话"
            @click="branchHere"
            >分支</span
          >
          <span
            class="bubble-head__copy"
            :class="{ copied }"
            :style="{ fontSize: fs(11) + 'px' }"
            @click="copyBody"
            >{{ copied ? '已复制' : '复制' }}</span
          >
        </div>

        <!-- segments in arrival order: thinking/tool collapse to ONE process
             line (streaming AND final alike); the dialog holds the live step
             list. Text stays inline; the trailing text span carries
             data-live-seg (R1 append target). -->
        <template v-if="segs.length > 0">
          <div
            v-if="structSegs.length > 0"
            class="seg-proc"
            :style="{ fontSize: fs(12) + 'px' }"
            @click="procOpen = true"
          >
            <div class="seg-proc__main">
              <span class="seg-proc__summary">{{ procSummary }}</span>
              <span v-if="streaming && activityHint" class="seg-proc__hint"> · {{ activityHint }}</span>
            </div>
            <div v-if="streaming && flavor" class="seg-proc__flavor">{{ flavor }}</div>
          </div>
          <div v-for="{ s, i } in textSegs" :key="i" class="seg-text">
            <span
              v-if="streaming || !mdEnabled"
              class="seg-text__plain"
              :data-live-seg="streaming && i === segs.length - 1 ? '' : undefined"
              >{{ s.text }}</span
            >
            <div v-else class="seg-text__md md-body" v-html="markdownOf(s.text ?? '')"></div>
          </div>
          <!-- plan updates stay visible (checklist card) -->
          <div v-for="{ s, i } in planSegs" :key="'plan' + i" class="seg-plan">
            <div class="seg-plan__title" :style="{ fontSize: fs(12) + 'px' }">计划</div>
            <div
              v-for="(e, j) in planEntries(s)"
              :key="j"
              class="seg-plan__row"
              :style="{ fontSize: fs(12) + 'px' }"
            >
              <span class="seg-plan__icon" :class="{ done: e.status === 'completed' }">{{ planIcon(e.status) }}</span>
              {{ e.content }}
            </div>
          </div>
          <ProcessDialog v-model:open="procOpen" :segments="structSegs" :title="displayName + ' · ' + timeText" />
        </template>

        <!-- fallback: user rows and the pending placeholder have no segments -->
        <div v-else-if="displayBody" class="seg-text">
          <div
            v-if="userQuoteHtml"
            class="seg-text__md md-body quote-body"
            v-html="userQuoteHtml"
          ></div>
          <div
            v-else-if="userMdHtml"
            class="seg-text__md md-body"
            :class="{ 'user-clamped': userLong && !userExpanded }"
            v-html="userMdHtml"
          ></div>
          <div
            v-else-if="!isUser && mdEnabled"
            class="seg-text__md md-body"
            v-html="markdownOf(displayBody)"
          ></div>
          <span
            v-else
            class="seg-text__plain"
            :class="{ 'err-text': isError, 'user-clamped': userLong && !userExpanded }"
            >{{ displayBody }}</span
          >
          <div
            v-if="userLong"
            class="user-fold"
            :style="{ fontSize: fs(11) + 'px' }"
            @click="userExpanded = !userExpanded"
          >
            {{ userExpanded ? '▲ 收起' : '▼ 展开全部' }}
          </div>
        </div>

        <!-- user attachments (paths already embedded as markdown images are skipped) -->
        <div v-if="visibleAtts.length" class="bubble-atts">
          <template v-for="(p, i) in visibleAtts" :key="i">
            <img
              v-if="isImagePath(p)"
              class="bubble-atts__img"
              :src="fileSrc(p)"
              :title="p"
              draggable="false"
              @click="openPath(p)"
            />
            <div v-else class="bubble-atts__chip" :title="p" @click="openPath(p)">
              <img src="/assets/wc3_extracted/ui/icon-file.png" draggable="false" />
              <span :style="{ fontSize: fs(11) + 'px' }">{{ fileName(p) }}</span>
            </div>
          </template>
        </div>

        <!-- inline-image lightbox -->
        <Teleport to="body">
          <div v-if="lightboxSrc" class="imglb" @click="lightboxSrc = ''">
            <img class="imglb__img" :src="lightboxSrc" draggable="false" />
            <span class="imglb__hint" :style="{ fontSize: fs(11) + 'px' }">点击任意处或 Esc 关闭</span>
          </div>
        </Teleport>
      </div>
    </div>
  </div>

  <!-- selection context menu: copy + fork a sub-session at this bubble -->
  <WarMenu
    v-model:visible="ctxOpen"
    :x="ctxX"
    :y="ctxY"
    :items="[
      { label: '复制', disabled: !ctxSel },
      { label: '基于选中文本提问（新会话）', disabled: !ctxAskable },
    ]"
    @select="onCtxSelect"
  />
</template>

<style scoped>
/* reminder system line: centered pill, muted colors from the war palette */
.bubble-sys {
  display: flex;
  justify-content: center;
  align-items: baseline;
  gap: 8px;
  padding: 4px 10px;
}

.bubble-sys__text {
  font-family: SimSun, serif;
  color: var(--war-text-muted);
  background: #12151c44;
  border: 1px solid var(--war-gold-dim);
  border-radius: 10px;
  padding: 2px 12px;
  opacity: 0.85;
  user-select: text;
}

.bubble-sys__label {
  color: var(--war-gold);
  margin-right: 6px;
}

.bubble-sys__time {
  color: var(--war-text-faint);
  user-select: none;
}

/* terminal-command block (`!` prefix): full-width, terminal green accents */
.bubble-term {
  width: 100%;
  border: 1px solid #3f7a52;
  background: #0d1116f0;
  border-radius: 3px;
  margin: 2px 0;
  font-family: Consolas, monospace;
}

.bubble-term__head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 10px;
  border-bottom: 1px solid #1a2230;
}

.bubble-term__prompt {
  color: #5cb380;
  font-weight: bold;
  user-select: none;
}

.bubble-term__cmd {
  flex: 1;
  min-width: 0;
  color: var(--war-gold);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  user-select: text;
}

.bubble-term__status {
  color: #5cb380;
  flex: none;
  user-select: none;
}

.bubble-term__status.err {
  color: var(--war-error);
}

.bubble-term__status.int {
  color: var(--war-interrupted);
}

.bubble-term__code {
  color: var(--war-text-faint);
  flex: none;
  user-select: none;
}

.bubble-term__kill {
  color: #ff8a70;
  flex: none;
  user-select: none;
  padding: 0 4px;
}

.bubble-term__kill:hover {
  color: var(--war-error);
}

.bubble-term__toggle {
  color: var(--war-text-muted);
  flex: none;
  user-select: none;
  padding: 0 4px;
}

.bubble-term__toggle:hover {
  color: var(--war-gold);
}

.bubble-term__time {
  color: var(--war-text-faint);
  flex: none;
  user-select: none;
}

.bubble-term__out {
  margin: 0;
  padding: 6px 10px;
  white-space: pre-wrap;
  overflow-wrap: break-word;
  color: #c8d6c8;
  user-select: text;
}

.bubble-row {
  display: flex;
  justify-content: flex-start;
  padding: 4px 10px;
}

.bubble-row.user {
  justify-content: flex-end;
}

.bubble-group {
  position: relative;
  display: flex;
  align-items: flex-start;
  gap: 3px; /* slotGap */
  max-width: 82%;
  width: fit-content;
  min-width: 140px;
}

.bubble-group.pin-wide {
  width: 100%;
}

.bubble-row.user .bubble-group {
  flex-direction: row-reverse;
}

/* ---- avatar slot ---- */
.bubble-slot {
  position: relative;
  flex: none;
  width: 64px;
  height: 58px;
}

.bubble-slot__frame {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: contain; /* PreserveAspectFit */
}

.bubble-slot.user .bubble-slot__frame {
  transform: scaleX(-1);
}

.bubble-slot__avatar {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.bubble-slot__avatar img {
  width: 48px;
  height: 48px;
  background: #141018;
  border-radius: 2px;
  padding: 2px;
  box-sizing: border-box;
  object-fit: cover; /* PreserveAspectCrop */
}

/* ---- body frame: stone border, repeat tile ---- */
.bubble-body {
  flex: 1;
  min-width: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 14px 16px; /* T/B R/L (slice 14/16/14/16) */
  border-image: url('/assets/ui/frames/frame_chat_bubble_body.png') 14 16 14 16 fill repeat;
  box-sizing: border-box;
  background: var(--war-glass);
  background-clip: padding-box;
  font-family: SimSun, serif;
  color: var(--war-text);
}

.bubble-body.error {
  background: #2a151888;
  background-clip: padding-box;
}

.bubble-body.streaming {
  box-shadow: inset 0 0 0 1px #f2cf6b66;
}

.bubble-body.error .seg-text__plain,
.bubble-body.error .seg-text__md {
  color: var(--war-error);
}

.bubble-body.streaming .seg-text__plain {
  color: var(--war-text);
}

/* ---- header ---- */
.bubble-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 4px;
  user-select: none;
}

.bubble-head.user {
  flex-direction: row-reverse;
}

.bubble-head__name {
  font-weight: bold;
  color: var(--war-gold);
}

.bubble-head__name.user {
  color: var(--war-user-blue);
}

.bubble-head__time {
  color: var(--war-text-muted);
}

.bubble-head__status {
  font-weight: bold;
}

.bubble-head__usage {
  white-space: nowrap;
}

.bubble-head__usage-in {
  color: var(--war-error);
}

.bubble-head__usage-out {
  color: #6fd17f;
  margin-left: 6px;
}

.bubble-head__usage-num {
  color: var(--war-text-muted);
  margin-left: 2px;
}

.bubble-head__copy {
  color: #a0a8b8;
  margin-left: auto;
}

.bubble-head__branch {
  color: #a0a8b8;
}

.bubble-head__branch:hover {
  color: var(--war-gold-bright);
}

.bubble-head__md {
  color: #a0a8b8;
}

.bubble-head__md:hover {
  color: var(--war-gold-bright);
}

.bubble-head.user .bubble-head__copy {
  margin-left: 0;
  margin-right: auto;
}

.bubble-head__copy:hover {
  color: var(--war-gold-bright);
}

.bubble-head__copy.copied {
  color: #80f0a0;
}

/* ---- text segments ---- */
.seg-text__plain {
  white-space: pre-wrap;
  overflow-wrap: break-word;
  user-select: text;
}

.seg-text__plain.err-text {
  color: var(--war-error);
}

/* 四.8: collapsed long user message — clamp to the first lines (pre-wrap
   newlines are preserved inside the line-clamp box). */
.seg-text__plain.user-clamped,
.seg-text__md.user-clamped {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 6;
  overflow: hidden;
}

.user-fold {
  color: var(--war-gold);
  margin-top: 4px;
  user-select: none;
  width: fit-content;
}

.user-fold:hover {
  color: var(--war-gold-bright);
}

.seg-text__md {
  user-select: text;
  overflow-wrap: break-word;
}

/* quote-block user messages: keep newlines (rendered via v-html) */
.quote-body {
  white-space: pre-wrap;
}

/* ---- process summary pill: full bubble width, TWO lines. Line 1 = counts
   + activity hint on one row (nowrap/ellipsis backstop). Line 2 = random
   flavor phrase on its own line — its left edge is constant, so hint length
   changes never shove it around and it never needs truncation. ---- */
.seg-proc {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  width: calc(100% - 24px); /* inset each side so the bubble's gold edge shows */
  min-width: 0;
  margin: 2px 0 6px;
  padding: 2px 10px;
  background: #12151c44;
  border: 1px solid #3a4a40;
  border-radius: 2px;
  color: #d0d6e0;
  user-select: none;
  overflow: hidden;
}

.seg-proc:hover {
  color: var(--war-gold);
  border-color: var(--war-gold-dim);
}

.seg-proc__main {
  display: flex;
  align-items: baseline;
  min-width: 0;
  max-width: 100%;
  white-space: nowrap;
  overflow: hidden;
}

.seg-proc__summary,
.seg-proc__hint {
  flex: 0 1 auto;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.seg-proc__hint {
  color: var(--war-gold);
}

.seg-proc__flavor {
  margin-top: 2px;
  max-width: 100%;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--war-text-muted);
}

/* ---- plan card (ACP plan updates) ---- */
.seg-plan {
  background: #12151c44;
  border: 1px solid #4a4033;
  border-radius: 2px;
  margin: 4px 0;
  padding: 4px 8px;
}

.seg-plan__title {
  color: var(--war-gold);
  user-select: none;
}

.seg-plan__row {
  color: #d0d6e0;
  padding: 1px 0;
  user-select: text;
}

.seg-plan__icon {
  color: var(--war-text-muted);
  margin-right: 6px;
}

.seg-plan__icon.done {
  color: #7ec88a;
}

/* ---- attachments ---- */
.bubble-atts {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 6px;
}

.bubble-atts__img {
  max-width: 280px;
  max-height: 280px;
  object-fit: contain;
  border-radius: 2px;
}

.bubble-atts__chip {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: 220px;
  padding: 4px 8px;
  background: #1a2334;
  border: 1px solid #2c4a7a;
  border-radius: 2px;
}

.bubble-atts__chip img {
  width: 15px;
  height: 15px;
  flex: none;
}

.bubble-atts__chip span {
  color: #c0d0ec;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>

<style>
/* markdown body (unscoped: v-html content). WC3 gold tone (四.7): headings
   and emphasis in gold, code blocks dark with a thin brass border, quotes /
   links / list markers follow the warTheme palette. Kept compact — body
   text styling only, no layout interference with the bubble frame. Shared
   with FilePreviewDialog's preview pane (same .md-body class). */
.md-body > :first-child {
  margin-top: 0;
}
.md-body > :last-child {
  margin-bottom: 0;
}
.md-body p {
  margin: 6px 0;
  line-height: 1.5;
}
.md-body strong,
.md-body b {
  color: var(--war-gold-bright);
}
.md-body em,
.md-body i {
  color: var(--war-gold);
}
.md-body pre {
  background: #00000070;
  border: 1px solid var(--war-gold-input);
  border-radius: 2px;
  padding: 6px 8px;
  overflow-x: auto;
  font-family: Consolas, monospace;
  font-size: 0.92em;
}
/* fenced code block wrapper: copy button pinned top-right, shown on hover */
.md-body .codeblock {
  position: relative;
}
.md-body .codeblock pre {
  margin: 6px 0;
}
.md-body .codeblock__copy {
  position: absolute;
  top: 10px;
  right: 4px;
  z-index: 1;
  padding: 1px 8px;
  background: #12151cbb;
  border: 1px solid var(--war-gold-dim);
  border-radius: 2px;
  color: #a0a8b8;
  font-family: SimSun, serif;
  font-size: 11px;
  cursor: url('/assets/ui/misc/cursor_green_32.png') 1 0, pointer;
  opacity: 0;
  transition: opacity 0.15s;
}
.md-body .codeblock:hover .codeblock__copy,
.md-body .codeblock__copy:focus-visible {
  opacity: 1;
}
.md-body .codeblock__copy:hover {
  color: var(--war-gold-bright);
  border-color: var(--war-gold);
}
.md-body code {
  font-family: Consolas, monospace;
  background: #00000050;
  color: var(--war-gold-bright);
  padding: 0 3px;
  border-radius: 2px;
  font-size: 0.92em;
}
.md-body pre code {
  background: none;
  color: var(--war-text);
  padding: 0;
}
.md-body ul,
.md-body ol {
  margin: 6px 0;
  padding-left: 22px;
}
.md-body li::marker {
  color: var(--war-gold-dim);
}
.md-body h1,
.md-body h2,
.md-body h3,
.md-body h4 {
  margin: 10px 0 6px;
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-weight: bold;
  text-shadow:
    -1px 0 var(--war-outline-brown), 1px 0 var(--war-outline-brown),
    0 -1px var(--war-outline-brown), 0 1px var(--war-outline-brown);
}
.md-body h1 { font-size: 1.25em; }
.md-body h2 { font-size: 1.15em; }
.md-body h3 { font-size: 1.05em; }
.md-body blockquote {
  margin: 6px 0;
  padding-left: 10px;
  border-left: 3px solid var(--war-gold-dim);
  color: var(--war-text-muted);
}
.md-body a {
  color: var(--war-gold-bright);
  text-decoration: underline;
}
.md-body a:hover {
  color: var(--war-gold);
}
.md-body img {
  max-width: min(100%, 320px);
  max-height: 320px;
  object-fit: contain;
  border-radius: 2px;
  cursor: zoom-in;
}

/* inline-image lightbox (unscoped: teleported to body) */
.imglb {
  position: fixed;
  inset: 0;
  z-index: 130;
  background: #000000d0;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: zoom-out;
}
.imglb__img {
  max-width: 94vw;
  max-height: 90vh;
  object-fit: contain;
}
.imglb__hint {
  position: absolute;
  bottom: 12px;
  left: 50%;
  transform: translateX(-50%);
  color: var(--war-text-faint);
  font-family: SimSun, serif;
  user-select: none;
}
.md-body table {
  border-collapse: collapse;
  margin: 6px 0;
}
.md-body th,
.md-body td {
  border: 1px solid #2a3344;
  padding: 3px 8px;
}
.md-body th {
  color: var(--war-gold);
}
.md-body hr {
  border: none;
  border-top: 1px solid #2a3344;
  margin: 8px 0;
}

/* <selection>…</selection> quote block in user bubbles — one elliptical
   capsule, elided to a fixed length, click-selects as a whole unit
   (unscoped: it lives inside v-html content) */
.md-body .md-selection {
  display: inline-block;
  max-width: 100%;
  background: #f2cf6b22;
  border: 1px solid #f2cf6b3d;
  border-radius: 999px;
  padding: 1px 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  vertical-align: baseline;
  user-select: all;
}
</style>

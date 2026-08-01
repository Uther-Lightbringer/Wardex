<script setup lang="ts">
// Chat bubble (ui-design.md §4.7, features/chat.md §2.2).
//
// R1 streaming contract: while `streaming` is true, text/thinking segments
// render as PLAIN TEXT and the trailing segment's DOM node is registered
// with the chat store (registerStreamTarget) so chunks append directly to
// the text node — this component does NOT re-render per chunk (the segment
// text mutation is in-place and non-reactive). When the turn goes final the
// row object is replaced, the component re-renders once, and text segments
// switch to memoized markdown HTML.
//
// segOpen expansion state is component-local keyed by segment INDEX: the
// segment list is append-only so indexes are stable, and since incremental
// appends never recreate this component, blocks the user opened mid-stream
// stay open (ChatBubble.qml:113-123 parity).
import { computed, onBeforeUnmount, onMounted, onUpdated, reactive, ref, watch } from 'vue';
import { fileSrc, openPath } from '../../lib/tauri';
import { renderMarkdown } from '../../lib/markdown';
import { copyText } from '../../lib/clipboard';
import { usePrefsStore } from '../../stores/prefs';
import { useChatStore, type ChatMessage, type ChatSegment } from '../../stores/chat';
import ProcessDialog from './ProcessDialog.vue';

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
const fs = (n: number) => prefs.fs(n);

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

// ---- segments ----

const segs = computed<ChatSegment[]>(() => props.row.segments ?? []);
const hasStructSegs = computed(() => segs.value.some((s) => s.kind !== 'text'));
/** Streaming or structured bubbles pin to max width (no frame-width jitter). */
const pinWide = computed(() => props.streaming || hasStructSegs.value);

// ---- process dialog (final rows: thinking/tool blocks move OUT of the bubble) ----
// Final rows show a single "⚙ N 个步骤" line; clicking opens ProcessDialog
// with the full step list. Streaming rows keep the inline one-line blocks —
// the live-append contract (data-live-seg) needs the tail segment mounted.
const structSegs = computed(() => segs.value.filter((s) => s.kind !== 'text'));
const textSegs = computed(() =>
  segs.value.map((s, i) => ({ s, i })).filter(({ s }) => s.kind === 'text'),
);
const procOpen = ref(false);
const procSummary = computed(() => {
  const think = structSegs.value.filter((s) => s.kind === 'thinking').length;
  const tools = structSegs.value.length - think;
  const parts: string[] = [];
  if (think > 0) parts.push(`思考×${think}`);
  if (tools > 0) parts.push(`工具×${tools}`);
  return `⚙ ${structSegs.value.length} 个步骤（${parts.join(' · ')}）`;
});

/** Placeholder "…" during streaming; treated as empty once final. */
const displayBody = computed(() => {
  const c = props.row.content ?? '';
  if (c === '…' && !props.streaming) return '';
  return c;
});

// segOpen[i]: expansion of thinking/tool blocks (default collapsed).
const segOpen = reactive<Record<number, boolean>>({});
function toggleSeg(i: number): void {
  segOpen[i] = !segOpen[i];
}

function collapseAllSegs(): void {
  for (const k of Object.keys(segOpen)) segOpen[Number(k)] = false;
}

// Auto-collapse on turn end (一.4): blocks the user opened mid-stream snap
// shut once the row reaches a final status. Historical rows load with a
// final status and segOpen empty — already collapsed, the watcher never
// fires for them.
const FINAL_STATUSES = new Set(['done', 'error', 'interrupted']);
watch(
  () => props.row.status,
  (now, was) => {
    if (FINAL_STATUSES.has(now) && !FINAL_STATUSES.has(was ?? '')) collapseAllSegs();
  },
);

// Global collapse/expand-all signal (一.10): the chat store owns a versioned
// command; every mounted bubble applies it to its thinking/tool blocks.
watch(
  () => chat.segCollapseSeq,
  () => {
    const open = chat.segCollapseOpen;
    segs.value.forEach((s, i) => {
      if (s.kind === 'thinking' || s.kind === 'tool') segOpen[i] = open;
    });
  },
);

function toolName(s: ChatSegment): string {
  return String(s.name || s.title || s.kind || 'tool');
}

const TOOL_PAYLOAD_MAX = 64 * 1024; // R4: the in-memory payload is already
// capped at 64KB by the backend; guard the display side the same way.
function toolPayload(s: ChatSegment): string {
  const v = s.rawInput ?? s.arguments ?? s.content ?? s.output ?? '';
  let text = typeof v === 'string' ? v : JSON.stringify(v, null, 2);
  if (text.length > TOOL_PAYLOAD_MAX) text = text.slice(0, TOOL_PAYLOAD_MAX) + '\n…（已截断）';
  return text;
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
</script>

<template>
  <div class="bubble-row" :class="{ user: isUser }">
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
            v-if="!isUser && !streaming"
            class="bubble-head__md"
            :style="{ fontSize: fs(11) + 'px' }"
            @click="mdEnabled = !mdEnabled"
            >{{ mdEnabled ? '原文' : '渲染' }}</span
          >
          <span
            class="bubble-head__copy"
            :class="{ copied }"
            :style="{ fontSize: fs(11) + 'px' }"
            @click="copyBody"
            >{{ copied ? '已复制' : '复制' }}</span
          >
        </div>

        <!-- segments in arrival order -->
        <template v-if="segs.length > 0">
          <!-- final rows: thinking/tool collapsed to ONE process line; the
               dialog holds the full step list (text stays inline) -->
          <template v-if="!streaming">
            <div
              v-if="structSegs.length > 0"
              class="seg-proc"
              :style="{ fontSize: fs(12) + 'px' }"
              @click="procOpen = true"
            >
              {{ procSummary }}
            </div>
            <div v-for="{ s, i } in textSegs" :key="i" class="seg-text">
              <span v-if="!mdEnabled" class="seg-text__plain">{{ s.text }}</span>
              <div v-else class="seg-text__md md-body" v-html="markdownOf(s.text ?? '')"></div>
            </div>
            <ProcessDialog v-model:open="procOpen" :segments="structSegs" :title="displayName + ' · ' + timeText" />
          </template>

          <!-- streaming: inline one-line blocks (live-append contract) -->
          <template v-else>
          <template v-for="(s, i) in segs" :key="i">
            <!-- thinking: warm collapsible block, collapsed by default -->
            <div v-if="s.kind === 'thinking'" class="seg-thinking">
              <div class="seg-thinking__head" :style="{ fontSize: fs(12) + 'px' }" @click="toggleSeg(i)">
                {{ segOpen[i] ? '▼' : '▶' }} 思考过程
              </div>
              <div
                v-if="segOpen[i]"
                class="seg-thinking__body"
                :class="{ 'no-md': streaming }"
                :style="{ fontSize: fs(11) + 'px' }"
                :data-live-seg="streaming && i === segs.length - 1 ? '' : undefined"
                >{{ s.text }}</div
              >
            </div>

            <!-- text: plain during streaming, markdown once final. The live
                 segment marker sits ON the span so the store's appendData
                 targets its text node directly (no stray sibling nodes). -->
            <div v-else-if="s.kind === 'text'" class="seg-text">
              <span
                v-if="streaming || !mdEnabled"
                class="seg-text__plain"
                :data-live-seg="streaming && i === segs.length - 1 ? '' : undefined"
                >{{ s.text }}</span
              >
              <div v-else class="seg-text__md md-body" v-html="markdownOf(s.text ?? '')"></div>
            </div>

            <!-- tool: single-line header, payload on expand -->
            <div v-else class="seg-tool">
              <div class="seg-tool__head" :style="{ fontSize: fs(12) + 'px' }" @click="toggleSeg(i)">
                {{ segOpen[i] ? '▼' : '▶' }} · {{ toolName(s) }}
                <span v-if="s.status" class="seg-tool__status">[{{ s.status }}]</span>
              </div>
              <pre v-if="segOpen[i]" class="seg-tool__payload" :style="{ fontSize: fs(11) + 'px' }">{{ toolPayload(s) }}</pre>
            </div>
          </template>
          </template>
        </template>

        <!-- fallback: user rows and the pending placeholder have no segments -->
        <div v-else-if="displayBody" class="seg-text">
          <div
            v-if="!isUser && mdEnabled"
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

        <!-- user attachments -->
        <div v-if="row.attachments && row.attachments.length" class="bubble-atts">
          <template v-for="(p, i) in row.attachments" :key="i">
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
      </div>
    </div>
  </div>
</template>

<style scoped>
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

.bubble-head__copy {
  color: #a0a8b8;
  margin-left: auto;
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
.seg-text__plain.user-clamped {
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

/* ---- process summary line (final rows; opens ProcessDialog) ---- */
.seg-proc {
  width: fit-content;
  margin: 2px 0 6px;
  padding: 2px 10px;
  background: #12151c44;
  border: 1px solid #3a4a40;
  border-radius: 2px;
  color: #d0d6e0;
  user-select: none;
}

.seg-proc:hover {
  color: var(--war-gold);
  border-color: var(--war-gold-dim);
}

/* ---- thinking block ---- */
.seg-thinking {
  background: #19151044;
  border: 1px solid #4a4232;
  border-radius: 2px;
  margin: 4px 0;
  padding: 4px 8px;
}

.seg-thinking__head {
  color: #c8b890;
  user-select: none;
}

.seg-thinking__body {
  color: #908878;
  white-space: pre-wrap;
  overflow-wrap: break-word;
  margin-top: 4px;
  user-select: text;
}

/* ---- tool block ---- */
.seg-tool {
  background: #12151c44;
  border: 1px solid #3a4a40;
  border-radius: 2px;
  margin: 4px 0;
  padding: 4px 8px;
}

.seg-tool__head {
  color: #d0d6e0;
  user-select: none;
}

.seg-tool__status {
  color: var(--war-text-muted);
  margin-left: 6px;
}

.seg-tool__payload {
  color: var(--war-text-muted);
  white-space: pre-wrap;
  overflow-wrap: break-word;
  margin: 4px 0 0;
  font-family: Consolas, monospace;
  user-select: text;
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
</style>

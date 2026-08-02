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
import { renderMarkdown, renderUserMarkdown } from '../../lib/markdown';
import { copyText } from '../../lib/clipboard';
import { usePrefsStore } from '../../stores/prefs';
import { useChatStore, type ChatMessage, type ChatSegment } from '../../stores/chat';
import { formatTokens } from '../../lib/format';
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
/** Reminder-triggered user rows render as a centered system line (no avatar,
 * no bubble frame, no branch button); copy is not offered there. */
const isReminder = computed(() => props.row.kind === 'reminder');
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
// Thinking/tool segments collapse to a single "⚙ N 个步骤" line; clicking
// opens ProcessDialog with the step list (live while streaming). Streaming
// rows append a current-activity hint and a random flavor line.
const structSegs = computed(() => segs.value.filter((s) => s.kind !== 'text' && !isPlan(s)));
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

/** Streaming-only: what the agent is doing right now (tail segment). */
const activityHint = computed(() => {
  if (!props.streaming) return '';
  const tail = segs.value[segs.value.length - 1];
  if (!tail) return '';
  if (tail.kind === 'thinking') return '思考中…';
  if (tail.kind === 'tool') {
    const st = tail.status ? ` [${tail.status}]` : '';
    return `▶ ${toolName(tail)}${st}`;
  }
  return '';
});

// Streaming-only flavor lines: a random fixed phrase trails the activity
// hint, re-picked on every structural event (new segment / tool upsert).
const FLAVOR_LINES = [
  '天灾军团正在集结……',
  '为了联盟！',
  '为了部落！',
  '敲响警钟！',
  '战争已经打响。',
  '力量与荣耀！',
  '敌人在前进……',
  '铁匠铺的炉火正旺。',
  '圣光保佑我们。',
  '黑暗即将降临。',
  '准备战斗！',
  '号角已吹响。',
  '侦察骑兵已经出发。',
  '箭塔已就位。',
  '金币叮当作响。',
];
const flavor = ref('');
let lastFlavorIdx = -1;
watch(
  () => segs.value,
  () => {
    if (!props.streaming || structSegs.value.length === 0) return;
    let idx = Math.floor(Math.random() * FLAVOR_LINES.length);
    if (FLAVOR_LINES.length > 1 && idx === lastFlavorIdx) idx = (idx + 1) % FLAVOR_LINES.length;
    lastFlavorIdx = idx;
    flavor.value = FLAVOR_LINES[idx];
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
  }
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
  <!-- reminder-triggered user row: centered system line, no avatar/branch -->
  <div v-if="isReminder" class="bubble-sys">
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
            <span>{{ procSummary }}</span>
            <template v-if="streaming">
              <span v-if="activityHint" class="seg-proc__hint">{{ activityHint }}</span>
              <span v-if="flavor" class="seg-proc__flavor">{{ flavor }}</span>
            </template>
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
            v-if="userMdHtml"
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

.seg-proc__hint {
  color: var(--war-gold);
}

.seg-proc__flavor {
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
</style>

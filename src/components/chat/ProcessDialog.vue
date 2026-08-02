<script setup lang="ts">
// Process detail dialog (features/chat.md §2.2): opened from a chat bubble's
// "⚙ N 个步骤" line (streaming AND final rows). Lists the turn's
// thinking/tool segments in arrival order — one line each, click to expand
// the payload inline. While a thinking tail is still streaming, a 250ms
// ticker re-reads its store text (R1 appends are non-reactive) so the open
// dialog stays live. Esc / mask click closes.
import { reactive, ref, watch, onBeforeUnmount } from 'vue';
import type { ChatSegment } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';
import WarButton from '../war/WarButton.vue';
import WarScrollBar from '../war/WarScrollBar.vue';

const props = defineProps<{
  open: boolean;
  segments: ChatSegment[];
  /** e.g. "阿尔萨斯2 · 09:21:09" */
  title: string;
}>();

const emit = defineEmits<{ (e: 'update:open', v: boolean): void }>();

const prefs = usePrefsStore();

const openIdx = reactive<Record<number, boolean>>({});
function toggle(i: number): void {
  openIdx[i] = !openIdx[i];
}

function toolName(s: ChatSegment): string {
  return String(s.name || s.title || s.kind || 'tool');
}

const PAYLOAD_MAX = 64 * 1024; // in-memory payload is already capped upstream
function payload(s: ChatSegment): string {
  const v = s.rawInput ?? s.arguments ?? s.content ?? s.output ?? '';
  let text = typeof v === 'string' ? v : JSON.stringify(v, null, 2);
  if (text.length > PAYLOAD_MAX) text = text.slice(0, PAYLOAD_MAX) + '\n…（已截断）';
  return text;
}

function close(): void {
  emit('update:open', false);
}

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.stopPropagation();
    close();
  }
}

watch(
  () => props.open,
  (v) => {
    if (v) {
      window.addEventListener('keydown', onKey, true);
      tick();
      if (!ticker) ticker = setInterval(tick, TICK_MS);
    } else {
      window.removeEventListener('keydown', onKey, true);
      stopTicker();
    }
  },
);
onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKey, true);
  stopTicker();
});

const listEl = ref<HTMLElement | null>(null);

// Live tail text (R1: thinking chunks extend the store non-reactively — a
// 250ms ticker re-reads the tail into a local ref so an open dialog stays
// current without re-rendering the list). Only active while open.
const TICK_MS = 250;
const liveText = ref('');
let ticker: ReturnType<typeof setInterval> | null = null;

function stopTicker(): void {
  if (ticker) {
    clearInterval(ticker);
    ticker = null;
  }
}

/** Keep the feed pinned to the bottom while the user is already at the bottom. */
function pinBottom(): void {
  const el = listEl.value;
  if (!el) return;
  if (el.scrollTop + el.clientHeight >= el.scrollHeight - 40) el.scrollTop = el.scrollHeight;
}

function tick(): void {
  const segs = props.segments;
  const tail = segs[segs.length - 1];
  if (tail?.kind === 'thinking') liveText.value = tail.text ?? '';
  pinBottom();
}

// New steps while open: keep the feed pinned when already near the bottom.
watch(
  () => props.segments.length,
  () => {
    if (props.open) pinBottom();
  },
);
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="pd-mask" @mousedown.self="close">
      <div class="pd">
        <div class="pd__frame"></div>

        <div class="pd__inner">
          <div class="pd__head">
            <div class="pd__title" :style="{ fontSize: prefs.fs(13) + 'px' }">
              过程明细 · {{ segments.length }} 个步骤
              <span class="pd__sub">{{ title }}</span>
            </div>
            <span class="pd__close" title="关闭" @click="close">✕</span>
          </div>

          <div class="pd__list-wrap">
            <div ref="listEl" class="pd__list">
              <template v-for="(s, i) in segments" :key="i">
                <!-- thinking -->
                <div v-if="s.kind === 'thinking'" class="pd__step pd__step--thinking">
                  <div class="pd__step-head" :style="{ fontSize: prefs.fs(12) + 'px' }" @click="toggle(i)">
                    {{ openIdx[i] ? '▼' : '▶' }} 思考过程
                  </div>
                  <div v-if="openIdx[i]" class="pd__thinking-body" :style="{ fontSize: prefs.fs(11) + 'px' }">
                    {{ i === segments.length - 1 ? liveText : s.text }}
                  </div>
                </div>

                <!-- tool -->
                <div v-else class="pd__step pd__step--tool">
                  <div class="pd__step-head" :style="{ fontSize: prefs.fs(12) + 'px' }" @click="toggle(i)">
                    {{ openIdx[i] ? '▼' : '▶' }} · {{ toolName(s) }}
                    <span v-if="s.status" class="pd__status">[{{ s.status }}]</span>
                  </div>
                  <pre v-if="openIdx[i]" class="pd__payload" :style="{ fontSize: prefs.fs(11) + 'px' }">{{
                    payload(s)
                  }}</pre>
                </div>
              </template>
            </div>
            <WarScrollBar :target="listEl" />
          </div>

          <div class="pd__footer">
            <WarButton skin="dialog" :width="150" text="关闭" @activated="close" />
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.pd-mask {
  position: fixed;
  inset: 0;
  z-index: 115;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.pd {
  position: relative;
  width: min(720px, 90vw);
  height: min(540px, 86vh);
}

/* frame_popup.png nine-slice (slice 88/100/90/100, center painted → fill) */
.pd__frame {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 88px 100px 90px 100px;
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.pd__inner {
  position: absolute;
  inset: 60px 64px 56px 62px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-height: 0;
  font-family: SimSun, serif;
}

.pd__head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 12px;
}

.pd__title {
  flex: 1;
  min-width: 0;
  color: var(--war-gold);
  font-weight: bold;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.pd__sub {
  color: var(--war-text-muted);
  font-weight: normal;
  margin-left: 8px;
}

.pd__close {
  flex: none;
  color: var(--war-text-dim);
  padding: 2px 6px;
  user-select: none;
}

.pd__close:hover {
  color: var(--war-gold-bright);
}

.pd__list-wrap {
  flex: 1;
  min-height: 0;
  display: flex;
  border: 1px solid #1a2230;
  background: #10141dcc;
}

.pd__list {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none;
  padding: 4px;
}

.pd__step {
  border-radius: 2px;
  margin: 4px;
  padding: 4px 8px;
}

.pd__step--thinking {
  background: #19151044;
  border: 1px solid #4a4232;
}

.pd__step--tool {
  background: #12151c44;
  border: 1px solid #3a4a40;
}

.pd__step-head {
  color: #d0d6e0;
  user-select: none;
}

.pd__step--thinking .pd__step-head {
  color: #c8b890;
}

.pd__step-head:hover {
  color: var(--war-gold);
}

.pd__status {
  color: var(--war-text-muted);
  margin-left: 6px;
}

.pd__thinking-body {
  color: #908878;
  white-space: pre-wrap;
  overflow-wrap: break-word;
  margin-top: 4px;
  user-select: text;
}

.pd__payload {
  color: var(--war-text-muted);
  white-space: pre-wrap;
  overflow-wrap: break-word;
  margin: 4px 0 0;
  font-family: Consolas, monospace;
  user-select: text;
}

.pd__footer {
  flex: none;
  display: flex;
  justify-content: center;
  padding-top: 2px;
}
</style>

<script setup lang="ts">
// Message list with simple windowing (performance.md §2: only viewport
// ±buffer is rendered). Rows are variable-height, so offsets come from a
// measured-height map with an estimate for unmounted rows; measurements
// stream in through MeasuredRow's ResizeObserver and the prefix sums are
// rebuilt (cheap: one pass over the id list, no DOM work).
//
// Scroll-follow (features/chat.md §2.4): nearBottom = ≤80px from the end;
// user scrolling away pauses follow, send / session switch force
// scroll-to-end. A floating "↓ 回到底部" pill appears when rows exist and
// the view is not near the bottom.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { convertFileSrc } from '@tauri-apps/api/core';
import { useChatStore } from '../../stores/chat';
import { useSessionsStore } from '../../stores/sessions';
import { usePrefsStore } from '../../stores/prefs';
import ChatBubble from './ChatBubble.vue';
import MeasuredRow from './MeasuredRow.vue';
import WarScrollBar from '../war/WarScrollBar.vue';

const chat = useChatStore();
const sessions = useSessionsStore();
const prefs = usePrefsStore();

const EST_HEIGHT = 90;
const NEAR_BOTTOM_PX = 80;

const scroller = ref<HTMLElement | null>(null);
const viewportH = ref(0);
const scrollTop = ref(0);
const followBottom = ref(true);

// ---- measured heights (non-reactive map + version counter) ----
const heights = new Map<string, number>();
const heightsVersion = ref(0);
let measureRaf = 0;

function onMeasure(id: string, h: number): void {
  if (heights.get(id) === h) return;
  heights.set(id, h);
  if (!measureRaf) {
    measureRaf = requestAnimationFrame(() => {
      measureRaf = 0;
      heightsVersion.value += 1;
    });
  }
}

// ---- offsets / windowing ----
interface Offset {
  top: number;
  height: number;
}

const layout = computed(() => {
  void heightsVersion.value; // recompute dependency
  const rows = chat.rows;
  const offsets: Offset[] = new Array(rows.length);
  let acc = 0;
  for (let i = 0; i < rows.length; i++) {
    const h = heights.get(rows[i].id) ?? EST_HEIGHT;
    offsets[i] = { top: acc, height: h };
    acc += h;
  }
  return { offsets, total: acc };
});

const BUFFER = computed(() => Math.max(viewportH.value, 400));

const range = computed(() => {
  const { offsets } = layout.value;
  const lo = scrollTop.value - BUFFER.value;
  const hi = scrollTop.value + viewportH.value + BUFFER.value;
  let start = 0;
  let end = offsets.length - 1;
  for (let i = 0; i < offsets.length; i++) {
    if (offsets[i].top + offsets[i].height >= lo) {
      start = i;
      break;
    }
  }
  for (let i = offsets.length - 1; i >= 0; i--) {
    if (offsets[i].top <= hi) {
      end = i;
      break;
    }
  }
  return { start, end };
});

const visibleRows = computed(() => {
  const { start, end } = range.value;
  return chat.rows.slice(start, end + 1).map((row, i) => ({ row, index: start + i }));
});

const padTop = computed(() => layout.value.offsets[range.value.start]?.top ?? 0);
const padBottom = computed(() => {
  const { offsets, total } = layout.value;
  const last = offsets[range.value.end];
  return last ? total - (last.top + last.height) : 0;
});

// ---- scroll follow ----
let programmatic = false;

function nearBottom(): boolean {
  const el = scroller.value;
  if (!el) return true;
  return el.scrollHeight - el.scrollTop - el.clientHeight <= NEAR_BOTTOM_PX;
}

function onScroll(): void {
  const el = scroller.value;
  if (!el) return;
  scrollTop.value = el.scrollTop;
  if (programmatic) return; // programmatic pinning never flips follow state
  followBottom.value = nearBottom();
}

async function scrollToEnd(): Promise<void> {
  followBottom.value = true;
  await nextTick();
  const el = scroller.value;
  if (!el) return;
  programmatic = true;
  el.scrollTop = el.scrollHeight;
  scrollTop.value = el.scrollTop;
  requestAnimationFrame(() => (programmatic = false));
}

// Growth (new rows, streaming height) pins to the bottom while following.
watch(
  () => [chat.rows.length, layout.value.total],
  async () => {
    if (!followBottom.value) return;
    await nextTick();
    const el = scroller.value;
    if (!el) return;
    programmatic = true;
    el.scrollTop = el.scrollHeight;
    scrollTop.value = el.scrollTop;
    requestAnimationFrame(() => (programmatic = false));
  },
);

watch(
  () => chat.scrollSeq,
  () => void scrollToEnd(),
);

// Session switch: drop stale measurements, jump to the end.
watch(
  () => chat.sessionId,
  () => {
    heights.clear();
    heightsVersion.value += 1;
    void scrollToEnd();
  },
);

// ---- viewport sizing ----
let ro: ResizeObserver | null = null;
onMounted(() => {
  if (scroller.value) {
    ro = new ResizeObserver(() => {
      viewportH.value = scroller.value?.clientHeight ?? 0;
    });
    ro.observe(scroller.value);
    viewportH.value = scroller.value.clientHeight;
  }
  void scrollToEnd();
});
onBeforeUnmount(() => {
  ro?.disconnect();
  if (measureRaf) cancelAnimationFrame(measureRaf);
});

// ---- bubble props helpers ----
const BUILTIN_AGENT_AVATAR = '/assets/ui/avatars/avatar_agent.png';
const BUILTIN_USER_AVATAR = '/assets/ui/avatars/avatar_user_default.png';

function displayName(row: { role: string }): string {
  if (row.role === 'user') return prefs.userName || '阿尔萨斯';
  return chat.meta?.agentName || 'Agent';
}

function avatarUrl(row: { role: string }): string {
  if (row.role === 'user') {
    return prefs.userAvatarPath ? convertFileSrc(prefs.userAvatarPath) : BUILTIN_USER_AVATAR;
  }
  const agent = sessions.agentById(chat.meta?.agentId ?? '');
  if (agent?.avatarPath) return convertFileSrc(agent.avatarPath);
  return BUILTIN_AGENT_AVATAR;
}

function isStreaming(index: number): boolean {
  return index === chat.rows.length - 1 && chat.streamRow?.id === chat.rows[index]?.id;
}

defineExpose({ scrollToEnd });
</script>

<template>
  <div class="msglist">
    <div ref="scroller" class="msglist__scroll" @scroll.passive="onScroll">
      <div :style="{ height: padTop + 'px' }"></div>
      <MeasuredRow
        v-for="item in visibleRows"
        :key="item.row.id"
        :row-id="item.row.id"
        @measure="onMeasure"
      >
        <ChatBubble
          :row="item.row"
          :streaming="isStreaming(item.index)"
          :display-name="displayName(item.row)"
          :avatar-url="avatarUrl(item.row)"
        />
      </MeasuredRow>
      <div :style="{ height: padBottom + 'px' }"></div>
    </div>

    <div class="msglist__warbar">
      <WarScrollBar :target="scroller" />
    </div>

    <button
      v-if="chat.rows.length > 0 && !followBottom"
      class="msglist__to-bottom"
      @click="scrollToEnd"
    >
      ↓ 回到底部
    </button>
  </div>
</template>

<style scoped>
.msglist {
  position: relative;
  height: 100%;
  min-height: 0;
}

.msglist__scroll {
  height: 100%;
  overflow-y: auto;
  scrollbar-width: none; /* native bar hidden — the WC3 WarScrollBar replaces it */
  padding-right: 24px; /* room for the WC3 scrollbar */
  box-sizing: border-box;
}

.msglist__warbar {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  width: 22px;
  z-index: 20;
}

.msglist__to-bottom {
  position: absolute;
  right: 34px;
  bottom: 12px;
  z-index: 30;
  width: 108px;
  height: 28px;
  border-radius: 14px;
  border: 1px solid #6a5a3f;
  background: #0d1116f0;
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-size: 12px;
  padding: 0;
}

.msglist__to-bottom:hover {
  color: var(--war-gold-bright);
  border-color: var(--war-gold);
}
</style>

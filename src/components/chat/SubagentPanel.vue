<script setup lang="ts">
// Sub-agent panel (features/chat.md §4.2, old SubagentPanel.qml): read-only
// list of the CURRENT turn's sub-agent calls. Visible while entries exist;
// the backend clears the list on each new turn (acp://subagent with []).
// A 1s heartbeat refreshes durations only while the panel is visible AND an
// entry is pending/in_progress ("不可见不工作").
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { useChatStore, type Subagent } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';

const chat = useChatStore();
const prefs = usePrefsStore();

const open = ref(true);
const now = ref(Date.now());

const activeCount = computed(
  () => chat.subagents.filter((s) => s.status === 'in_progress').length,
);
const hasLive = computed(() =>
  chat.subagents.some((s) => s.status === 'pending' || s.status === 'in_progress'),
);

// 1s heartbeat — only while visible with live entries.
let timer: ReturnType<typeof setInterval> | null = null;
function syncTimer(): void {
  const need = hasLive.value;
  if (need && !timer) {
    timer = setInterval(() => (now.value = Date.now()), 1000);
  } else if (!need && timer) {
    clearInterval(timer);
    timer = null;
  }
}
watch(hasLive, syncTimer, { immediate: true });
onBeforeUnmount(() => {
  if (timer) clearInterval(timer);
});

const STATUS_CN: Record<string, string> = {
  in_progress: '执行中',
  pending: '等待',
  completed: '完成',
  failed: '失败',
  interrupted: '中断',
};

function dotClass(s: Subagent): string {
  switch (s.status) {
    case 'in_progress':
      return 'live';
    case 'pending':
      return 'pending';
    case 'failed':
      return 'failed';
    case 'interrupted':
      return 'pending';
    default:
      return 'done';
  }
}

function breathing(s: Subagent): boolean {
  return s.status === 'pending' || s.status === 'in_progress';
}

function duration(s: Subagent): string {
  void now.value; // heartbeat dependency
  const end = s.finishedAt > 0 ? s.finishedAt : Date.now();
  if (s.startedAt <= 0) return '';
  const sec = Math.max(0, Math.round((end - s.startedAt) / 1000));
  if (sec < 60) return `${sec}s`;
  return `${Math.floor(sec / 60)}m${sec % 60}s`;
}

function meta(s: Subagent): string {
  const parts = [STATUS_CN[s.status] ?? s.status];
  if (s.summary) parts.push(s.summary);
  const d = duration(s);
  if (d) parts.push(d);
  return parts.join(' · ');
}
</script>

<template>
  <div v-if="chat.subagents.length > 0" class="subagent">
    <div class="subagent__head" @click="open = !open">
      <span :style="{ fontSize: prefs.fs(12) + 'px' }">
        {{ open ? '▼' : '▶' }} 子 Agent (执行中 {{ activeCount }} / 共 {{ chat.subagents.length }})
      </span>
    </div>
    <div v-if="open" class="subagent__list">
      <div v-for="s in chat.subagents" :key="s.id" class="subagent__row">
        <span class="subagent__dot" :class="[dotClass(s), { breath: breathing(s) }]"></span>
        <span
          class="subagent__title"
          :class="{ settled: s.status === 'completed' }"
          :style="{ fontSize: prefs.fs(12) + 'px' }"
          >{{ s.children > 0 ? `[${s.children} 个子任务] ` : '' }}{{ s.title }}</span
        >
        <span class="subagent__meta" :style="{ fontSize: prefs.fs(10) + 'px' }">{{ meta(s) }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.subagent {
  background: #0d1116f0;
  border: 1px solid #6a5a3f;
  border-radius: 3px;
  font-family: SimSun, serif;
}

.subagent__head {
  display: flex;
  align-items: center;
  height: 26px;
  padding: 0 10px;
  color: var(--war-gold);
  user-select: none;
}

.subagent__list {
  max-height: 160px;
  overflow-y: auto;
  padding: 0 10px 6px;
}

.subagent__row {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 24px;
}

.subagent__dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
}

.subagent__dot.live {
  background: #57d977;
}

.subagent__dot.pending {
  background: #f2cf6b;
}

.subagent__dot.failed {
  background: #d08070;
}

.subagent__dot.done {
  background: #4a5265;
}

.subagent__dot.breath {
  animation: breath 550ms ease-in-out infinite alternate;
}

@keyframes breath {
  from {
    opacity: 0.35;
  }
  to {
    opacity: 1;
  }
}

.subagent__title {
  flex: 1;
  min-width: 0;
  color: #d0d6e0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.subagent__title.settled {
  color: #8a92a2;
}

.subagent__meta {
  flex: none;
  color: #6d7688;
  white-space: nowrap;
}
</style>

<script setup lang="ts">
// Background-task panel: the CLI's background tasks (Bash run_in_background
// etc.), tracked as kind === 'task' entries in the chat store's subagent list
// (runtime.rs track_subagent). Content-only — the dock supplies the frame.
// A 1s heartbeat refreshes elapsed times only while an entry is
// pending/in_progress ("不可见不工作"), same pattern as SubagentPanel.
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { useChatStore, type Subagent } from '../stores/chat';
import { usePrefsStore } from '../stores/prefs';
import WarScrollBar from '../components/war/WarScrollBar.vue';

const chat = useChatStore();
const prefs = usePrefsStore();

const listEl = ref<HTMLElement | null>(null);
const now = ref(Date.now());

const tasks = computed<Subagent[]>(() => chat.subagents.filter((s) => s.kind === 'task'));
const hasLive = computed(() =>
  tasks.value.some((s) => s.status === 'pending' || s.status === 'in_progress'),
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

function elapsed(s: Subagent): string {
  void now.value; // heartbeat dependency
  if (s.startedAt <= 0) return '';
  const end = s.finishedAt > 0 ? s.finishedAt : Date.now();
  const sec = Math.max(0, Math.round((end - s.startedAt) / 1000));
  if (sec < 60) return `${sec}s`;
  return `${Math.floor(sec / 60)}m${sec % 60}s`;
}

function shortId(s: Subagent): string {
  const id = s.taskId || s.id;
  return id.length > 8 ? id.slice(0, 8) : id;
}

function meta(s: Subagent): string {
  const parts = [STATUS_CN[s.status] ?? s.status];
  const d = elapsed(s);
  if (d) parts.push(d);
  return parts.join(' · ');
}
</script>

<template>
  <div class="taskp">
    <div class="taskp__list-wrap">
      <div ref="listEl" class="taskp__list">
        <div v-if="tasks.length === 0" class="taskp__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">
          （暂无后台任务）
        </div>
        <div v-for="t in tasks" :key="t.id" class="taskp__row" :class="{ 'taskp__row--done': t.status === 'completed' }">
          <span class="taskp__dot" :class="[dotClass(t), { breath: breathing(t) }]"></span>
          <span class="taskp__id" :style="{ fontSize: prefs.fs(10) + 'px' }" :title="t.taskId || t.id">
            {{ shortId(t) }}
          </span>
          <div class="taskp__main">
            <div class="taskp__title" :style="{ fontSize: prefs.fs(12) + 'px' }" :title="t.title">{{ t.title }}</div>
            <div class="taskp__meta" :style="{ fontSize: prefs.fs(10) + 'px' }">{{ meta(t) }}</div>
          </div>
        </div>
      </div>
      <WarScrollBar :target="listEl" />
    </div>
  </div>
</template>

<style scoped>
.taskp {
  display: flex;
  flex-direction: column;
  gap: 6px;
  height: 100%;
  min-height: 0;
  font-family: SimSun, serif;
}

.taskp__list-wrap {
  flex: 1;
  min-height: 0;
  display: flex;
}

.taskp__list {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none; /* native bar hidden — the WC3 WarScrollBar replaces it */
}

.taskp__empty {
  color: var(--war-text-faint);
  text-align: center;
  padding: 8px 0;
}

.taskp__row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 3px 0;
}

.taskp__row--done .taskp__title,
.taskp__row--done .taskp__meta {
  color: var(--war-text-faint);
}

.taskp__dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
}

.taskp__dot.live {
  background: #57d977;
}

.taskp__dot.pending {
  background: #f2cf6b;
}

.taskp__dot.failed {
  background: #d08070;
}

.taskp__dot.done {
  background: #4a5265;
}

.taskp__dot.breath {
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

.taskp__id {
  flex: none;
  color: var(--war-gold-dim);
  user-select: none;
}

.taskp__main {
  flex: 1;
  min-width: 0;
}

.taskp__title {
  color: var(--war-text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.taskp__meta {
  color: var(--war-text-muted);
}
</style>

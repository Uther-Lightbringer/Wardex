<script setup lang="ts">
// Reminders panel: the current session's reminder list (content, due time /
// countdown, agent/user source badge, cancel button) plus an 添加提醒 entry
// that opens the ReminderDialog. refreshOn: turnEnd | sessionSwitch | manual
// — agent-scheduled reminders arrive during a turn via chat://reminders; the
// store listener keeps the mirror live, the watches re-pull on switch.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useChatStore } from '../stores/chat';
import { usePrefsStore } from '../stores/prefs';
import { useRemindersStore, type Reminder } from '../stores/reminders';
import ReminderDialog from '../components/chat/ReminderDialog.vue';
import WarScrollBar from '../components/war/WarScrollBar.vue';

const chat = useChatStore();
const prefs = usePrefsStore();
const reminders = useRemindersStore();

const dialogOpen = ref(false);
const listEl = ref<HTMLElement | null>(null);

// Countdown tick: 20s cadence is plenty for minute-resolution reminders.
const now = ref(Date.now());
let tickTimer = 0;

onMounted(() => {
  void reminders.init();
  void reminders.load(chat.sessionId);
  tickTimer = window.setInterval(() => {
    now.value = Date.now();
  }, 20_000);
});
onBeforeUnmount(() => window.clearInterval(tickTimer));

watch(() => chat.sessionId, (id) => void reminders.load(id)); // sessionSwitch
watch(() => chat.turnSeq, () => void reminders.refresh()); // turnEnd

const sorted = computed<Reminder[]>(() =>
  [...reminders.rows].sort((a, b) => Number(a.done) - Number(b.done) || a.dueAtMs - b.dueAtMs),
);

const p = (n: number) => String(n).padStart(2, '0');

function dueLine(r: Reminder): string {
  if (r.done) return '已提醒';
  const d = new Date(r.dueAtMs);
  const stamp = `${p(d.getHours())}:${p(d.getMinutes())}`;
  const diff = r.dueAtMs - now.value;
  if (diff <= 0) return `${stamp} · 已到期`;
  const mins = Math.round(diff / 60_000);
  const remain = mins >= 60 ? `${Math.floor(mins / 60)} 小时 ${mins % 60} 分钟` : `${mins} 分钟`;
  return `${stamp} · 还剩 ${remain}`;
}

function sourceBadge(r: Reminder): string {
  return r.source === 'agent' ? 'Agent' : '我';
}
</script>

<template>
  <div class="remp">
    <div class="remp__list-wrap">
      <div ref="listEl" class="remp__list">
        <div v-if="!chat.sessionId" class="remp__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">（无会话）</div>
        <div v-else-if="sorted.length === 0" class="remp__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">
          （暂无提醒）
        </div>
        <div v-for="r in sorted" :key="r.id" class="remp__row" :class="{ 'remp__row--done': r.done }">
          <span class="remp__src" :class="{ 'remp__src--agent': r.source === 'agent' }" :style="{ fontSize: prefs.fs(10) + 'px' }">
            {{ sourceBadge(r) }}
          </span>
          <div class="remp__main">
            <div class="remp__content" :style="{ fontSize: prefs.fs(12) + 'px' }" :title="r.content">
              {{ r.content }}
            </div>
            <div class="remp__due" :style="{ fontSize: prefs.fs(10) + 'px' }">{{ dueLine(r) }}</div>
          </div>
          <span
            v-if="!r.done"
            class="remp__cancel"
            title="取消提醒"
            :style="{ fontSize: prefs.fs(10) + 'px' }"
            @click="reminders.cancel(r.id)"
          >
            取消
          </span>
        </div>
      </div>
      <WarScrollBar :target="listEl" />
    </div>
    <div class="remp__footer">
      <span class="remp__add" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="dialogOpen = true">＋ 添加提醒</span>
    </div>
    <ReminderDialog :open="dialogOpen" @update:open="(v) => (dialogOpen = v)" />
  </div>
</template>

<style scoped>
.remp {
  display: flex;
  flex-direction: column;
  gap: 6px;
  height: 100%;
  min-height: 0;
  font-family: SimSun, serif;
}

.remp__list-wrap {
  flex: 1;
  min-height: 0;
  display: flex;
}

.remp__list {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none; /* native bar hidden — the WC3 WarScrollBar replaces it */
}

.remp__empty {
  color: var(--war-text-faint);
  text-align: center;
  padding: 8px 0;
}

.remp__row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 3px 0;
}

.remp__row--done .remp__content,
.remp__row--done .remp__due {
  color: var(--war-text-faint);
}

.remp__src {
  flex: none;
  min-width: 34px;
  text-align: center;
  color: var(--war-user-blue);
}

.remp__src--agent {
  color: var(--war-gold-dim);
}

.remp__main {
  flex: 1;
  min-width: 0;
}

.remp__content {
  color: var(--war-text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.remp__due {
  color: var(--war-text-muted);
}

.remp__cancel {
  flex: none;
  color: var(--war-text-muted);
  user-select: none;
}

.remp__cancel:hover {
  color: var(--war-error);
}

.remp__footer {
  flex: none;
  display: flex;
  justify-content: center;
}

.remp__add {
  color: var(--war-gold);
  user-select: none;
}

.remp__add:hover {
  color: var(--war-gold-bright);
}
</style>

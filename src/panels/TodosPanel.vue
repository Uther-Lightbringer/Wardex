<script setup lang="ts">
// Todos panel (unified todo/reminder board): context view following the
// current session — 本会话 (session scope) / 本项目 (project scope) / 全局
// (global scope) groups with per-group counts, due rows show 已到期, a
// toggle ticks rows off, ✕ removes. ＋ 添加待办 opens TodoDialog.
// refreshOn: turnEnd | sessionSwitch | manual — the store re-pulls on
// todos://changed (MCP writes, due tick, command layer).
import { computed, onMounted, ref, watch } from 'vue';
import { useChatStore } from '../stores/chat';
import { usePrefsStore } from '../stores/prefs';
import { useTodosStore, type TodoRow } from '../stores/todos';
import TodoDialog from '../components/chat/TodoDialog.vue';
import WarScrollBar from '../components/war/WarScrollBar.vue';

const chat = useChatStore();
const prefs = usePrefsStore();
const todos = useTodosStore();

const dialogOpen = ref(false);
const listEl = ref<HTMLElement | null>(null);

const sessionGroup = computed<TodoRow[]>(() => todos.groups.session);
const projectGroup = computed<TodoRow[]>(() => todos.groups.project);
const globalGroup = computed<TodoRow[]>(() => todos.groups.global);

const canProject = computed(() => !!chat.projectDir);

onMounted(() => {
  void todos.init();
  void todos.load(chat.sessionId, chat.projectDir);
});
watch(() => chat.sessionId, () => void todos.load(chat.sessionId, chat.projectDir)); // sessionSwitch
watch(() => chat.projectDir, () => void todos.load(chat.sessionId, chat.projectDir));

function dueLine(r: TodoRow): string {
  if (r.dueAtMs <= 0) return '';
  const d = new Date(r.dueAtMs);
  const p = (n: number) => String(n).padStart(2, '0');
  const stamp = `${p(d.getHours())}:${p(d.getMinutes())}`;
  if (r.dueAtMs <= Date.now()) return `${stamp} · 已到期`;
  return `${stamp} · 到期`;
}

function projectName(): string {
  const dir = chat.projectDir.replace(/[\\/]+$/, '');
  const parts = dir.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? dir;
}
</script>

<template>
  <div class="todop">
    <div class="todop__list-wrap">
      <div ref="listEl" class="todop__list">
        <!-- 本会话 -->
        <div class="todop__group-title" :style="{ fontSize: prefs.fs(11) + 'px' }">
          本会话 <span class="todop__count">{{ sessionGroup.length }}</span>
        </div>
        <div v-if="sessionGroup.length === 0" class="todop__empty" :style="{ fontSize: prefs.fs(11) + 'px' }">
          （无）
        </div>
        <div v-for="r in sessionGroup" :key="r.id" class="todop__row" @click="todos.toggle(r.id)">
          <span class="todop__check" :class="{ on: r.done }">✓</span>
          <span class="todop__title" :title="r.title" :style="{ fontSize: prefs.fs(12) + 'px' }">{{
            r.title
          }}</span>
          <span v-if="dueLine(r)" class="todop__due" :class="{ overdue: r.dueAtMs <= Date.now() }" :style="{ fontSize: prefs.fs(10) + 'px' }">{{
            dueLine(r)
          }}</span>
          <span class="todop__del" title="删除" @click.stop="todos.remove(r.id)">✕</span>
        </div>

        <!-- 本项目 -->
        <div class="todop__group-title" :style="{ fontSize: prefs.fs(11) + 'px' }">
          <template v-if="canProject">
            本项目（{{ projectName() }}） <span class="todop__count">{{ projectGroup.length }}</span>
          </template>
          <template v-else>本项目（未关联项目）</template>
        </div>
        <div v-if="projectGroup.length === 0" class="todop__empty" :style="{ fontSize: prefs.fs(11) + 'px' }">
          （无）
        </div>
        <div v-for="r in projectGroup" :key="r.id" class="todop__row" @click="todos.toggle(r.id)">
          <span class="todop__check" :class="{ on: r.done }">✓</span>
          <span class="todop__title" :title="r.title" :style="{ fontSize: prefs.fs(12) + 'px' }">{{
            r.title
          }}</span>
          <span v-if="r.dueAtMs > 0" class="todop__due" :style="{ fontSize: prefs.fs(10) + 'px' }">{{
            r.dueAtMs <= Date.now() ? '已到期' : '到期'
          }}</span>
          <span class="todop__del" title="删除" @click.stop="todos.remove(r.id)">✕</span>
        </div>

        <!-- 全局 -->
        <div class="todop__group-title" :style="{ fontSize: prefs.fs(11) + 'px' }">
          全局 <span class="todop__count">{{ globalGroup.length }}</span>
        </div>
        <div v-if="globalGroup.length === 0" class="todop__empty" :style="{ fontSize: prefs.fs(11) + 'px' }">
          （无）
        </div>
        <div v-for="r in globalGroup" :key="r.id" class="todop__row" @click="todos.toggle(r.id)">
          <span class="todop__check" :class="{ on: r.done }">✓</span>
          <span class="todop__title" :title="r.title" :style="{ fontSize: prefs.fs(12) + 'px' }">{{
            r.title
          }}</span>
          <span v-if="dueLine(r)" class="todop__due" :class="{ overdue: r.dueAtMs <= Date.now() }" :style="{ fontSize: prefs.fs(10) + 'px' }">{{
            dueLine(r)
          }}</span>
          <span class="todop__del" title="删除" @click.stop="todos.remove(r.id)">✕</span>
        </div>
      </div>
      <WarScrollBar :target="listEl" />
    </div>
    <div class="todop__footer">
      <span class="todop__add" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="dialogOpen = true"
        >＋ 添加待办</span
      >
    </div>
    <TodoDialog :open="dialogOpen" @update:open="(v) => (dialogOpen = v)" />
  </div>
</template>

<style scoped>
.todop {
  display: flex;
  flex-direction: column;
  gap: 6px;
  height: 100%;
  min-height: 0;
  font-family: SimSun, serif;
}

.todop__list-wrap {
  flex: 1;
  min-height: 0;
  display: flex;
}

.todop__list {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none; /* native bar hidden — the WC3 WarScrollBar replaces it */
}

.todop__group-title {
  color: var(--war-gold-dim);
  padding: 6px 0 2px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.todop__count {
  color: var(--war-text-muted);
}

.todop__empty {
  color: var(--war-text-faint);
  padding: 2px 0 4px;
}

.todop__row {
  display: flex;
  align-items: center;
  gap: 5px;
  height: 22px;
  cursor: pointer;
  user-select: none;
}

.todop__row:hover {
  background: #32509633;
}

.todop__check {
  flex: none;
  width: 12px;
  height: 12px;
  border: 1px solid #4a5265;
  border-radius: 2px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 9px;
  color: transparent;
}

.todop__check.on {
  color: #57d977;
  border-color: #57d977;
}

.todop__title {
  flex: 1;
  min-width: 0;
  color: #d0d6e0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.todop__due {
  flex: none;
  color: var(--war-text-muted);
  white-space: nowrap;
}

.todop__due.overdue {
  color: var(--war-error);
}

.todop__del {
  flex: none;
  color: var(--war-text-faint);
  padding: 0 3px;
}

.todop__del:hover {
  color: var(--war-error);
}

.todop__footer {
  flex: none;
  display: flex;
  justify-content: center;
}

.todop__add {
  color: var(--war-gold);
  user-select: none;
}

.todop__add:hover {
  color: var(--war-gold-bright);
}
</style>

<script setup lang="ts">
// Todo page: the full board. 待办 grouped by scope — 全局 first, then 项目级
// (grouped by project dir basename), then 会话级 (grouped by session title);
// 已完成 below. Add box scopes the new row like the panel dialog (session
// needs an active session, project needs a project dir — both fall back to
// 全局 when unavailable). Due rows show 已到期.
import { computed, onMounted, ref, watch } from 'vue';
import { useNavStore } from '../stores/nav';
import { usePrefsStore } from '../stores/prefs';
import { useChatStore } from '../stores/chat';
import { useSessionsStore } from '../stores/sessions';
import { useTodosStore, type TodoRow, type TodoScope } from '../stores/todos';
import WarButton from '../components/war/WarButton.vue';

const nav = useNavStore();
const prefs = usePrefsStore();
const todos = useTodosStore();
const chat = useChatStore();
const sessions = useSessionsStore();

onMounted(async () => {
  await sessions.reloadAll();
  void todos.init();
  await todos.load(chat.sessionId, chat.projectDir);
});

const addScope = ref<TodoScope>('global');
const addText = ref('');
const addMinutes = ref(10);
const addDue = ref(false);

const canSession = computed(() => !!chat.sessionId);
const canProject = computed(() => !!chat.projectDir);

watch(
  () => nav.page,
  (p) => {
    if (p !== 'todo') return;
    void sessions.reloadAll();
    void todos.refresh();
  },
);

// ---- grouping helpers ----
const globalGroup = computed<TodoRow[]>(() => todos.groups.global);

/** project rows grouped by projectDir basename, group order by newest. */
const projectGroups = computed(() => {
  const map = new Map<string, TodoRow[]>();
  for (const r of todos.groups.project) {
    const dir = r.projectDir.replace(/[\\/]+$/, '');
    const parts = dir.split(/[\\/]/).filter(Boolean);
    const key = parts[parts.length - 1] ?? dir;
    const list = map.get(key) ?? [];
    list.push(r);
    map.set(key, list);
  }
  return [...map.entries()].sort((a, b) => b[1][0].createdAt - a[1][0].createdAt);
});

/** session rows grouped by session title (sessions index lookup). */
const sessionGroups = computed(() => {
  const titleOf = new Map(sessions.all.map((s) => [s.id, s.title]));
  const map = new Map<string, TodoRow[]>();
  for (const r of todos.groups.session) {
    const key = titleOf.get(r.sessionId) ?? `会话 ${r.sessionId.slice(0, 8)}`;
    const list = map.get(key) ?? [];
    list.push(r);
    map.set(key, list);
  }
  return [...map.entries()].sort((a, b) => b[1][0].createdAt - a[1][0].createdAt);
});

const doneGroup = computed<TodoRow[]>(() => todos.groups.done);

// ---- add ----
function addMinutesValid(): number {
  const m = Math.floor(Number(addMinutes.value));
  return Number.isFinite(m) && m > 0 ? m : 0;
}

const canAdd = computed(
  () =>
    !!addText.value.trim() &&
    (addScope.value !== 'session' || canSession.value) &&
    (addScope.value !== 'project' || canProject.value),
);

async function add(): Promise<void> {
  if (!canAdd.value) return;
  const dueAtMs =
    addDue.value && addScope.value !== 'project' && addMinutesValid() > 0
      ? Date.now() + addMinutesValid() * 60_000
      : 0;
  await todos.add(addText.value, addScope.value, chat.sessionId, chat.projectDir, dueAtMs, 'popup');
  addText.value = '';
}

// ---- row helpers ----
const p2 = (n: number) => String(n).padStart(2, '0');

function dueLine(r: TodoRow): string {
  if (r.dueAtMs <= 0) return '';
  const d = new Date(r.dueAtMs);
  const stamp = `${p2(d.getMonth() + 1)}-${p2(d.getDate())} ${p2(d.getHours())}:${p2(d.getMinutes())}`;
  if (r.dueAtMs <= Date.now()) return `${stamp} · 已到期`;
  return `${stamp} · 到期`;
}

function formatTime(r: TodoRow): string {
  const d = new Date(r.done ? r.doneAt : r.createdAt);
  return `${p2(d.getMonth() + 1)}-${p2(d.getDate())} ${p2(d.getHours())}:${p2(d.getMinutes())}`;
}

function stats(): string {
  const n = todos.pending.length;
  const overdue = todos.overdue.length;
  return `待办 ${n} 项 · 已完成 ${todos.done.length} 项${overdue > 0 ? ` · 已到期 ${overdue} 项` : ''}`;
}
</script>

<template>
  <div class="todo">
    <div class="todo__frame">
      <div class="todo__col">
        <div class="todo__title war-font-title war-outline-black" :style="{ fontSize: prefs.fs(18) + 'px' }">
          待办事项
        </div>
        <div class="todo__stats" :style="{ fontSize: prefs.fs(12) + 'px' }">{{ stats() }}</div>

        <!-- add row -->
        <div class="todo__add-row">
          <input
            v-model="addText"
            class="war-input todo__input"
            placeholder="输入待办内容，回车或点击添加"
            :style="{ fontSize: prefs.fs(13) + 'px' }"
            @keydown.enter.prevent="add"
          />
          <span
            class="todo__scope"
            :class="{ active: addScope === 'global' }"
            :style="{ fontSize: prefs.fs(12) + 'px' }"
            @click="addScope = 'global'"
            >全局</span
          >
          <span
            class="todo__scope"
            :class="{ active: addScope === 'project', disabled: !canProject }"
            :style="{ fontSize: prefs.fs(12) + 'px' }"
            @click="canProject && (addScope = 'project')"
            >项目</span
          >
          <span
            class="todo__scope"
            :class="{ active: addScope === 'session', disabled: !canSession }"
            :style="{ fontSize: prefs.fs(12) + 'px' }"
            @click="canSession && (addScope = 'session')"
            >会话</span
          >
          <label class="todo__due-toggle" :style="{ fontSize: prefs.fs(12) + 'px' }">
            <input v-model="addDue" type="checkbox" :disabled="addScope === 'project'" />
            到期
          </label>
          <template v-if="addDue && addScope !== 'project'">
            <input
              v-model.number="addMinutes"
              type="number"
              min="1"
              class="war-input todo__minutes"
              :style="{ fontSize: prefs.fs(13) + 'px' }"
            />
            <span class="todo__due-label" :style="{ fontSize: prefs.fs(12) + 'px' }">分钟后</span>
          </template>
          <WarButton skin="dialog" :width="100" :art-aspect="5" text="添加" :enabled="canAdd" @activated="add" />
        </div>

        <div class="todo__scroll">
          <!-- 全局 -->
          <div class="todo__section" :style="{ fontSize: prefs.fs(14) + 'px' }">全局</div>
          <div class="todo__list">
            <div
              v-for="(row, i) in globalGroup"
              :key="row.id"
              class="todo__row"
              :class="{ zebra: i % 2 === 1 }"
              @click="todos.toggle(row.id)"
            >
              <span class="todo__check">✓</span>
              <span class="todo__row-title" :class="{ overdue: row.dueAtMs > 0 && row.dueAtMs <= Date.now() }">{{
                row.title
              }}</span>
              <span v-if="dueLine(row)" class="todo__due" :class="{ overdue: row.dueAtMs <= Date.now() }">{{
                dueLine(row)
              }}</span>
              <span class="todo__time">{{ formatTime(row) }}</span>
              <span class="todo__del" @click.stop="todos.remove(row.id)">✕</span>
            </div>
            <div v-if="globalGroup.length === 0" class="todo__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">
              （暂无全局待办）
            </div>
          </div>

          <!-- 项目级 -->
          <div class="todo__section" :style="{ fontSize: prefs.fs(14) + 'px' }">项目</div>
          <template v-if="projectGroups.length === 0">
            <div class="todo__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">（暂无项目待办）</div>
          </template>
          <template v-for="[name, rows] in projectGroups" :key="name">
            <div class="todo__group" :style="{ fontSize: prefs.fs(12) + 'px' }">{{ name }}</div>
            <div class="todo__list">
              <div
                v-for="(row, i) in rows"
                :key="row.id"
                class="todo__row"
                :class="{ zebra: i % 2 === 1 }"
                @click="todos.toggle(row.id)"
              >
                <span class="todo__check">✓</span>
                <span class="todo__row-title" :class="{ overdue: row.dueAtMs > 0 && row.dueAtMs <= Date.now() }">{{
                  row.title
                }}</span>
                <span v-if="dueLine(row)" class="todo__due" :class="{ overdue: row.dueAtMs <= Date.now() }">{{
                  dueLine(row)
                }}</span>
                <span class="todo__time">{{ formatTime(row) }}</span>
                <span class="todo__del" @click.stop="todos.remove(row.id)">✕</span>
              </div>
            </div>
          </template>

          <!-- 会话级 -->
          <div class="todo__section" :style="{ fontSize: prefs.fs(14) + 'px' }">会话</div>
          <template v-if="sessionGroups.length === 0">
            <div class="todo__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">（暂无会话待办）</div>
          </template>
          <template v-for="[name, rows] in sessionGroups" :key="name">
            <div class="todo__group" :style="{ fontSize: prefs.fs(12) + 'px' }">{{ name }}</div>
            <div class="todo__list">
              <div
                v-for="(row, i) in rows"
                :key="row.id"
                class="todo__row"
                :class="{ zebra: i % 2 === 1 }"
                @click="todos.toggle(row.id)"
              >
                <span class="todo__check">✓</span>
                <span class="todo__row-title" :class="{ overdue: row.dueAtMs > 0 && row.dueAtMs <= Date.now() }">{{
                  row.title
                }}</span>
                <span v-if="dueLine(row)" class="todo__due" :class="{ overdue: row.dueAtMs <= Date.now() }">{{
                  dueLine(row)
                }}</span>
                <span class="todo__time">{{ formatTime(row) }}</span>
                <span class="todo__del" @click.stop="todos.remove(row.id)">✕</span>
              </div>
            </div>
          </template>

          <!-- 已完成 -->
          <div class="todo__section todo__section--done" :style="{ fontSize: prefs.fs(14) + 'px' }">已完成</div>
          <div class="todo__list todo__list--done">
            <div
              v-for="(row, i) in doneGroup"
              :key="row.id"
              class="todo__row todo__row--done"
              :class="{ zebra: i % 2 === 1 }"
              @click="todos.toggle(row.id)"
            >
              <span class="todo__check todo__check--on">✓</span>
              <span class="todo__row-title">{{ row.title }}</span>
              <span class="todo__time">{{ formatTime(row) }}</span>
              <span class="todo__del" @click.stop="todos.remove(row.id)">✕</span>
            </div>
            <div v-if="doneGroup.length === 0" class="todo__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">
              （暂无已完成）
            </div>
          </div>
        </div>

        <div class="todo__actions">
          <WarButton
            skin="dialog"
            :width="150"
            :art-aspect="5.34"
            text="清除已完成"
            :enabled="todos.done.length > 0"
            @activated="todos.clearDone()"
          />
          <span class="todo__spring"></span>
          <WarButton skin="dialog" :width="150" :art-aspect="5.34" text="返回(B)" shortcut-key="B" :shortcut-active="nav.page === 'todo'" @activated="nav.goMain()" />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.todo {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  padding: 24px;
  box-sizing: border-box;
}

.todo__frame {
  width: calc(62% - 5px); /* same as before: leftW = (w-gap)*0.62, gap 10 */
  max-width: 860px;
  height: 100%;
  border: 1px solid #6a5a3f;
  background: #0d1116f0;
  border-radius: 3px;
  padding: 14px 18px;
  box-sizing: border-box;
}

.todo__col {
  display: flex;
  flex-direction: column;
  gap: 8px;
  height: 100%;
  min-height: 0;
  font-family: SimSun, serif;
}

.todo__title {
  flex: none;
  color: var(--war-gold);
  font-weight: bold;
  text-align: center;
}

.todo__stats {
  flex: none;
  color: var(--war-text-muted);
  text-align: center;
}

.todo__add-row {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
}

.todo__input {
  flex: 1;
  min-width: 0;
}

.todo__scope {
  flex: none;
  padding: 3px 10px;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: var(--war-text-dim);
  user-select: none;
  cursor: pointer;
}

.todo__scope.active {
  color: var(--war-gold-bright);
  border-color: var(--war-gold-dim);
}

.todo__scope.disabled {
  color: var(--war-text-faint);
  cursor: not-allowed;
}

.todo__due-toggle {
  display: flex;
  align-items: center;
  gap: 4px;
  color: var(--war-text-dim);
  user-select: none;
}

.todo__minutes {
  width: 70px;
  flex: none;
}

.todo__due-label {
  color: var(--war-text-muted);
  flex: none;
}

.todo__scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
}

.todo__section {
  color: var(--war-gold);
  margin: 10px 0 4px;
}

.todo__section--done {
  color: var(--war-text-muted);
}

.todo__group {
  color: var(--war-gold-dim);
  margin: 4px 0 2px;
  padding-left: 8px;
}

.todo__list {
  border-top: 1px solid #1a2230;
}

.todo__row {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 26px;
  padding: 0 6px;
  cursor: pointer;
  user-select: none;
}

.todo__row.zebra {
  background: #ffffff04;
}

.todo__row:hover {
  background: #1a2334;
}

.todo__row--done .todo__row-title {
  color: var(--war-text-faint);
  text-decoration: line-through;
}

.todo__check {
  flex: none;
  width: 14px;
  height: 14px;
  border: 1px solid #4a5265;
  border-radius: 2px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 10px;
  color: transparent;
}

.todo__check--on {
  color: #57d977;
  border-color: #57d977;
}

.todo__row-title {
  flex: 1;
  min-width: 0;
  color: #d0d6e0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.todo__row-title.overdue {
  color: var(--war-error);
}

.todo__due {
  flex: none;
  color: var(--war-text-muted);
  white-space: nowrap;
}

.todo__due.overdue {
  color: var(--war-error);
}

.todo__time {
  flex: none;
  color: var(--war-text-faint);
  font-size: 11px;
  white-space: nowrap;
}

.todo__del {
  flex: none;
  color: var(--war-text-faint);
  padding: 0 4px;
}

.todo__del:hover {
  color: var(--war-error);
}

.todo__empty {
  color: var(--war-text-faint);
  text-align: center;
  padding: 8px 0;
}

.todo__actions {
  flex: none;
  display: flex;
  align-items: center;
  gap: 12px;
}

.todo__spring {
  flex: 1;
}
</style>

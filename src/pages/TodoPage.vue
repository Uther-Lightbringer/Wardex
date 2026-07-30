<script setup lang="ts">
// Todo page (TodoPage.qml): app-level personal board backed by todos.json
// via Rust commands. Two sections 待办 / 已完成; done rows strike through and
// gray out. Single wide panel, leftW = (w-gap)*0.62, embed 52.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import PageShell from '../components/PageShell.vue';
import WarFrame from '../components/war/WarFrame.vue';
import WarButton from '../components/war/WarButton.vue';
import { useNavStore } from '../stores/nav';
import { usePrefsStore } from '../stores/prefs';
import { useTodosStore, type TodoRow } from '../stores/todos';

const nav = useNavStore();
const prefs = usePrefsStore();
const todos = useTodosStore();

onMounted(() => void todos.load());

// Esc returns to the main menu (same as the 返回(B) button).
function onPageKey(e: KeyboardEvent): void {
  if (nav.page !== 'todo') return;
  if (e.key === 'Escape') void nav.goMain();
}
onMounted(() => window.addEventListener('keydown', onPageKey));
onBeforeUnmount(() => window.removeEventListener('keydown', onPageKey));

const input = ref('');
const canAdd = computed(() => input.value.trim().length > 0);

function add(): void {
  if (!canAdd.value) return;
  void todos.add(input.value);
  input.value = '';
}

function stats(): string {
  return `待办 ${todos.pending.length} 项 · 已完成 ${todos.done.length} 项`;
}

function formatTime(row: TodoRow): string {
  const ms = row.done && row.doneAt ? row.doneAt : row.createdAt;
  if (!ms) return '';
  const d = new Date(Number(ms));
  const p = (n: number): string => String(n).padStart(2, '0');
  return `${d.getMonth() + 1}-${d.getDate()} ${p(d.getHours())}:${p(d.getMinutes())}`;
}
</script>

<template>
  <PageShell :embed="52">
    <div class="todo">
      <WarFrame
        class="todo__frame"
        src="/assets/ui/frames/frame_iron_panel.png"
        :slice="[96, 110, 69, 108]"
        :hole="[56, 25, 21, 24]"
        :content-left-extra="16"
      >
        <div class="todo__col">
          <div class="todo__title war-font-title war-outline-black" :style="{ fontSize: prefs.fs(18) + 'px' }">
            待办事项
          </div>
          <div class="todo__stats" :style="{ fontSize: prefs.fs(12) + 'px' }">{{ stats() }}</div>

          <!-- add row -->
          <div class="todo__add-row">
            <input
              v-model="input"
              class="war-input todo__input"
              placeholder="输入待办内容，回车或点击添加"
              maxlength="200"
              :style="{ fontSize: prefs.fs(14) + 'px' }"
              @keydown.enter="add"
            />
            <WarButton skin="dialog" :width="120" :art-aspect="5.34" text="添加" :enabled="canAdd" @activated="add" />
          </div>

          <div class="todo__section" :style="{ fontSize: prefs.fs(14) + 'px' }">待办</div>
          <div class="todo__list todo__list--pending">
            <div
              v-for="(row, i) in todos.pending"
              :key="row.id"
              class="todo__row"
              :class="{ zebra: i % 2 === 1 }"
              :style="{ fontSize: prefs.fs(14) + 'px' }"
              @click="todos.toggle(row.id)"
            >
              <span class="todo__check">✓</span>
              <span class="todo__row-title">{{ row.title }}</span>
              <span class="todo__time">{{ formatTime(row) }}</span>
              <span class="todo__del" @click.stop="todos.remove(row.id)">✕</span>
            </div>
            <div v-if="todos.pending.length === 0" class="todo__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">
              （暂无待办）
            </div>
          </div>

          <div class="todo__section todo__section--done" :style="{ fontSize: prefs.fs(14) + 'px' }">已完成</div>
          <div class="todo__list todo__list--done">
            <div
              v-for="(row, i) in todos.done"
              :key="row.id"
              class="todo__row todo__row--done"
              :class="{ zebra: i % 2 === 1 }"
              :style="{ fontSize: prefs.fs(14) + 'px' }"
              @click="todos.toggle(row.id)"
            >
              <span class="todo__check todo__check--on">✓</span>
              <span class="todo__row-title">{{ row.title }}</span>
              <span class="todo__time">{{ formatTime(row) }}</span>
              <span class="todo__del" @click.stop="todos.remove(row.id)">✕</span>
            </div>
            <div v-if="todos.done.length === 0" class="todo__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">
              （暂无已完成）
            </div>
          </div>

          <!-- bottom actions -->
          <div class="todo__actions">
            <WarButton
              skin="dialog"
              :width="170"
              :art-aspect="5.34"
              text="清除已完成"
              :enabled="todos.done.length > 0"
              @activated="todos.clearDone()"
            />
            <span class="todo__spring"></span>
            <WarButton skin="dialog" :width="150" :art-aspect="5.34" text="返回(B)" shortcut-key="B" :shortcut-active="nav.page === 'todo'" @activated="nav.goMain()" />
          </div>
        </div>
      </WarFrame>
    </div>
  </PageShell>
</template>

<style scoped>
.todo {
  height: 100%;
  padding-top: 4px;
  padding-bottom: 8px;
  box-sizing: border-box;
}

.todo__frame {
  width: calc(62% - 5px); /* leftW = (w-gap)*0.62, gap 10 */
  height: 100%;
}

.todo__col {
  display: flex;
  flex-direction: column;
  gap: 8px;
  height: 100%;
}

.todo__title {
  color: var(--war-text-dim);
  flex: none;
}

.todo__stats {
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  flex: none;
}

.todo__add-row {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 36px;
  flex: none;
}

.todo__input {
  flex: 1;
  height: 32px;
}

.todo__section {
  color: var(--war-gold);
  font-family: SimSun, serif;
  flex: none;
}

.todo__section--done {
  color: var(--war-text-muted);
}

.todo__list {
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

/* old TodoPage heights: (parent-236) × 0.55 / 0.45 */
.todo__list--pending {
  flex: 55;
}

.todo__list--done {
  flex: 45;
}

.todo__row {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 40px;
  flex: none;
  padding: 5px 6px;
  color: var(--war-text);
  font-family: SimSun, serif;
  border: 1px solid #1c2430;
  border-radius: 3px;
  background: #0e121899;
  box-sizing: border-box;
}

.todo__row.zebra {
  background: #14182099;
}

.todo__row:hover {
  background: #32509640;
}

.todo__row--done {
  color: var(--war-text-muted);
  background: #10141866;
  border-color: #141a24;
}

.todo__row--done .todo__row-title {
  text-decoration: line-through;
}

/* bordered checkbox square (old TodoRow: 18×18, gold border when done) */
.todo__check {
  flex: none;
  width: 18px;
  height: 18px;
  border: 1px solid #3a4658;
  border-radius: 2px;
  background: #15192299;
  color: transparent; /* ✓ hidden until done */
  font-size: 13px;
  font-weight: bold;
  text-align: center;
  line-height: 17px;
  box-sizing: border-box;
}

.todo__check--on {
  border-color: var(--war-gold);
  color: var(--war-gold);
}

.todo__row-title {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.todo__time {
  color: var(--war-text-muted);
  font-size: 0.85em;
  flex: none;
}

.todo__del {
  color: var(--war-text-muted);
  flex: none;
  padding: 0 4px;
}

.todo__del:hover {
  color: var(--war-error);
}

.todo__empty {
  color: var(--war-text-faint);
  font-family: SimSun, serif;
  text-align: center;
  padding: 12px 0;
}

.todo__actions {
  display: flex;
  align-items: center;
  gap: 12px;
  flex: none;
}

.todo__spring {
  flex: 1;
}
</style>

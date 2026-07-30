<script setup lang="ts">
// Left session rail (features/chat.md §5): sessions of the current project,
// pinned first; status dots (running green / waiting gold / idle gray with a
// 550ms breathing animation while non-idle), unread "NEW ·" prefix, instant
// title filter, right-click menu (pin / inline rename / copy transcript /
// ask-based-on / delete with confirm).
import { computed, ref } from 'vue';
import { useChatStore } from '../../stores/chat';
import { useSessionsStore, type RailSession } from '../../stores/sessions';
import { usePrefsStore } from '../../stores/prefs';
import WarMenu, { type WarMenuItem } from '../war/WarMenu.vue';
import WarDialog from '../war/WarDialog.vue';
import WarButton from '../war/WarButton.vue';

const chat = useChatStore();
const sessions = useSessionsStore();
const prefs = usePrefsStore();

const filter = ref('');

const rows = computed(() => {
  const f = filter.value.trim().toLowerCase();
  if (!f) return sessions.rail;
  return sessions.rail.filter((s) => s.title.toLowerCase().includes(f));
});

function subLine(s: RailSession): string {
  const dot = sessions.dotState(s.sessionId);
  const unread = sessions.unreadIds.includes(s.sessionId);
  let line = `${s.messageCount} 条`;
  if (dot === 'running') line += ' · 执行中';
  if (dot === 'waiting') line += ' · 等待确认';
  if (unread) line = 'NEW · ' + line;
  return line;
}

function onPick(s: RailSession): void {
  if (s.sessionId === chat.sessionId || renamingId.value) return;
  // Deferred a tick (old code: 延迟一拍执行避免事件栈内切页).
  void Promise.resolve().then(() => chat.openSession(s.sessionId));
}

// ---- context menu ----
const menuVisible = ref(false);
const menuX = ref(0);
const menuY = ref(0);
const menuSession = ref<RailSession | null>(null);

const menuItems = computed<WarMenuItem[]>(() => {
  const s = menuSession.value;
  if (!s) return [];
  return [
    { label: s.pinned ? '取消置顶' : '置顶会话' },
    { label: '重命名会话' },
    { label: '复制会话内容' },
    { label: '基于此提问' },
    { label: '删除会话' },
  ];
});

function onContextMenu(e: MouseEvent, s: RailSession): void {
  menuSession.value = s;
  menuX.value = e.clientX;
  menuY.value = e.clientY;
  menuVisible.value = true;
}

function onMenuSelect(i: number): void {
  const s = menuSession.value;
  if (!s) return;
  switch (i) {
    case 0:
      void sessions.setPinned(s.sessionId, !s.pinned).then(() => sessions.refresh(chat.projectDir));
      break;
    case 1:
      renamingId.value = s.sessionId;
      renameText.value = s.title;
      break;
    case 2:
      void sessions.copyTranscript(s.sessionId).then((err) => {
        if (err) chat.status = { ...chat.status, lastError: err };
      });
      break;
    case 3:
      // New empty session in the same project + composer prefill.
      sessions.pendingComposerText = `基于会话「${s.title}」：`;
      void chat.newSession();
      break;
    case 4:
      deleteTarget.value = s;
      break;
  }
}

// ---- inline rename (Enter submits non-empty trim ≤48 chars; Esc cancels) ----
const renamingId = ref('');
const renameText = ref('');

async function commitRename(): Promise<void> {
  const id = renamingId.value;
  const title = renameText.value.trim().slice(0, 48);
  renamingId.value = '';
  if (!id || !title) return;
  try {
    await sessions.rename(id, title);
    await sessions.refresh(chat.projectDir);
    await chat.refreshMeta();
  } catch (e) {
    console.warn('[rail] rename failed', e);
  }
}

function cancelRename(): void {
  renamingId.value = '';
}

// ---- delete confirm ----
const deleteTarget = ref<RailSession | null>(null);
const deleteOpen = computed({
  get: () => deleteTarget.value !== null,
  set: (v: boolean) => {
    if (!v) deleteTarget.value = null;
  },
});

async function confirmDelete(): Promise<void> {
  const s = deleteTarget.value;
  deleteTarget.value = null;
  if (!s) return;
  await chat.deleteSession(s.sessionId);
}

// Esc while renaming belongs to the input, not the page-back shortcut.
defineExpose({ renaming: renamingId });
</script>

<template>
  <div class="rail">
    <div class="rail__title war-font-title war-outline-black" :style="{ fontSize: prefs.fs(15) + 'px' }">
      本项目会话
    </div>

    <button class="rail__new" :style="{ fontSize: prefs.fs(12) + 'px' }" @click="chat.newSession()">
      ＋ 新会话
    </button>

    <input
      v-model="filter"
      class="war-input rail__search"
      placeholder="搜索会话…"
      :style="{ fontSize: prefs.fs(12) + 'px' }"
    />

    <div class="rail__list">
      <div v-if="rows.length === 0" class="rail__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">
        （无会话）
      </div>
      <div
        v-for="s in rows"
        :key="s.sessionId"
        class="rail__row"
        :class="{ active: s.sessionId === chat.sessionId }"
        @click="onPick(s)"
        @contextmenu.prevent="onContextMenu($event, s)"
      >
        <span
          class="rail__dot"
          :class="[sessions.dotState(s.sessionId), { breath: sessions.dotState(s.sessionId) !== 'idle' }]"
        ></span>
        <div class="rail__text">
          <div class="rail__name" :style="{ fontSize: prefs.fs(12) + 'px' }">
            <template v-if="renamingId === s.sessionId">
              <input
                v-model="renameText"
                class="rail__rename"
                :style="{ fontSize: prefs.fs(12) + 'px' }"
                maxlength="48"
                v-focus
                @keydown.enter.prevent="commitRename"
                @keydown.esc.stop.prevent="cancelRename"
                @click.stop
                @blur="commitRename"
              />
            </template>
            <template v-else>{{ s.title }}</template>
          </div>
          <div class="rail__sub" :style="{ fontSize: prefs.fs(10) + 'px' }">{{ subLine(s) }}</div>
        </div>
        <span v-if="s.pinned" class="rail__pin">📌</span>
      </div>
    </div>

    <div class="rail__legend" :style="{ fontSize: prefs.fs(10) + 'px' }">
      <span class="lg running">●</span> 执行中
      <span class="lg waiting">●</span> 等待
      <span class="lg idle">●</span> 空闲
    </div>

    <WarMenu v-model:visible="menuVisible" :x="menuX" :y="menuY" :items="menuItems" @select="onMenuSelect" />

    <WarDialog
      v-model:open="deleteOpen"
      title-text="删除会话"
      :message-text="'确定删除这条会话及其全部消息吗？\n该操作不可撤销。'"
    >
      <WarButton skin="dialog" :width="190" text="删除" @activated="confirmDelete" />
      <WarButton skin="dialog" :width="190" text="取消" @activated="deleteTarget = null" />
    </WarDialog>
  </div>
</template>

<script lang="ts">
// v-focus: focus the inline rename input when it mounts.
export default {
  directives: {
    focus: {
      mounted(el: HTMLElement) {
        el.focus();
        (el as HTMLInputElement).select?.();
      },
    },
  },
};
</script>

<style scoped>
.rail {
  display: flex;
  flex-direction: column;
  gap: 8px;
  height: 100%;
  min-height: 0;
  font-family: SimSun, serif;
}

.rail__title {
  flex: none;
  color: var(--war-text-dim);
  text-align: center;
}

.rail__new {
  flex: none;
  height: 28px;
  background: #10141f;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: var(--war-gold);
  font-family: SimSun, serif;
}

.rail__new:hover {
  border-color: var(--war-gold-input);
  color: var(--war-gold-bright);
}

.rail__search {
  flex: none;
  height: 28px;
}

.rail__list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.rail__empty {
  color: var(--war-text-faint);
  text-align: center;
  padding: 12px 0;
}

.rail__row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 6px;
  border: 1px solid #0a0c10;
  border-radius: 2px;
  user-select: none;
}

.rail__row:hover {
  background: #32509640;
  border-color: #4a3c14;
}

.rail__row.active {
  border-color: #8a6f24;
}

.rail__row.active .rail__name {
  color: var(--war-gold);
  font-weight: bold;
}

.rail__dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
}

.rail__dot.running {
  background: #57d977;
}

.rail__dot.waiting {
  background: #f2cf6b;
}

.rail__dot.idle {
  background: #4a5265;
}

.rail__dot.breath {
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

.rail__text {
  flex: 1;
  min-width: 0;
}

.rail__name {
  color: var(--war-text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.rail__rename {
  width: 100%;
  background: #10141f;
  border: 1px solid #8a6f24;
  border-radius: 2px;
  color: var(--war-text);
  font-family: SimSun, serif;
  padding: 1px 4px;
  outline: none;
}

.rail__sub {
  color: var(--war-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.rail__pin {
  flex: none;
  font-size: 10px;
}

.rail__legend {
  flex: none;
  color: var(--war-text-muted);
  text-align: center;
  user-select: none;
}

.rail__legend .lg {
  margin-left: 6px;
}

.rail__legend .lg:first-child {
  margin-left: 0;
}

.lg.running {
  color: #57d977;
}

.lg.waiting {
  color: #f2cf6b;
}

.lg.idle {
  color: #4a5265;
}
</style>

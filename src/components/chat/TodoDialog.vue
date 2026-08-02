<script setup lang="ts">
// Add-todo dialog (replaces the old add-reminder dialog): content + scope
// picker (会话 / 项目 / 全局, context-disabled when missing) + optional due
// time (N 分钟后; 0 = 纯清单). Ctrl+Enter confirms, Esc / mask cancels.
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';
import { useTodosStore, type TodoScope } from '../../stores/todos';
import WarButton from '../war/WarButton.vue';

const props = defineProps<{
  open: boolean;
}>();

const emit = defineEmits<{ (e: 'update:open', v: boolean): void }>();

const prefs = usePrefsStore();
const todos = useTodosStore();
const chat = useChatStore();

const content = ref('');
const scope = ref<TodoScope>('session');
const minutes = ref(10);
const dueEnabled = ref(true);
const contentEl = ref<HTMLInputElement | null>(null);

// Context availability: session scope needs an active session, project scope
// a project dir. Default to the first available scope.
const canSession = computed(() => !!chat.sessionId);
const canProject = computed(() => !!chat.projectDir);
const canDue = computed(() => scope.value !== 'project');

watch(
  () => props.open,
  (v) => {
    if (v) {
      content.value = '';
      minutes.value = 10;
      dueEnabled.value = true;
      if (canSession.value) scope.value = 'session';
      else if (canProject.value) scope.value = 'project';
      else scope.value = 'global';
      window.addEventListener('keydown', onKey, true);
      requestAnimationFrame(() => contentEl.value?.focus());
    } else {
      window.removeEventListener('keydown', onKey, true);
    }
  },
);
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true));

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.stopPropagation();
    close();
  } else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    confirm();
  }
}

function minutesValid(): number {
  const m = Math.floor(Number(minutes.value));
  return Number.isFinite(m) && m > 0 ? m : 0;
}

const dueAtMs = computed(() =>
  dueEnabled.value && scope.value !== 'project' && minutesValid() > 0
    ? Date.now() + minutesValid() * 60_000
    : 0,
);

const canConfirm = computed(
  () => !!content.value.trim() && (scope.value !== 'session' || canSession.value),
);

async function confirm(): Promise<void> {
  if (!canConfirm.value) return;
  await todos.add(
    content.value,
    scope.value,
    chat.sessionId,
    chat.projectDir,
    dueAtMs.value,
    'popup',
  );
  emit('update:open', false);
}

function close(): void {
  emit('update:open', false);
}
</script>

<template>
  <Teleport to="body">
    <div v-if="props.open" class="td-mask" @mousedown.self="close">
      <div class="td">
        <div class="td__frame"></div>
        <div class="td__inner">
          <div class="td__head">
            <span class="td__title" :style="{ fontSize: prefs.fs(13) + 'px' }">添加待办</span>
            <span class="td__close" title="关闭" @click="close">✕</span>
          </div>
          <div class="td__body">
            <input
              ref="contentEl"
              v-model="content"
              class="td__field"
              placeholder="待办内容…"
              :style="{ fontSize: prefs.fs(13) + 'px' }"
            />
            <div class="td__scope-row">
              <span
                class="td__scope"
                :class="{ active: scope === 'session', disabled: !canSession }"
                :style="{ fontSize: prefs.fs(12) + 'px' }"
                @click="canSession && (scope = 'session')"
                >本会话</span
              >
              <span
                class="td__scope"
                :class="{ active: scope === 'project', disabled: !canProject }"
                :style="{ fontSize: prefs.fs(12) + 'px' }"
                @click="canProject && (scope = 'project')"
                >本项目</span
              >
              <span
                class="td__scope"
                :class="{ active: scope === 'global' }"
                :style="{ fontSize: prefs.fs(12) + 'px' }"
                @click="scope = 'global'"
                >全局</span
              >
            </div>
            <div class="td__time-row">
              <label class="td__due-toggle" :style="{ fontSize: prefs.fs(12) + 'px' }">
                <input v-model="dueEnabled" type="checkbox" :disabled="scope === 'project'" />
                到期提醒
              </label>
              <template v-if="dueEnabled && scope !== 'project'">
                <input
                  v-model.number="minutes"
                  type="number"
                  min="1"
                  class="td__field td__field--minutes"
                  :style="{ fontSize: prefs.fs(13) + 'px' }"
                />
                <span class="td__time-label" :style="{ fontSize: prefs.fs(12) + 'px' }">分钟后</span>
              </template>
              <span
                v-else-if="scope === 'project'"
                class="td__time-label td__time-label--hint"
                :style="{ fontSize: prefs.fs(11) + 'px' }"
                >到期后自动在项目内新建会话处理</span
              >
            </div>
          </div>
          <div class="td__footer">
            <span class="td__hint" :style="{ fontSize: prefs.fs(10) + 'px' }">Ctrl+Enter 确定 · Esc 取消</span>
            <WarButton
              :width="120"
              :art-aspect="5"
              skin="dialog"
              text="确定"
              :enabled="canConfirm"
              @activated="confirm"
            />
            <WarButton :width="120" :art-aspect="5" skin="dialog" text="取消" @activated="close" />
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.td-mask {
  position: fixed;
  inset: 0;
  z-index: 115;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.td {
  position: relative;
  width: min(480px, 90vw);
  height: min(340px, 80vh);
}

/* frame_popup.png nine-slice (same frame as the other dialogs) */
.td__frame {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 88px 100px 90px 100px;
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.td__inner {
  position: absolute;
  inset: 60px 64px 56px 62px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  min-height: 0;
  font-family: SimSun, serif;
}

.td__head {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.td__title {
  color: var(--war-gold);
  font-weight: bold;
}

.td__close {
  color: var(--war-text-dim);
  padding: 2px 6px;
  user-select: none;
}

.td__close:hover {
  color: var(--war-gold-bright);
}

.td__body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.td__field {
  background: #10141f;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: var(--war-text);
  font-family: SimSun, serif;
  padding: 6px 10px;
  outline: none;
}

.td__field:focus {
  border-color: var(--war-gold-input);
}

.td__field::placeholder {
  color: var(--war-text-faint);
}

.td__scope-row {
  display: flex;
  gap: 8px;
}

.td__scope {
  padding: 4px 14px;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: var(--war-text-dim);
  user-select: none;
  cursor: pointer;
}

.td__scope.active {
  color: var(--war-gold-bright);
  border-color: var(--war-gold-dim);
}

.td__scope.disabled {
  color: var(--war-text-faint);
  cursor: not-allowed;
}

.td__time-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.td__due-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--war-text-dim);
  user-select: none;
}

.td__field--minutes {
  width: 80px;
}

.td__time-label {
  color: var(--war-text-muted);
}

.td__time-label--hint {
  color: var(--war-text-faint);
}

.td__footer {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
}

.td__hint {
  color: var(--war-text-faint);
  margin-right: auto;
}
</style>

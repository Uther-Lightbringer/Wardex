<script setup lang="ts">
// Add-reminder dialog: content input + "N 分钟后" number input, confirmed
// with Ctrl+Enter / 确定. Esc or mask click cancels.
import { onBeforeUnmount, ref, watch } from 'vue';
import { usePrefsStore } from '../../stores/prefs';
import { useRemindersStore } from '../../stores/reminders';
import WarButton from '../war/WarButton.vue';

const props = defineProps<{
  open: boolean;
}>();

const emit = defineEmits<{ (e: 'update:open', v: boolean): void }>();

const prefs = usePrefsStore();
const reminders = useRemindersStore();

const content = ref('');
const minutes = ref(10);
const contentEl = ref<HTMLInputElement | null>(null);

watch(
  () => props.open,
  (v) => {
    if (v) {
      content.value = '';
      minutes.value = 10;
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

async function confirm(): Promise<void> {
  const m = minutesValid();
  if (!content.value.trim() || !m) return;
  await reminders.add(content.value, m);
  emit('update:open', false);
}

function close(): void {
  emit('update:open', false);
}
</script>

<template>
  <Teleport to="body">
    <div v-if="props.open" class="rd-mask" @mousedown.self="close">
      <div class="rd">
        <div class="rd__frame"></div>
        <div class="rd__inner">
          <div class="rd__head">
            <span class="rd__title" :style="{ fontSize: prefs.fs(13) + 'px' }">添加提醒</span>
            <span class="rd__close" title="关闭" @click="close">✕</span>
          </div>
          <div class="rd__body">
            <input
              ref="contentEl"
              v-model="content"
              class="rd__field"
              placeholder="提醒内容…"
              :style="{ fontSize: prefs.fs(13) + 'px' }"
            />
            <div class="rd__time-row">
              <input
                v-model.number="minutes"
                type="number"
                min="1"
                class="rd__field rd__field--minutes"
                :style="{ fontSize: prefs.fs(13) + 'px' }"
              />
              <span class="rd__time-label" :style="{ fontSize: prefs.fs(12) + 'px' }">分钟后提醒我</span>
            </div>
          </div>
          <div class="rd__footer">
            <span class="rd__hint" :style="{ fontSize: prefs.fs(10) + 'px' }">Ctrl+Enter 确定 · Esc 取消</span>
            <WarButton
              :width="120"
              :art-aspect="5"
              skin="dialog"
              text="确定"
              :enabled="!!content.trim() && minutesValid() > 0"
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
.rd-mask {
  position: fixed;
  inset: 0;
  z-index: 115;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.rd {
  position: relative;
  width: min(480px, 90vw);
  height: min(300px, 80vh);
}

/* frame_popup.png nine-slice (same frame as the other dialogs) */
.rd__frame {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 88px 100px 90px 100px;
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.rd__inner {
  position: absolute;
  inset: 60px 64px 56px 62px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  min-height: 0;
  font-family: SimSun, serif;
}

.rd__head {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.rd__title {
  color: var(--war-gold);
  font-weight: bold;
}

.rd__close {
  color: var(--war-text-dim);
  padding: 2px 6px;
  user-select: none;
}

.rd__close:hover {
  color: var(--war-gold-bright);
}

.rd__body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.rd__field {
  background: #10141f;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: var(--war-text);
  font-family: SimSun, serif;
  padding: 6px 10px;
  outline: none;
}

.rd__field:focus {
  border-color: var(--war-gold-input);
}

.rd__field::placeholder {
  color: var(--war-text-faint);
}

.rd__time-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.rd__field--minutes {
  width: 80px;
}

.rd__time-label {
  color: var(--war-text-muted);
}

.rd__footer {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
}

.rd__hint {
  color: var(--war-text-faint);
  margin-right: auto;
}
</style>

<script setup lang="ts">
// Expanded composer (features/chat.md §3): the Composer's ⛶ button pops the
// draft into a roomy dialog for long-message editing. Same 64K cap; Ctrl+Enter
// confirms, Esc cancels. The vertical scrollbar is the WC3 WarScrollBar (the
// native bar is hidden, like every other scrollable in the app).
import { onBeforeUnmount, ref, watch } from 'vue';
import { cmd } from '../../lib/tauri';
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';
import WarButton from '../war/WarButton.vue';
import WarScrollBar from '../war/WarScrollBar.vue';

const MAX_LEN = 64000;

const props = defineProps<{
  open: boolean;
  initialText: string;
}>();

const emit = defineEmits<{
  (e: 'update:open', v: boolean): void;
  (e: 'confirm', v: string): void;
}>();

const prefs = usePrefsStore();
const chat = useChatStore();

const draft = ref('');
const areaEl = ref<HTMLTextAreaElement | null>(null);
const composing = ref(false);

watch(
  () => props.open,
  (v) => {
    if (v) {
      draft.value = props.initialText;
      window.addEventListener('keydown', onKey, true);
      requestAnimationFrame(() => areaEl.value?.focus());
    } else {
      window.removeEventListener('keydown', onKey, true);
    }
  },
);
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true));

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.stopPropagation();
    cancel();
  } else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    confirm();
  }
}

function onInput(): void {
  if (composing.value) return; // never touch the document mid-IME
  if (draft.value.length > MAX_LEN) draft.value = draft.value.slice(0, MAX_LEN);
}

// Image paste → media cache + attachment bar, same as the main Composer; the
// draft also gets a ![name](<path>) embed so the sent bubble renders inline.
function fileNameOf(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

async function onPaste(e: ClipboardEvent): Promise<void> {
  const items = e.clipboardData?.items;
  if (!items) return;
  for (const item of items) {
    if (item.kind === 'file' && item.type.startsWith('image/')) {
      e.preventDefault();
      const file = item.getAsFile();
      if (!file) return;
      const buf = await file.arrayBuffer();
      try {
        const path = await cmd<string>('save_clipboard_image', {
          sessionId: chat.sessionId,
          bytes: Array.from(new Uint8Array(buf)),
        });
        if (path) {
          chat.addAttachments([path]);
          const snippet = `![${fileNameOf(path)}](<${path.replace(/\\/g, '/')}>)`; // '\' breaks markdown destinations
          const el = areaEl.value;
          const s = el ? el.selectionStart : draft.value.length;
          const t = el ? el.selectionEnd : draft.value.length;
          draft.value = (draft.value.slice(0, s) + snippet + draft.value.slice(t)).slice(0, MAX_LEN);
          void Promise.resolve().then(() => {
            if (el) {
              el.selectionStart = el.selectionEnd = s + snippet.length;
              el.focus();
            }
          });
        }
      } catch (err) {
        console.warn('[composer-expand] save_clipboard_image failed', err);
      }
      return;
    }
  }
}

function confirm(): void {
  emit('confirm', draft.value);
  emit('update:open', false);
}

function cancel(): void {
  emit('update:open', false);
}
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="ced-mask" @mousedown.self="cancel">
      <div class="ced">
        <div class="ced__frame"></div>
        <div class="ced__inner">
          <div class="ced__head">
            <span class="ced__title" :style="{ fontSize: prefs.fs(13) + 'px' }">编辑消息</span>
            <span class="ced__counter" :class="{ full: draft.length >= MAX_LEN }" :style="{ fontSize: prefs.fs(10) + 'px' }">
              {{ draft.length }} / {{ MAX_LEN }}
            </span>
          </div>
          <div class="ced__body">
            <textarea
              ref="areaEl"
              v-model="draft"
              class="ced__field"
              placeholder="输入消息…（@ 引用文件在发送时展开，Ctrl+V 可粘贴图片）"
              :style="{ fontSize: prefs.fs(14) + 'px' }"
              @input="onInput"
              @paste="onPaste"
              @compositionstart="composing = true"
              @compositionend="composing = false; onInput()"
            ></textarea>
            <WarScrollBar :target="areaEl" />
          </div>
          <div class="ced__footer">
            <span class="ced__hint" :style="{ fontSize: prefs.fs(10) + 'px' }">Ctrl+Enter 完成 · Esc 取消</span>
            <WarButton :width="120" :art-aspect="5" skin="dialog" text="完成" @activated="confirm" />
            <WarButton :width="120" :art-aspect="5" skin="dialog" text="取消" @activated="cancel" />
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.ced-mask {
  position: fixed;
  inset: 0;
  z-index: 110;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.ced {
  position: relative;
  width: min(900px, 92vw);
  height: min(640px, 80vh);
}

/* frame_popup.png nine-slice (same frame as GitCommitDialog) */
.ced__frame {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 88px 100px 90px 100px;
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.ced__inner {
  position: absolute;
  inset: 60px 64px 56px 62px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.ced__head {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.ced__title {
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-weight: bold;
}

.ced__counter {
  color: var(--war-text-muted);
  user-select: none;
}

.ced__counter.full {
  color: #ff8a70;
}

.ced__body {
  flex: 1;
  min-height: 0;
  display: flex;
}

.ced__field {
  flex: 1;
  min-width: 0;
  resize: none;
  background: #10141f;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: var(--war-text);
  font-family: SimSun, serif;
  padding: 8px 10px;
  outline: none;
  scrollbar-width: none; /* native bar hidden — the WC3 WarScrollBar replaces it */
}

.ced__field:focus {
  border-color: var(--war-gold-input);
}

.ced__field::placeholder {
  color: var(--war-text-faint);
}

.ced__footer {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
}

.ced__hint {
  color: var(--war-text-faint);
  font-family: SimSun, serif;
  margin-right: auto;
}
</style>

<script setup lang="ts">
// Due-todo overlay (global component, mounted in ChatPage): listens for
//   todos://due         → popup rows (session/global scope) due — a small
//                         notice + 知道了. Row stays pending; ticking it off
//                         in the list is the "completion" action.
//   todos://projectDue  → project rows due — the backend already created a
//                         new session in the project (named after the todo).
//                         Three-way choice:
//                           跳转并处理 → switch to the new session + send the
//                                        todo text (kind=reminder)
//                           后台处理   → keep the current view, send anyway
//                           我知道了   → keep the session, send nothing
//                         A 会话命名 input lets the user rename before any
//                         choice (backend default = the todo title).
import { onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { cmd, isTauri } from '../../lib/tauri';
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';
import { useTodosStore, type TodoRow } from '../../stores/todos';
import WarButton from '../war/WarButton.vue';

const chat = useChatStore();
const prefs = usePrefsStore();
const todos = useTodosStore();

const unlisteners: UnlistenFn[] = [];

// ---- popup notice (session/global due) ----
const popup = ref<{ row: TodoRow } | null>(null);

// ---- project due three-way ----
const proj = ref<{ row: TodoRow; sessionId: string } | null>(null);
const sessionName = ref('');
const acting = ref(false);
const doneText = ref('');

onMounted(async () => {
  if (!isTauri) return;
  unlisteners.push(
    await listen<{ row: TodoRow }>('todos://due', (e) => {
      // Only surface rows the CURRENT session should see; global rows always.
      const r = e.payload.row;
      if (r.scope === 'session' && r.sessionId !== chat.sessionId) return;
      popup.value = e.payload;
    }),
  );
  unlisteners.push(
    await listen<{ row: TodoRow; sessionId: string }>('todos://projectDue', (e) => {
      proj.value = e.payload;
      sessionName.value = e.payload.row.title;
      doneText.value = '';
    }),
  );
});
onBeforeUnmount(() => {
  unlisteners.forEach((u) => u());
});

function closePopup(): void {
  popup.value = null;
}

async function renameIfChanged(): Promise<void> {
  const name = sessionName.value.trim();
  const row = proj.value?.row;
  if (!row || !name || name === row.title) return;
  try {
    await cmd('rename_session', { sessionId: proj.value!.sessionId, title: name });
  } catch (e) {
    console.warn('[todos] rename_session failed', e);
  }
}

async function choose(action: 'jump' | 'background' | 'ack'): Promise<void> {
  if (!proj.value || acting.value) return;
  acting.value = true;
  const { row, sessionId } = proj.value;
  await renameIfChanged();
  if (action !== 'ack') {
    const text = row.title.trim();
    if (text) {
      try {
        await cmd('send_reminder', { sessionId, text });
      } catch (e) {
        console.warn('[todos] send_reminder failed', e);
      }
    }
  }
  if (action === 'jump') {
    const ok = await chat.openSession(sessionId);
    if (!ok) doneText.value = '无法跳转到新会话';
  } else {
    doneText.value = action === 'ack' ? '已保留会话，未发送' : '已在后台会话发送';
  }
  acting.value = false;
  proj.value = null;
}

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.stopPropagation();
    if (popup.value) closePopup();
    else if (proj.value && !acting.value) proj.value = null;
  }
}
watch(
  [popup, proj],
  () => {
    if (popup.value || proj.value) window.addEventListener('keydown', onKey, true);
    else window.removeEventListener('keydown', onKey, true);
  },
  { deep: true },
);
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true));
</script>

<template>
  <Teleport to="body">
    <!-- popup due notice -->
    <div v-if="popup" class="due-mask" @mousedown.self="closePopup">
      <div class="due due--popup">
        <div class="due__frame"></div>
        <div class="due__inner">
          <div class="due__title" :style="{ fontSize: prefs.fs(14) + 'px' }">待办到期</div>
          <div class="due__text" :style="{ fontSize: prefs.fs(13) + 'px' }">
            {{ popup.row.title }}
          </div>
          <div class="due__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
            {{ popup.row.scope === 'session' ? '（本会话待办）' : '（全局待办）' }} · 完成后在待办里勾掉即可
          </div>
          <div class="due__btns">
            <WarButton :width="140" skin="dialog" text="知道了" @activated="closePopup" />
          </div>
        </div>
      </div>
    </div>

    <!-- project due three-way -->
    <div v-if="proj" class="due-mask" @mousedown.self="!acting && (proj = null)">
      <div class="due">
        <div class="due__frame"></div>
        <div class="due__inner">
          <div class="due__title" :style="{ fontSize: prefs.fs(14) + 'px' }">项目待办到期</div>
          <div class="due__text" :style="{ fontSize: prefs.fs(13) + 'px' }">
            {{ proj.row.title }}
          </div>
          <div class="due__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
            已在项目内新建会话，选择如何处理：
          </div>
          <input
            v-model="sessionName"
            class="due__name"
            :style="{ fontSize: prefs.fs(12) + 'px' }"
            placeholder="会话命名"
          />
          <div v-if="doneText" class="due__done" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ doneText }}</div>
          <div class="due__btns due__btns--col">
            <WarButton
              :width="220"
              :art-aspect="6"
              skin="dialog"
              text="跳转并处理（切到新会话并开始）"
              :enabled="!acting"
              @activated="choose('jump')"
            />
            <WarButton
              :width="220"
              :art-aspect="6"
              skin="dialog"
              text="后台处理（不切换，自动开始）"
              :enabled="!acting"
              @activated="choose('background')"
            />
            <WarButton
              :width="220"
              :art-aspect="6"
              skin="dialog"
              text="我知道了（仅保留会话）"
              :enabled="!acting"
              @activated="choose('ack')"
            />
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.due-mask {
  position: fixed;
  inset: 0;
  z-index: 110;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.due {
  position: relative;
  width: min(460px, 90vw);
  height: min(320px, 80vh);
}

.due--popup {
  width: min(400px, 90vw);
  height: min(240px, 70vh);
}

/* frame_popup.png nine-slice (same frame as the other dialogs) */
.due__frame {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 88px 100px 90px 100px;
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.due__inner {
  position: absolute;
  inset: 60px 64px 56px 62px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  min-height: 0;
  font-family: SimSun, serif;
}

.due__title {
  color: var(--war-gold);
  font-weight: bold;
  text-align: center;
}

.due__text {
  color: var(--war-text);
  text-align: center;
  white-space: pre-wrap;
  overflow-wrap: break-word;
  max-height: 40%;
  overflow: hidden;
}

.due__hint {
  color: var(--war-text-muted);
  text-align: center;
}

.due__name {
  width: 100%;
  background: #10141f;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: var(--war-text);
  font-family: SimSun, serif;
  padding: 5px 10px;
  outline: none;
}

.due__name:focus {
  border-color: var(--war-gold-input);
}

.due__done {
  color: #80f0a0;
}

.due__btns {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}

.due__btns--col {
  flex: 1;
  flex-direction: column;
  gap: 8px;
}
</style>

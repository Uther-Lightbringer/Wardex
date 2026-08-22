<script setup lang="ts">
// UI-plugin panel host (插件化改造 P2): renders one plugin's panel.html in a
// sandboxed iframe (asset protocol). The iframe talks to WarDex ONLY through
// a thin postMessage bridge — it cannot reach the Tauri IPC surface:
//
//   plugin → host : { source: 'wardex-plugin', type: 'ready' | 'notify'
//                     | 'sendPrompt', text?: string }
//   host → plugin : { source: 'wardex-host', type: 'info',
//                     payload: { sessionId, projectDir } }  (on ready)
//
// sendPrompt forwards text into the ACTIVE session's composer pipeline
// (chat.send). Anything else is ignored — the whitelist IS the security
// boundary; extend deliberately.
import { onBeforeUnmount, onMounted, ref } from 'vue';
import { fileSrc } from '../../lib/tauri';
import { useChatStore } from '../../stores/chat';

const props = defineProps<{
  /** Absolute path of panel.html (from plugins store uiPanels). */
  src: string;
  title: string;
}>();

const chat = useChatStore();
const frame = ref<HTMLIFrameElement | null>(null);
const notice = ref('');
let noticeTimer: ReturnType<typeof setTimeout> | null = null;

function showNotice(text: string): void {
  notice.value = text;
  if (noticeTimer) clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    notice.value = '';
    noticeTimer = null;
  }, 4000);
}

function postInfo(): void {
  const win = frame.value?.contentWindow;
  if (!win) return;
  win.postMessage(
    {
      source: 'wardex-host',
      type: 'info',
      payload: { sessionId: chat.sessionId ?? '', projectDir: chat.projectDir ?? '' },
    },
    '*',
  );
}

async function onMessage(e: MessageEvent): Promise<void> {
  if (e.source !== frame.value?.contentWindow) return;
  const data = e.data as { source?: string; type?: string; text?: string } | null;
  if (!data || data.source !== 'wardex-plugin') return;
  switch (data.type) {
    case 'ready':
      postInfo();
      break;
    case 'notify':
      showNotice(String(data.text ?? '').slice(0, 200));
      break;
    case 'sendPrompt': {
      const text = String(data.text ?? '').trim();
      if (!text) return;
      await chat.send(text, []);
      break;
    }
    default:
      break;
  }
}

onMounted(() => window.addEventListener('message', onMessage));
onBeforeUnmount(() => window.removeEventListener('message', onMessage));

const url = (): string => fileSrc(props.src);
</script>

<template>
  <div class="plugin-panel">
    <transition name="plugin-notice">
      <div v-if="notice" class="plugin-panel__notice">{{ notice }}</div>
    </transition>
    <!-- sandbox: scripts + same-origin allow-same-origin are needed for
         asset-protocol pages to run and postMessage; storage/forms denied. -->
    <iframe
      ref="frame"
      class="plugin-panel__frame"
      :src="url()"
      :title="title"
      sandbox="allow-scripts allow-same-origin"
      @load="postInfo()"
    ></iframe>
  </div>
</template>

<style scoped>
.plugin-panel {
  position: relative;
  height: 100%;
  display: flex;
  flex-direction: column;
}

.plugin-panel__frame {
  flex: 1;
  width: 100%;
  border: 0;
  background: transparent;
  color-scheme: inherit;
}

.plugin-panel__notice {
  position: absolute;
  left: 8px;
  right: 8px;
  bottom: 8px;
  z-index: 3;
  padding: 6px 10px;
  border-radius: 4px;
  background: rgba(20, 14, 6, 0.88);
  border: 1px solid var(--war-outline-brown, #5a3c1c);
  color: var(--war-gold, #e8c56a);
  font-size: 12px;
  pointer-events: none;
}

.plugin-notice-enter-active,
.plugin-notice-leave-active {
  transition: opacity 200ms ease;
}
.plugin-notice-enter-from,
.plugin-notice-leave-to {
  opacity: 0;
}
</style>

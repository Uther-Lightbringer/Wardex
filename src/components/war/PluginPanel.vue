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
import { onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { cmd } from '../../lib/tauri';
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

// Panel content: fetched Rust-side (whitelisted against enabled plugins) and
// rendered via srcdoc — asset-protocol URLs break on non-ASCII Windows paths
// (e.g. wardex-plugins\时钟\panel.html), and srcdoc also lets us drop
// allow-same-origin from the sandbox.
const html = ref('');
watch(
  () => props.src,
  async (src) => {
    if (!src) {
      html.value = '';
      return;
    }
    try {
      html.value = await cmd<string>('plugins_read_panel', { path: src }, '');
    } catch (e) {
      html.value = `<body style="font:13px sans-serif;padding:12px">面板加载失败：${String(e)}</body>`;
    }
  },
  { immediate: true },
);
</script>

<template>
  <div class="plugin-panel">
    <transition name="plugin-notice">
      <div v-if="notice" class="plugin-panel__notice">{{ notice }}</div>
    </transition>
    <!-- sandbox WITHOUT allow-same-origin: the plugin runs with a null
         origin and zero storage/IPC access; postMessage still works. -->
    <iframe
      ref="frame"
      class="plugin-panel__frame"
      :srcdoc="html"
      :title="title"
      sandbox="allow-scripts"
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

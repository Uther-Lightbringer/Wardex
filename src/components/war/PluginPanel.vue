<script setup lang="ts">
// UI-plugin panel host (插件化改造 P2): renders one plugin's panel.html in an
// iframe. UNSANDBOXED by design decision — panels share the app origin and
// may use window.__TAURI__ IPC directly. The postMessage bridge remains as a
// convenience layer (scoped storage, sendPrompt, log capture).
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
import { usePluginsStore } from '../../stores/plugins';

const props = defineProps<{
  /** Absolute path of panel.html (from plugins store uiPanels). */
  src: string;
  title: string;
  /** Plugin id — used to route captured runtime logs to .logs/<id>.log. */
  pluginId?: string;
}>();

const chat = useChatStore();
const plugins = usePluginsStore();
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
  const data = e.data as {
    source?: string;
    type?: string;
    text?: string;
    level?: string;
    op?: string;
  } | null;
  if (!data || data.source !== 'wardex-plugin') return;
  switch (data.type) {
    case 'ready':
      postInfo();
      break;
    case 'notify':
      showNotice(String(data.text ?? '').slice(0, 200));
      break;
    case 'log':
      captureLog(data.level ?? 'log', String(data.text ?? ''));
      break;
    case 'sendPrompt': {
      const text = String(data.text ?? '').trim();
      if (!text) return;
      await chat.send(text, []);
      break;
    }
    case 'storage': {
      void handleStorage(data as StorageMsg);
      break;
    }
    case 'window': {
      // surface:'both' panels may promote themselves to a dialog (op open)
      // or ask the dialog container to close (op close). Whitelisted op only.
      const op = data.op === 'close' ? 'close' : 'open';
      window.dispatchEvent(
        new CustomEvent('wardex-plugin-window', { detail: { id: props.pluginId, title: props.title, src: props.src, op } }),
      );
      break;
    }
    default:
      break;
  }
}

onMounted(() => window.addEventListener('message', onMessage));
onBeforeUnmount(() => window.removeEventListener('message', onMessage));

// --- runtime log capture ---------------------------------------------
// A tiny shim is prepended into the panel's own html (inside its sandbox):
// it patches console.error/warn and hooks window error/rejection events,
// forwarding everything over the SAME postMessage whitelist bridge as a
// 'log' message. The host keeps a small ring buffer for on-screen notices
// and persists lines Rust-side so the model's plugin_logs tool can read
// them and debug its own panels.
// --- scoped data storage (阶段③) ---------------------------------------
// The plugin declares ONE scope in plugin.json (data.scope); the host routes
// get/set/remove ops there. session → in-memory (dies with the session);
// project → <project>/.wardex/plugin-data/<id>.json; global → data root.
// The plugin never touches files itself.
interface StorageMsg {
  reqId?: number | string;
  op?: string;
  key?: string;
  value?: unknown;
}

/** Session-scope docs: `${sessionId}:${pluginId}` → object. */
const sessionStore = new Map<string, Record<string, unknown>>();

async function loadScopeDoc(scope: string): Promise<Record<string, unknown>> {
  const pid = props.pluginId ?? '';
  if (!pid) return {};
  if (scope === 'session') {
    return sessionStore.get(`${chat.sessionId}:${pid}`) ?? {};
  }
  return cmd<Record<string, unknown>>(
    'plugin_data_get',
    { id: pid, scope, projectDir: chat.projectDir ?? '' },
    {},
  );
}

async function saveScopeDoc(scope: string, doc: Record<string, unknown>): Promise<void> {
  const pid = props.pluginId ?? '';
  if (!pid) return;
  if (scope === 'session') {
    sessionStore.set(`${chat.sessionId}:${pid}`, doc);
    return;
  }
  await cmd('plugin_data_set', { id: pid, scope, projectDir: chat.projectDir ?? '', doc });
}

async function handleStorage(m: StorageMsg): Promise<void> {
  const reply = (ok: boolean, value?: unknown, error?: string) => {
    frame.value?.contentWindow?.postMessage(
      { source: 'wardex-host', type: 'storage', reqId: m.reqId, ok, value, error },
      '*',
    );
  };
  try {
    // Scope comes from the REGISTRY declaration, not the message — the
    // panel cannot choose a different scope at runtime.
    let scope = plugins.list.find((p) => p.id === props.pluginId)?.dataScope ?? 'project';
    if (!['session', 'project', 'global'].includes(scope)) scope = 'project';
    const doc = await loadScopeDoc(scope);
    switch (m.op) {
      case 'get':
        reply(true, m.key === undefined ? doc : doc[String(m.key)]);
        break;
      case 'set': {
        if (typeof m.key !== 'string' || !m.key) return reply(false, undefined, 'key 必须是非空字符串');
        doc[m.key] = m.value ?? null;
        await saveScopeDoc(scope, doc);
        reply(true);
        break;
      }
      case 'remove': {
        if (typeof m.key === 'string' && m.key in doc) delete doc[m.key];
        await saveScopeDoc(scope, doc);
        reply(true);
        break;
      }
      default:
        reply(false, undefined, `未知操作: ${String(m.op)}`);
    }
  } catch (e) {
    reply(false, undefined, String(e));
  }
}

const LOG_RING_MAX = 40;
const logRing = ref<string[]>([]);
let logFlushTimer: ReturnType<typeof setTimeout> | null = null;

function injectShim(html: string): string {
  const shim =
    '<script>(function(){' +
    "var send=function(l,t){try{parent.postMessage({source:'wardex-plugin',type:'log',level:l,text:String(t).slice(0,1500)},'*')}catch(_){}};" +
    "var fmt=function(a){return Array.prototype.map.call(a,function(x){if(typeof x==='string')return x;if(x&&x.stack)return x.stack;if(x&&x.message)return x.message;try{return JSON.stringify(x)}catch(_){return String(x)}}).join(' ')};" +
    "var oe=console.error,ow=console.warn;console.error=function(){send('error',fmt(arguments));oe.apply(console,arguments)};" +
    "console.warn=function(){send('warn',fmt(arguments));ow.apply(console,arguments)};" +
    "window.addEventListener('error',function(e){send('error',(e.message||'script error')+(e.lineno?(' @line '+e.lineno):''))});" +
    "window.addEventListener('unhandledrejection',function(e){var r=e.reason;send('error','unhandled rejection: '+((r&&(r.stack||r.message))||String(r)))});" +
    '})();<' + '/script>';
  const head = html.match(/<head[^>]*>/i);
  return head ? html.replace(head[0], head[0] + shim) : shim + html;
}

function captureLog(level: string, text: string): void {
  const stamp = new Date().toLocaleTimeString('en-GB');
  logRing.value.push(`[${stamp}] [${level}] ${text}`.slice(0, 1600));
  if (logRing.value.length > LOG_RING_MAX) logRing.value.shift();
  if (level === 'error') showNotice(`⚠ 面板报错：${text.slice(0, 120)}`);
  // Batch + persist so the model can read the trail via plugin_logs.
  if (!props.pluginId) return;
  pendingLogLines.push(`[${stamp}] [${level}] ${text}`.slice(0, 1600));
  if (!logFlushTimer) {
    logFlushTimer = setTimeout(flushLogs, 800);
  }
}

const pendingLogLines: string[] = [];

async function flushLogs(): Promise<void> {
  logFlushTimer = null;
  const id = props.pluginId;
  const lines = pendingLogLines.splice(0);
  if (!id || !lines.length) return;
  try {
    await cmd('plugin_log_append', { id, lines });
  } catch {
    /* logging must never break the panel host */
  }
}

onBeforeUnmount(() => {
  if (logFlushTimer) clearTimeout(logFlushTimer);
  void flushLogs();
});

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
      html.value = injectShim(
        await cmd<string>('plugins_read_panel', { path: src }, ''),
      );
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
    <!-- UNSANDBOXED (user decision, single-user personal build): srcdoc
         inherits the app's origin, so panels get full same-origin access —
         window.__TAURI__ IPC (withGlobalTauri), parent DOM, storage.
         The postMessage bridge stays for convenience (storage scopes,
         sendPrompt, log capture). -->
    <iframe
      ref="frame"
      class="plugin-panel__frame"
      :srcdoc="html"
      :title="title"
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

<script setup lang="ts">
// 迷你会话窗（原型 #chatwin）：不切 active 的后台会话小窗。
//   - 历史：session_messages 整拉。ai 气泡 markdown 渲染（复用
//     lib/markdown.ts renderMarkdown，content-addressed 缓存照 ChatBubble）；
//     user 气泡纯文本优先、嵌图片 markdown 时走 renderUserMarkdown；附件图
//     内嵌（fileSrc，点击系统打开）、非图片附件文件名芯片。工具/thinking 段
//     保持不渲染（visibleText 只取 text 段）。
//   - 发送：本地插用户气泡 → ensure_runtime → send_prompt；acp://turn 完成
//     后整拉重渲染。
//   - 底栏：Agent 下拉（可切换，monitor.switchAgent → 后端 switch_agent）
//     + 模型（只读展示当前模型——ChatPage composer 本就无模型切换，
//     模型挂在 Agent 配置上）+ 权限模式下拉（set_session_perm_mode）+ 发送。
//   - 打开期间该会话有权限请求（isPermPending false→true，含打开时已为真）
//     自动弹审批弹窗；acp://permissionCleared 由 monitor store 关弹窗。
//   - 「在完整页面打开 →」走 chat.openSession + nav.goOverlay('chat')。
//   - Esc 由 MonitorPage 页级处理（monitor.closeChatWin）。
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { cmd, fileSrc, openPath } from '../../lib/tauri';
import { renderMarkdown, renderUserMarkdown, handleMdLinkClick } from '../../lib/markdown';
import { copyText } from '../../lib/clipboard';
import { visibleText, useSessionsStore, type StoredMessage } from '../../stores/sessions';
import { useMonitorStore } from '../../stores/monitor';
import { useChatStore, type SessionMeta } from '../../stores/chat';
import { useAgentsStore } from '../../stores/agents';
import { useNavStore } from '../../stores/nav';
import { useUiStore } from '../../stores/ui';
import { usePrefsStore } from '../../stores/prefs';

const props = defineProps<{ sessionId: string }>();
const emit = defineEmits<{ (e: 'close'): void; (e: 'toast', msg: string): void }>();

const sessions = useSessionsStore();
const monitor = useMonitorStore();
const chat = useChatStore();
const agents = useAgentsStore();
const nav = useNavStore();
const ui = useUiStore();
const prefs = usePrefsStore();

interface Bubble {
  role: 'user' | 'ai' | 'sys';
  text: string;
  /** 预渲染的 markdown HTML（'' = 走纯文本）；构建气泡时一次算好。 */
  html: string;
  attachments: string[];
}

// ---- markdown（照 ChatBubble：content-addressed 缓存 + 用户图判定） ----

const mdCache = new Map<string, string>();
function markdownOf(text: string): string {
  let html = mdCache.get(text);
  if (html === undefined) {
    html = renderMarkdown(text);
    if (mdCache.size > 500) mdCache.clear(); // bounded, content-addressed
    mdCache.set(text, html);
  }
  return html;
}

/** user 文本只有嵌了图片 markdown（Composer 粘贴图）才走渲染器（照 ChatBubble）。 */
const MD_IMG_RE = /!\[[^\]]*\]\(/;
function userHtml(text: string): string {
  return MD_IMG_RE.test(text) ? renderUserMarkdown(text) : '';
}

// ---- 附件（照 ChatBubble：图片内嵌 / 其余文件名芯片，点击系统打开） ----

const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'];
function isImagePath(p: string): boolean {
  const ext = p.split('.').pop()?.toLowerCase() ?? '';
  return IMAGE_EXTS.includes(ext);
}
function fileName(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}
/** 已嵌进正文 markdown 图片的附件不重复显示（照 ChatBubble visibleAtts）。 */
function visibleAtts(b: Bubble): string[] {
  return b.attachments.filter((p) => !b.text.includes(p.replace(/\\/g, '/')));
}

/** v-html 委托点击（照 ChatBubble.onBodyClick）：代码块复制按钮 + 链接
 * （http 系统浏览器 / 本地路径 OS 打开；webview 自身绝不导航）。 */
function onBodyClick(e: MouseEvent): void {
  const t = e.target as HTMLElement;
  if (!t.closest('.md-body')) return;
  const btn = t.closest<HTMLElement>('.codeblock__copy');
  if (btn) {
    const text = btn.parentElement?.querySelector('pre')?.innerText ?? '';
    if (!text.trim()) return;
    void copyText(text).then((ok) => {
      if (!ok) return;
      btn.textContent = '已复制';
      setTimeout(() => (btn.textContent = '复制'), 1200);
    });
    return;
  }
  handleMdLinkClick(e);
}

const meta = ref<SessionMeta | null>(null);
const bubbles = ref<Bubble[]>([]);
const input = ref('');
const sending = ref(false);
const bodyEl = ref<HTMLElement | null>(null);

const indexRow = computed(() => sessions.all.find((s) => s.id === props.sessionId));
const title = computed(() => meta.value?.title ?? indexRow.value?.title ?? '会话');
const agentLine = computed(() => {
  const ag = meta.value?.agentName ?? indexRow.value?.agentName ?? '';
  const dir = meta.value?.projectDir ?? indexRow.value?.projectDir ?? '';
  return [ag, dir].filter(Boolean).join(' · ');
});
const modelLabel = computed(() => meta.value?.model || '默认');
const permMode = computed(() => indexRow.value?.permMode ?? 'default');
const busy = computed(() => sessions.runtimeStates[props.sessionId]?.busy === true);

// ---- Agent 切换（照 MonitorPage 新会话面板的 usableAgents 口径：enabled） ----

const usableAgents = computed(() => agents.agents.filter((a) => a.enabled));
const currentAgentId = computed(() => meta.value?.agentId ?? '');
const switchingAgent = ref(false);

async function onAgentChange(e: Event): Promise<void> {
  const sel = e.target as HTMLSelectElement;
  const v = sel.value;
  if (!v || v === currentAgentId.value || switchingAgent.value) return;
  switchingAgent.value = true;
  try {
    const ok = await monitor.switchAgent(props.sessionId, v);
    if (!ok) {
      sel.value = currentAgentId.value; // 单向 :value 绑定，失败手动复位
      emit('toast', '切换 Agent 失败');
      return;
    }
    await loadMeta(); // 刷新头部 agent 名与模型只读显示
  } finally {
    switchingAgent.value = false;
  }
}

// ---- 权限请求自动弹审批 ----

const permPending = computed(() => monitor.isPermPending(props.sessionId));
watch(
  permPending,
  (v) => {
    if (v) void monitor.openPermDialog(props.sessionId);
  },
  { immediate: true }, // 打开时已 pending 也立即弹
);

const PERM_MODES: [string, string][] = [
  ['default', '默认'],
  ['plan', '计划'],
  ['auto', '自动'],
  ['yolo', '放任'],
];

function toBubbles(rows: StoredMessage[]): Bubble[] {
  const out: Bubble[] = [];
  for (const r of rows) {
    const t = visibleText(r).trim();
    const atts = r.attachments ?? [];
    if (!t && atts.length === 0) continue;
    const role = r.role === 'user' ? 'user' : r.role === 'assistant' ? 'ai' : 'sys';
    out.push({
      role,
      text: t,
      html: role === 'ai' ? markdownOf(t) : role === 'user' ? userHtml(t) : '',
      attachments: atts,
    });
  }
  return out;
}

async function scrollToEnd(): Promise<void> {
  await nextTick();
  if (bodyEl.value) bodyEl.value.scrollTop = bodyEl.value.scrollHeight;
}

async function reload(): Promise<void> {
  const rows = await cmd<StoredMessage[]>('session_messages', { sessionId: props.sessionId }, []);
  bubbles.value = toBubbles(rows);
  await scrollToEnd();
}

let unlistenTurn: UnlistenFn | null = null;

async function loadMeta(): Promise<void> {
  meta.value = await cmd<SessionMeta | null>('session_meta', { sessionId: props.sessionId }, null);
}

onMounted(async () => {
  void loadMeta();
  await reload();
  // turn 完成 → 整拉重渲染（MVP 不做流式增量）。
  unlistenTurn = await listen<{ sessionId: string }>('acp://turn', (e) => {
    if (e.payload.sessionId === props.sessionId) void reload();
  });
});
onBeforeUnmount(() => unlistenTurn?.());

async function send(): Promise<void> {
  const text = input.value.trim();
  if (!text || sending.value) return;
  sending.value = true;
  try {
    bubbles.value = [...bubbles.value, { role: 'user', text, html: userHtml(text), attachments: [] }];
    input.value = '';
    await scrollToEnd();
    const ok = await monitor.sendTo(props.sessionId, text);
    if (!ok) {
      bubbles.value = [...bubbles.value, { role: 'sys', text: '⚠ 发送失败，请重试', html: '', attachments: [] }];
      await scrollToEnd();
    }
  } finally {
    sending.value = false;
  }
}

function onModeChange(e: Event): void {
  const v = (e.target as HTMLSelectElement).value;
  void monitor.setPermMode(props.sessionId, v);
}

async function openFull(): Promise<void> {
  emit('close');
  const ok = await chat.openSession(props.sessionId);
  if (!ok) {
    ui.showBanner('无法打开会话');
    return;
  }
  await nav.goOverlay('chat');
}
</script>

<template>
  <div class="cw">
    <div class="cw__head">
      <span class="cw__title" :style="{ fontSize: prefs.fs(15) + 'px' }">🛡 {{ title }}</span>
      <span class="cw__agent" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ agentLine }}</span>
      <span class="cw__full" :style="{ fontSize: prefs.fs(12) + 'px' }" @click="openFull">在完整页面打开 →</span>
      <span class="cw__close" :style="{ fontSize: prefs.fs(14) + 'px' }" @click="emit('close')">✕</span>
    </div>
    <div ref="bodyEl" class="cw__body" @click="onBodyClick">
      <div
        v-for="(b, i) in bubbles"
        :key="i"
        class="cw__bubble"
        :class="b.role"
        :style="{ fontSize: prefs.fs(13) + 'px' }"
      >
        <div v-if="b.html" class="cw__md md-body" v-html="b.html"></div>
        <template v-else>{{ b.text }}</template>
        <!-- 附件：图片内嵌（点击系统打开），其余文件名芯片 -->
        <div v-if="visibleAtts(b).length" class="cw__atts">
          <template v-for="(p, j) in visibleAtts(b)" :key="j">
            <img
              v-if="isImagePath(p)"
              class="cw__atts-img"
              :src="fileSrc(p)"
              :title="p"
              draggable="false"
              @click.stop="openPath(p)"
            />
            <div v-else class="cw__atts-chip" :title="p" @click.stop="openPath(p)">
              <img src="/assets/wc3_extracted/ui/icon-file.png" draggable="false" />
              <span :style="{ fontSize: prefs.fs(11) + 'px' }">{{ fileName(p) }}</span>
            </div>
          </template>
        </div>
      </div>
      <div v-if="busy" class="cw__bubble sys" :style="{ fontSize: prefs.fs(11) + 'px' }">生成中…</div>
      <div v-if="bubbles.length === 0 && !busy" class="cw__bubble sys" :style="{ fontSize: prefs.fs(11) + 'px' }">
        （暂无消息，直接输入开始）
      </div>
    </div>
    <div class="cw__composer">
      <textarea
        v-model="input"
        class="cw__input"
        placeholder="输入消息，Enter 发送，Shift+Enter 换行…"
        :style="{ fontSize: prefs.fs(13) + 'px' }"
        @keydown.enter.exact.prevent="send"
      ></textarea>
      <div class="cw__bar">
        <span class="cw__sel-wrap" :style="{ fontSize: prefs.fs(11) + 'px' }">
          Agent
          <select
            class="cw__select cw__select--agent"
            :value="currentAgentId"
            :disabled="switchingAgent || usableAgents.length === 0"
            :style="{ fontSize: prefs.fs(12) + 'px' }"
            @change="onAgentChange"
          >
            <option v-if="currentAgentId && !usableAgents.some((a) => a.id === currentAgentId)" :value="currentAgentId">
              {{ meta?.agentName || '当前' }}
            </option>
            <option v-for="a in usableAgents" :key="a.id" :value="a.id">
              {{ a.name }}{{ a.id === agents.defaultAgentId ? ' ★' : '' }}
            </option>
          </select>
        </span>
        <span class="cw__sel-wrap" :style="{ fontSize: prefs.fs(11) + 'px' }">
          模型
          <select class="cw__select" :value="modelLabel" disabled :style="{ fontSize: prefs.fs(12) + 'px' }">
            <option>{{ modelLabel }}</option>
          </select>
        </span>
        <span class="cw__sel-wrap" :style="{ fontSize: prefs.fs(11) + 'px' }">
          权限
          <select
            class="cw__select"
            :value="permMode"
            :style="{ fontSize: prefs.fs(12) + 'px' }"
            @change="onModeChange"
          >
            <option v-for="[v, t] in PERM_MODES" :key="v" :value="v">{{ t }}</option>
          </select>
        </span>
        <span class="cw__send" :class="{ disabled: sending }" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="send">
          发送
        </span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.cw {
  position: absolute;
  z-index: 70;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  width: 540px;
  max-width: 90%;
  height: 74%;
  display: flex;
  flex-direction: column;
  background: #10141f;
  border: 2px solid #3a4a63;
  border-radius: 4px;
  box-shadow:
    0 0 60px #000,
    inset 0 0 40px #0007;
  font-family: SimSun, serif;
}

.cw__head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 14px;
  border-bottom: 1px solid #2a3344;
}

.cw__title {
  color: var(--war-gold);
  font-weight: bold;
  text-shadow: 1px 1px 0 #000;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 40%;
}

.cw__agent {
  color: var(--war-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}

.cw__full {
  margin-left: auto;
  flex: none;
  color: #7ec9e0;
  border: 1px solid #7ec9e044;
  padding: 2px 8px;
  border-radius: 2px;
}

.cw__full:hover {
  background: #7ec9e022;
}

.cw__close {
  flex: none;
  color: #b9c4dc;
  padding: 2px 8px;
  border: 1px solid #2a3344;
  border-radius: 2px;
}

.cw__close:hover {
  color: #ff9b8a;
  border-color: #b0552f;
}

.cw__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
  padding: 14px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  background: #0a0d14;
}

.cw__bubble {
  max-width: 80%;
  padding: 8px 12px;
  line-height: 1.6;
  border-radius: 6px;
  user-select: text;
  white-space: pre-wrap;
  overflow-wrap: break-word;
}

.cw__bubble.user {
  align-self: flex-end;
  background: #2b3a50;
  color: #e8d9a0;
  border: 1px solid #4a5b75;
}

.cw__bubble.ai {
  align-self: flex-start;
  background: #141a26;
  color: #b9c4dc;
  border: 1px solid #2a3344;
}

.cw__bubble.sys {
  align-self: center;
  background: none;
  border: none;
  color: var(--war-text-muted);
  font-size: 11px;
  padding: 0;
}

.cw__composer {
  flex: none;
  border-top: 1px solid #2a3344;
  padding: 10px;
}

.cw__input {
  width: 100%;
  height: 56px;
  background: #0a0d14;
  border: 1px solid #2a3344;
  border-radius: 3px;
  color: #e8d9a0;
  font-family: SimSun, serif;
  padding: 7px;
  resize: none;
  outline: none;
  user-select: text;
  box-sizing: border-box;
}

.cw__input:focus {
  border-color: #c9a22766;
}

.cw__bar {
  display: flex;
  gap: 8px;
  margin-top: 8px;
  align-items: center;
}

.cw__sel-wrap {
  display: flex;
  align-items: center;
  gap: 4px;
  color: var(--war-text-muted);
}

.cw__select {
  background: #10141f;
  color: #e8d9a0;
  border: 1px solid #4a5b75;
  border-radius: 2px;
  font-family: SimSun, serif;
  height: 26px;
  padding: 0 4px;
  outline: none;
  max-width: 160px;
}

.cw__select:disabled {
  opacity: 0.6;
}

.cw__select--agent {
  max-width: 110px;
}

.cw__send {
  margin-left: auto;
  padding: 4px 22px;
  color: #a8e6a0;
  border: 1px solid #7ec97a66;
  border-radius: 2px;
  background: #7ec97a12;
}

.cw__send:hover {
  background: #7ec97a2a;
}

.cw__send.disabled {
  opacity: 0.5;
  pointer-events: none;
}

/* ---- markdown 气泡体（v-html；内容规则在下面 unscoped 块） ---- */
.cw__md {
  white-space: normal; /* 气泡 pre-wrap 只服务纯文本路径 */
  user-select: text;
}

/* ---- 附件（照 ChatBubble bubble-atts） ---- */
.cw__atts {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 6px;
}

.cw__atts-img {
  max-width: 280px;
  max-height: 200px;
  object-fit: contain;
  border-radius: 2px;
  border: 1px solid #4a5b75;
}

.cw__atts-chip {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: 200px;
  padding: 4px 8px;
  background: #1a2334;
  border: 1px solid #2c4a7a;
  border-radius: 2px;
}

.cw__atts-chip:hover {
  border-color: #4a5b75;
}

.cw__atts-chip img {
  width: 15px;
  height: 15px;
  flex: none;
}

.cw__atts-chip span {
  color: #c0d0ec;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>

<style>
/* 小窗 markdown 内容（unscoped：v-html 渲染产物没有 scoped 属性）。规则
   照 ChatBubble 的全局 .md-body 块（war 金色调），命名空间收在 .cw 下避免
   与聊天页互相影响；窄窗适配：pre 横向滚动、图片限宽 280px。 */
.cw .md-body > :first-child {
  margin-top: 0;
}
.cw .md-body > :last-child {
  margin-bottom: 0;
}
.cw .md-body p {
  margin: 4px 0;
  line-height: 1.5;
}
.cw .md-body strong,
.cw .md-body b {
  color: var(--war-gold-bright);
}
.cw .md-body em,
.cw .md-body i {
  color: var(--war-gold);
}
.cw .md-body pre {
  background: #00000070;
  border: 1px solid var(--war-gold-input);
  border-radius: 2px;
  padding: 5px 7px;
  overflow-x: auto;
  font-family: Consolas, monospace;
  font-size: 0.92em;
}
.cw .md-body .codeblock {
  position: relative;
}
.cw .md-body .codeblock pre {
  margin: 4px 0;
}
.cw .md-body .codeblock__copy {
  position: absolute;
  top: 8px;
  right: 4px;
  z-index: 1;
  padding: 0 6px;
  background: #12151cbb;
  border: 1px solid var(--war-gold-dim);
  border-radius: 2px;
  color: #a0a8b8;
  font-family: SimSun, serif;
  font-size: 11px;
  opacity: 0;
  transition: opacity 0.15s;
}
.cw .md-body .codeblock:hover .codeblock__copy {
  opacity: 1;
}
.cw .md-body .codeblock__copy:hover {
  color: var(--war-gold-bright);
  border-color: var(--war-gold);
}
.cw .md-body code {
  font-family: Consolas, monospace;
  background: #00000050;
  color: var(--war-gold-bright);
  padding: 0 3px;
  border-radius: 2px;
  font-size: 0.92em;
}
.cw .md-body pre code {
  background: none;
  color: var(--war-text);
  padding: 0;
}
.cw .md-body ul,
.cw .md-body ol {
  margin: 4px 0;
  padding-left: 20px;
}
.cw .md-body li::marker {
  color: var(--war-gold-dim);
}
.cw .md-body h1,
.cw .md-body h2,
.cw .md-body h3,
.cw .md-body h4 {
  margin: 8px 0 4px;
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-weight: bold;
}
.cw .md-body h1 { font-size: 1.25em; }
.cw .md-body h2 { font-size: 1.15em; }
.cw .md-body h3 { font-size: 1.05em; }
.cw .md-body blockquote {
  margin: 4px 0;
  padding-left: 8px;
  border-left: 3px solid var(--war-gold-dim);
  color: var(--war-text-muted);
}
.cw .md-body a {
  color: var(--war-gold-bright);
  text-decoration: underline;
}
.cw .md-body a:hover {
  color: var(--war-gold);
}
.cw .md-body img {
  max-width: min(100%, 280px);
  max-height: 280px;
  object-fit: contain;
  border-radius: 2px;
}
.cw .md-body table {
  border-collapse: collapse;
  margin: 6px 0;
}
.cw .md-body th,
.cw .md-body td {
  border: 1px solid #2a3344;
  padding: 2px 8px;
}
.cw .md-body th {
  color: var(--war-gold);
}
.cw .md-body hr {
  border: none;
  border-top: 1px solid #2a3344;
  margin: 8px 0;
}
</style>

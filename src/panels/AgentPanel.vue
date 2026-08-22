<script setup lang="ts">
// Session info panel (features/chat.md §6.2): agentName · provider, model,
// message count + created/updated timestamps, per-session token usage from
// the backend usage.json aggregate (`session_usage`: resident-memory sum over
// that session's records — covers backfilled history, which the in-row
// message usages don't), work directory, session summary, and the sticky
// lastError line (startup errors like "no usable default agent" surface
// here).
import { computed, onMounted, ref, watch } from 'vue';
import { useChatStore } from '../stores/chat';
import { usePrefsStore } from '../stores/prefs';
import { useUiStore } from '../stores/ui';
import { cmd, isTauri } from '../lib/tauri';
import { formatTokens } from '../lib/format';

interface SessionUsage {
  turns: number;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  cachedReadTokens: number;
  cachedWriteTokens: number;
  thoughtTokens: number;
  contextTokens: number;
}

interface InfoRow {
  k: string;
  v: string;
  hint?: string;
}

const chat = useChatStore();
const prefs = usePrefsStore();
const ui = useUiStore();

const meta = computed(() => chat.meta);

function stamp(ms: number): string {
  if (!ms) return '';
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

// Option B entry: let a project-less session bind a project dir afterwards.
function bindProject(): void {
  ui.folderDialogPurpose = 'bind';
  ui.folderDialogOpen = true;
}

// 每会话开关：是否向 prompt 自动注入 codegraph 相关符号上下文（缺省=开）。
const useCodegraph = computed(() => meta.value?.useCodegraph ?? true);
function toggleCodegraph(): void {
  void chat.setUseCodegraph(!useCodegraph.value);
}

// @引用展开方式（全局，见 prefs.refExpandMode）：'model' 只发标记让模型
// 自读最新内容；'inject' 发送时把文件内容注入消息。切换按钮从 Composer
// 搬到了会话信息面板（仅搬 UI，仍是全局共享偏好）。
const refExpandLabel = computed(() =>
  prefs.refExpandMode === 'inject' ? '注入内容' : '模型自读',
);
const refExpandTitle = computed(() =>
  prefs.refExpandMode === 'inject'
    ? '引用：发送时注入文件内容。点击改为「模型自读」'
    : '引用：只发标记，模型自行读取最新内容。点击改为「注入内容」',
);
function toggleRefExpand(): void {
  prefs.refExpandMode = prefs.refExpandMode === 'inject' ? 'model' : 'inject';
}

// codegraph MCP 注入状态展示：装没装 / 会话注入没注入 / 索引建没建。
const cgInstalled = ref(false);
const cgIndex = ref(false);
async function loadCgInstalled(): Promise<void> {
  const dir = meta.value?.projectDir || meta.value?.workDir || '';
  if (!dir || !isTauri) {
    cgInstalled.value = false;
    return;
  }
  try {
    const s = await cmd<{ installed: boolean; indexExists: boolean }>('codegraph_status', {
      projectDir: dir,
    });
    cgInstalled.value = s.installed;
    cgIndex.value = s.indexExists;
  } catch {
    cgInstalled.value = false;
  }
}
watch(() => chat.meta?.projectDir, () => void loadCgInstalled());
onMounted(() => void loadCgInstalled());

const cgMCPText = computed(() => {
  if (!meta.value?.projectDir && !meta.value?.workDir) return '';
  if (!cgInstalled.value) return 'codegraph 未安装';
  if (!useCodegraph.value) return '未启用 codegraph';
  return cgIndex.value
    ? 'codegraph MCP 已注入本会话'
    : 'codegraph MCP 已注入（索引未建，Agent 需先 build）';
});

const agentLine = computed(() => {
  if (!meta.value) return '';
  return `${meta.value.agentName || 'Agent'} · ${meta.value.provider}`;
});

// 基础信息组：模型 / 消息数 / 创建 / 更新（空值行自动省略）。
const infoRows = computed<InfoRow[]>(() => {
  const m = meta.value;
  if (!m) return [];
  const rows: InfoRow[] = [];
  const live = chat.configOptions.find((o) => o.id === 'model');
  const liveVal = live?.currentValue ?? '';
  const liveName = live?.options.find((o) => o.value === liveVal)?.name;
  const model = liveName || liveVal || m.model;
  if (model) rows.push({ k: '模型', v: model });
  rows.push({ k: '消息', v: `${m.messageCount} 条` });
  rows.push({ k: '创建', v: stamp(m.createdAt) });
  rows.push({ k: '更新', v: stamp(m.updatedAt) });
  return rows;
});

// ---- usage: one `session_usage` IPC per session switch / structural rows
// change (turn end). Backend aggregates its resident usage.json records —
// including backfilled history — so old sessions show cached/thought too.
const usage = ref<SessionUsage | null>(null);
let usageSeq = 0;

async function loadUsage(): Promise<void> {
  const sid = chat.sessionId;
  if (!sid || !isTauri) return;
  const seq = ++usageSeq;
  try {
    const u = await cmd<SessionUsage | null>('session_usage', { sessionId: sid }, null);
    if (seq === usageSeq) usage.value = u;
  } catch {
    /* keep the previous value */
  }
}

watch(
  () => chat.sessionId,
  () => {
    usage.value = null;
    void loadUsage();
  },
);
// Structural rows replacement = a turn finished / messages reloaded → the
// backend record for that turn is already appended. (Streaming chunks mutate
// rows in place, so no extra IPC during generation.)
watch(
  () => chat.rows,
  () => {
    if (chat.sessionId) void loadUsage();
  },
);
onMounted(() => void loadUsage());

// 用量统计组：tokens / 回合 / 缓存读写 / 思考 / 上下文（零值行省略）。
// 上下文为估算值：最新一轮 input，kimi 每次请求带全量上下文，故最末轮
// input ≈ 当前上下文大小（含缓存前缀）。
const usageRows = computed<InfoRow[]>(() => {
  const u = usage.value;
  if (!u) return [];
  const rows: InfoRow[] = [];
  if (u.inputTokens > 0 || u.outputTokens > 0)
    rows.push({
      k: 'tokens',
      v: `输入 ${formatTokens(u.inputTokens)} · 输出 ${formatTokens(u.outputTokens)}`,
    });
  if (u.turns > 0) rows.push({ k: '回合', v: `${u.turns}` });
  if (u.cachedReadTokens > 0)
    rows.push({ k: '缓存读', v: `↑${formatTokens(u.cachedReadTokens)}` });
  if (u.cachedWriteTokens > 0)
    rows.push({ k: '缓存写', v: `↑${formatTokens(u.cachedWriteTokens)}` });
  if (u.thoughtTokens > 0)
    rows.push({ k: '思考', v: `↑${formatTokens(u.thoughtTokens)}` });
  if (u.contextTokens > 0)
    rows.push({
      k: '上下文',
      v: `≈${formatTokens(u.contextTokens)}`,
      hint: '估算值：最新一轮输入量（含缓存前缀）',
    });
  return rows;
});
</script>

<template>
  <div class="ainfo">
    <template v-if="meta">
      <div class="ainfo__agent" :style="{ fontSize: prefs.fs(13) + 'px' }">{{ agentLine }}</div>

      <div class="ainfo__sep"></div>
      <div class="ainfo__grid" :style="{ fontSize: prefs.fs(11) + 'px' }">
        <template v-for="r in infoRows" :key="r.k">
          <div class="ainfo__k">{{ r.k }}</div>
          <div class="ainfo__v" :title="r.hint">{{ r.v }}</div>
        </template>
      </div>

      <template v-if="usageRows.length">
        <div class="ainfo__sep"></div>
        <div class="ainfo__grid" :style="{ fontSize: prefs.fs(11) + 'px' }">
          <template v-for="r in usageRows" :key="r.k">
            <div class="ainfo__k">{{ r.k }}</div>
            <div class="ainfo__v" :title="r.hint">{{ r.v }}</div>
          </template>
        </div>
      </template>

      <div class="ainfo__sep"></div>
      <div class="ainfo__label" :style="{ fontSize: prefs.fs(11) + 'px' }">工作目录</div>
      <div class="ainfo__path" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ meta.workDir || meta.projectDir }}</div>
      <div
        v-if="!meta.projectDir"
        class="ainfo__bind"
        :style="{ fontSize: prefs.fs(11) + 'px' }"
        @click="bindProject"
      >
        关联项目目录…
      </div>

      <div class="ainfo__sep"></div>
      <div
        class="ainfo__refmode"
        :class="{ active: prefs.refExpandMode === 'model' }"
        :title="refExpandTitle"
        :style="{ fontSize: prefs.fs(11) + 'px' }"
        @click="toggleRefExpand"
      >
        <span class="ainfo__refmode-dot">@</span>{{ refExpandLabel }}
      </div>

      <div class="ainfo__sep"></div>
      <label class="ainfo__toggle" :style="{ fontSize: prefs.fs(11) + 'px' }">
        <input type="checkbox" :checked="useCodegraph" @change="toggleCodegraph" />
        使用 codegraph 索引
      </label>
      <div
        v-if="cgMCPText"
        class="ainfo__cgstatus"
        :class="{ muted: !useCodegraph || !cgInstalled }"
        :style="{ fontSize: prefs.fs(10) + 'px' }"
      >
        {{ cgMCPText }}
      </div>

      <template v-if="meta.summary">
        <div class="ainfo__sep"></div>
        <div class="ainfo__label" :style="{ fontSize: prefs.fs(11) + 'px' }">会话摘要</div>
        <div class="ainfo__summary" :title="meta.summary" :style="{ fontSize: prefs.fs(11) + 'px' }">
          {{ meta.summary }}
        </div>
      </template>
    </template>
    <div v-else class="ainfo__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">（无会话）</div>
    <div v-if="chat.status.lastError" class="ainfo__error" :style="{ fontSize: prefs.fs(11) + 'px' }">
      {{ chat.status.lastError }}
    </div>
  </div>
</template>

<style scoped>
.ainfo {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-family: SimSun, serif;
  height: 100%;
  overflow-y: auto;
  scrollbar-width: none;
}

.ainfo__agent {
  color: var(--war-gold);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.ainfo__grid {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  column-gap: 8px;
  row-gap: 2px;
  align-items: baseline;
}

.ainfo__k {
  color: var(--war-text-muted);
  white-space: nowrap;
}

.ainfo__v {
  color: var(--war-text-dim);
  min-width: 0;
  overflow-wrap: anywhere;
}

.ainfo__sep {
  height: 1px;
  background: #2a3344;
  margin: 4px 0;
}

.ainfo__label {
  color: var(--war-text-muted);
}

.ainfo__path {
  color: var(--war-text-dim);
  overflow-wrap: anywhere;
}

.ainfo__summary {
  color: var(--war-text-dim);
  display: -webkit-box;
  -webkit-line-clamp: 4;
  -webkit-box-orient: vertical;
  overflow: hidden;
  overflow-wrap: anywhere;
}

.ainfo__empty {
  color: var(--war-text-faint);
  text-align: center;
  padding: 8px 0;
}

.ainfo__bind {
  color: var(--war-gold);
  user-select: none;
}

.ainfo__bind:hover {
  color: var(--war-gold-bright);
}

.ainfo__refmode {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  align-self: flex-start;
  padding: 1px 8px;
  border: 1px solid #6a5a3f;
  border-radius: 3px;
  background: #0d1116f0;
  color: var(--war-text-faint);
  cursor: pointer;
  user-select: none;
  white-space: nowrap;
}

.ainfo__refmode:hover {
  border-color: #f2cf6b;
  color: var(--war-text);
}

.ainfo__refmode.active {
  border-color: #2c4a7a;
  color: var(--war-gold);
}

.ainfo__refmode-dot {
  font-weight: 700;
}

.ainfo__toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--war-text-muted);
  cursor: pointer;
  user-select: none;
  flex-wrap: wrap;
}

.ainfo__toggle input {
  accent-color: var(--war-gold);
  cursor: pointer;
}

.ainfo__cgstatus {
  color: var(--war-text-dim);
  font-family: SimSun, serif;
  overflow-wrap: anywhere;
}

.ainfo__cgstatus.muted {
  color: var(--war-text-faint);
}

.ainfo__error {
  color: var(--war-error);
  margin-top: auto;
  overflow-wrap: break-word;
}
</style>

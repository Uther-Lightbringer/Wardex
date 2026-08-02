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

const agentLine = computed(() => {
  if (!meta.value) return '';
  return `${meta.value.agentName || 'Agent'} · ${meta.value.provider}`;
});

// 基础信息组：模型 / 消息数 / 创建 / 更新（空值行自动省略）。
const infoRows = computed<InfoRow[]>(() => {
  const m = meta.value;
  if (!m) return [];
  const rows: InfoRow[] = [];
  if (m.model) rows.push({ k: '模型', v: m.model });
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
    rows.push({ k: 'tokens', v: `↑${formatTokens(u.inputTokens)} ↓${formatTokens(u.outputTokens)}` });
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

.ainfo__error {
  color: var(--war-error);
  margin-top: auto;
  overflow-wrap: break-word;
}
</style>

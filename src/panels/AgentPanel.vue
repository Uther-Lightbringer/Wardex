<script setup lang="ts">
// Session info panel (features/chat.md §6.2): agentName · provider, model,
// message count + created/updated timestamps, per-session token usage
// (front-end sum of already-loaded rows — no extra IPC), work directory,
// session summary, and the sticky lastError line (startup errors like "no
// usable default agent" surface here).
import { computed } from 'vue';
import { useChatStore } from '../stores/chat';
import { usePrefsStore } from '../stores/prefs';
import { useUiStore } from '../stores/ui';
import { formatTokens } from '../lib/format';

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

const modelLine = computed(() => (meta.value?.model ? `模型 ${meta.value.model}` : ''));

const statsLine = computed(() => {
  if (!meta.value) return '';
  return `消息 ${meta.value.messageCount} · 更新 ${stamp(meta.value.updatedAt)}`;
});

const createdLine = computed(() =>
  meta.value ? `创建于 ${stamp(meta.value.createdAt)}` : '',
);

// Cached computed over already-loaded rows: no backend round-trip, so it
// stays in sync with the title-row usage at zero extra cost.
const sessionUsage = computed(() => {
  let input = 0;
  let output = 0;
  for (const r of chat.rows) {
    if (r.role !== 'assistant' || !r.usage) continue;
    input += r.usage.inputTokens;
    output += r.usage.outputTokens;
  }
  if (input === 0 && output === 0) return '';
  return `tokens ↑${formatTokens(input)} ↓${formatTokens(output)}`;
});

// 缓存读写（cachedRead/cachedWrite 合计）+ 思考 token 合计 + 当前上下文
// 估算（最近一轮 input，kimi 每次请求带全量上下文，故最末轮 input ≈ 当前
// 上下文大小，含缓存前缀）。
const cacheLine = computed(() => {
  let cachedRead = 0;
  let cachedWrite = 0;
  let thought = 0;
  let ctx = 0;
  for (const r of chat.rows) {
    if (r.role !== 'assistant' || !r.usage) continue;
    cachedRead += r.usage.cachedReadTokens ?? 0;
    cachedWrite += r.usage.cachedWriteTokens ?? 0;
    thought += r.usage.thoughtTokens ?? 0;
    if (r.usage.inputTokens > 0) ctx = r.usage.inputTokens;
  }
  const parts: string[] = [];
  if (cachedRead > 0) parts.push(`缓存读 ↑${formatTokens(cachedRead)}`);
  if (cachedWrite > 0) parts.push(`缓存写 ↑${formatTokens(cachedWrite)}`);
  if (thought > 0) parts.push(`思考 ↑${formatTokens(thought)}`);
  if (ctx > 0) parts.push(`上下文 ≈ ${formatTokens(ctx)}`);
  return parts.join(' · ');
});

const cacheHint =
  '缓存读：各轮 cachedRead 合计；缓存写：cachedWrite 合计；上下文为估算值（最近一轮输入量，含缓存前缀）';
</script>

<template>
  <div class="ainfo">
    <template v-if="meta">
      <div class="ainfo__agent" :style="{ fontSize: prefs.fs(13) + 'px' }">{{ agentLine }}</div>
      <div v-if="modelLine" class="ainfo__stats" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ modelLine }}</div>
      <div class="ainfo__stats" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ statsLine }}</div>
      <div class="ainfo__stats" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ createdLine }}</div>
      <div v-if="sessionUsage" class="ainfo__stats" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ sessionUsage }}</div>
      <div v-if="cacheLine" class="ainfo__stats" :title="cacheHint" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ cacheLine }}</div>
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

.ainfo__stats {
  color: var(--war-text-muted);
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

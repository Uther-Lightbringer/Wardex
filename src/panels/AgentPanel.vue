<script setup lang="ts">
// Session info panel (features/chat.md §6.2): agentName · provider, message
// count + updatedAt, work directory, and the sticky lastError line (startup
// errors like "no usable default agent" surface here).
import { computed } from 'vue';
import { useChatStore } from '../stores/chat';
import { usePrefsStore } from '../stores/prefs';

const chat = useChatStore();
const prefs = usePrefsStore();

const meta = computed(() => chat.meta);

const agentLine = computed(() => {
  if (!meta.value) return '';
  return `${meta.value.agentName || 'Agent'} · ${meta.value.provider}`;
});

const statsLine = computed(() => {
  if (!meta.value) return '';
  const d = new Date(meta.value.updatedAt);
  const p = (n: number) => String(n).padStart(2, '0');
  const stamp = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
  return `消息 ${meta.value.messageCount} · ${stamp}`;
});
</script>

<template>
  <div class="ainfo">
    <template v-if="meta">
      <div class="ainfo__agent" :style="{ fontSize: prefs.fs(13) + 'px' }">{{ agentLine }}</div>
      <div class="ainfo__stats" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ statsLine }}</div>
      <div class="ainfo__sep"></div>
      <div class="ainfo__label" :style="{ fontSize: prefs.fs(11) + 'px' }">工作目录</div>
      <div class="ainfo__path" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ meta.workDir || meta.projectDir }}</div>
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

.ainfo__empty {
  color: var(--war-text-faint);
  text-align: center;
  padding: 8px 0;
}

.ainfo__error {
  color: var(--war-error);
  margin-top: auto;
  overflow-wrap: break-word;
}
</style>

<script setup lang="ts">
// Send-queue panel (features/chat.md §4.1): floats above the composer,
// visible while queueLength > 0. Header toggles collapse (auto-expands after
// an enqueue, auto-collapses when drained — driven by the chat store).
// Rows: index + 48-char preview; 引导 (gold, guideAt — cancel the current
// turn and send this entry now) and 移除 (red). 清空 wipes the queue.
//
// NOTE: the backend exposes only the queue LENGTH; previews come from the
// frontend mirror of what this process enqueued (chat.queueMirror).
import { computed } from 'vue';
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';

const chat = useChatStore();
const prefs = usePrefsStore();

const count = computed(() => chat.status.queueLength);

function preview(t: string): string {
  const flat = t.replace(/\s+/g, ' ').trim();
  return flat.length > 48 ? flat.slice(0, 48) + '…' : flat;
}
</script>

<template>
  <div v-if="count > 0" class="queue">
    <div class="queue__head" @click="chat.queueOpen = !chat.queueOpen">
      <span :style="{ fontSize: prefs.fs(12) + 'px' }">
        {{ chat.queueOpen ? '▼' : '▶' }} 发送队列 ({{ count }}/10)
      </span>
      <span class="queue__clear" :style="{ fontSize: prefs.fs(11) + 'px' }" @click.stop="chat.clearQueue()">
        清空
      </span>
    </div>
    <div v-if="chat.queueOpen" class="queue__list">
      <div v-for="(t, i) in chat.queueMirror" :key="i" class="queue__row">
        <span class="queue__text" :style="{ fontSize: prefs.fs(12) + 'px' }">{{ i + 1 }}. {{ preview(t) }}</span>
        <span class="queue__op guide" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="chat.guideAt(i)">引导</span>
        <span class="queue__op remove" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="chat.removeQueueAt(i)">移除</span>
      </div>
      <div v-if="chat.queueMirror.length === 0" class="queue__row dim" :style="{ fontSize: prefs.fs(11) + 'px' }">
        （队列内容在后台运行时）
      </div>
    </div>
  </div>
</template>

<style scoped>
.queue {
  background: #0d1116f0;
  border: 1px solid #6a5a3f;
  border-radius: 3px;
  font-family: SimSun, serif;
}

.queue__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 26px;
  padding: 0 10px;
  color: var(--war-gold);
  user-select: none;
}

.queue__clear {
  color: var(--war-error);
}

.queue__clear:hover {
  color: #ffb0a0;
}

.queue__list {
  max-height: calc(6 * 24px);
  overflow-y: auto;
  padding: 0 6px 6px;
}

.queue__row {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 24px;
  padding: 0 4px;
}

.queue__row.dim {
  color: var(--war-text-faint);
}

.queue__text {
  flex: 1;
  min-width: 0;
  color: var(--war-text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.queue__op.guide {
  color: var(--war-gold);
}

.queue__op.guide:hover {
  color: var(--war-gold-bright);
}

.queue__op.remove {
  color: var(--war-error);
}

.queue__op.remove:hover {
  color: #ffb0a0;
}
</style>

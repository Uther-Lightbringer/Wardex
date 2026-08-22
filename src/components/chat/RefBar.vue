<script setup lang="ts">
// Ref bar (@引用条): floats above the composer like the quote/attachment
// bars. One chip per acknowledged @ reference (picker pick, exact-match,
// preview context menu, or file drop); NOT derived from raw text, so a bare @
// in prose (git@github…, emails, @handles) is never turned into a reference.
// On send the composer expands each chip into a 【引用文件：…】 block.
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';

const chat = useChatStore();
const prefs = usePrefsStore();

function label(path: string, from: number, to: number): string {
  if (from <= 0 || to <= 0) return path;
  return to === from ? `${path}:${from}` : `${path}:${from}-${to}`;
}
</script>

<template>
  <div v-if="chat.composerRefs.length > 0" class="rbar">
    <span class="rbar__label" :style="{ fontSize: prefs.fs(10) + 'px' }">引用文件</span>
    <div
      v-for="(r, i) in chat.composerRefs"
      :key="i"
      class="rbar__chip"
      :title="label(r.path, r.from, r.to)"
    >
      <span class="rbar__text" :style="{ fontSize: prefs.fs(11) + 'px' }">
        {{ label(r.path, r.from, r.to) }}
      </span>
      <span
        class="rbar__x"
        :style="{ fontSize: prefs.fs(10) + 'px' }"
        @click="chat.removeRef(i)"
        >✕</span
      >
    </div>
  </div>
</template>

<style scoped>
.rbar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  padding: 6px 8px;
  background: var(--war-panel-dark);
  border: 1px solid var(--war-border-brown);
  border-radius: 3px;
  width: fit-content;
  max-width: 100%;
}

.rbar__label {
  color: var(--war-text-faint);
  font-family: SimSun, serif;
  user-select: none;
}

.rbar__chip {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: min(420px, 60vw);
  background: #5aa0ff22;
  border: 1px solid #2c4a7a;
  border-radius: 999px;
  padding: 2px 8px;
}

.rbar__text {
  color: var(--war-text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  user-select: none;
}

.rbar__x {
  color: var(--war-error);
  cursor: pointer;
  user-select: none;
  line-height: 1;
}

.rbar__x:hover {
  color: #ffb0a0;
}
</style>

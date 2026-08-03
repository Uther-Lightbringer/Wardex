<script setup lang="ts">
// Quote bar (引用块条): floats above the composer like the attachment bar.
// <selection>…</selection> quotes are NOT embedded in the input text anymore —
// each quote is one chip with an elided summary and an ✕ to remove. The full
// body is the chip title; on send the composer re-wraps them into tags.
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';

const chat = useChatStore();
const prefs = usePrefsStore();

const QUOTE_ELIDE = 24;

function elideQuote(s: string): string {
  const t = s.replace(/\s+/g, ' ').trim();
  const chars = [...t];
  return chars.length <= QUOTE_ELIDE ? t : chars.slice(0, QUOTE_ELIDE).join('') + '…';
}
</script>

<template>
  <div v-if="chat.composerQuotes.length > 0" class="qbar">
    <div v-for="(q, i) in chat.composerQuotes" :key="i" class="qbar__chip" :title="q">
      <span class="qbar__mark" :style="{ fontSize: prefs.fs(11) + 'px' }">❝</span>
      <span class="qbar__text" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ elideQuote(q) }}</span>
      <span class="qbar__x" :style="{ fontSize: prefs.fs(10) + 'px' }" @click="chat.removeComposerQuote(i)">✕</span>
    </div>
  </div>
</template>

<style scoped>
.qbar {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 6px 8px;
  background: #0d1116f0;
  border: 1px solid #6a5a3f;
  border-radius: 3px;
  width: fit-content;
  max-width: 100%;
}

.qbar__chip {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: 100%;
  background: #f2cf6b22;
  border: 1px solid #f2cf6b3d;
  border-radius: 999px;
  padding: 2px 8px;
}

.qbar__mark {
  color: var(--war-gold);
  user-select: none;
}

.qbar__text {
  color: var(--war-text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 340px;
  user-select: none;
}

.qbar__x {
  color: var(--war-error);
  cursor: pointer;
  user-select: none;
  line-height: 1;
}

.qbar__x:hover {
  color: #ffb0a0;
}
</style>

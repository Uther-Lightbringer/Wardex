<script setup lang="ts">
// Quote bar (引用块条): floats above the composer like the attachment bar,
// but LEFT-aligned so it never covers the mode-dropdown popup (which opens
// upward from the composer's right side). <selection>…</selection> quotes
// are NOT embedded in the input text: each quote is one chip with an elided
// summary and an ✕ to remove; the full body is the chip title. On send the
// composer re-wraps them into tags and prepends them to the message.
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
    <span class="qbar__label" :style="{ fontSize: prefs.fs(10) + 'px' }">引用</span>
    <div v-for="(q, i) in chat.composerQuotes" :key="i" class="qbar__chip" :title="q">
      <span class="qbar__text" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ elideQuote(q) }}</span>
      <span class="qbar__x" :style="{ fontSize: prefs.fs(10) + 'px' }" @click="chat.removeComposerQuote(i)">✕</span>
    </div>
  </div>
</template>

<style scoped>
.qbar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  padding: 6px 8px;
  background: #0d1116f0;
  border: 1px solid #6a5a3f;
  border-radius: 3px;
  width: fit-content;
  max-width: 100%;
}

.qbar__label {
  color: var(--war-text-faint);
  font-family: SimSun, serif;
  user-select: none;
}

.qbar__chip {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: min(420px, 60vw);
  background: #f2cf6b22;
  border: 1px solid #f2cf6b3d;
  border-radius: 999px;
  padding: 2px 8px;
}

.qbar__text {
  color: var(--war-text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
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

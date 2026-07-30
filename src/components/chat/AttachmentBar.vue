<script setup lang="ts">
// Attachment bar (features/chat.md §3.5): floats above the composer, top of
// the floating stack. ≤6 chips — image thumbnails (56×56, PreserveAspectCrop)
// or file-icon chips with middle-elided names; ✕ corner badge removes.
import { convertFileSrc } from '@tauri-apps/api/core';
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';

const chat = useChatStore();
const prefs = usePrefsStore();

const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'];
function isImage(p: string): boolean {
  return IMAGE_EXTS.includes(p.split('.').pop()?.toLowerCase() ?? '');
}
function elideName(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  const n = parts[parts.length - 1] ?? p;
  return n.length > 18 ? n.slice(0, 8) + '…' + n.slice(-8) : n;
}
</script>

<template>
  <div v-if="chat.attachments.length > 0" class="atts">
    <div v-for="(p, i) in chat.attachments" :key="p" class="atts__chip" :title="p">
      <img v-if="isImage(p)" class="atts__thumb" :src="convertFileSrc(p)" draggable="false" />
      <div v-else class="atts__file">
        <img class="atts__icon" src="/assets/wc3_extracted/ui/icon-file.png" draggable="false" />
        <span class="atts__name" :style="{ fontSize: prefs.fs(10) + 'px' }">{{ elideName(p) }}</span>
      </div>
      <span class="atts__x" @click="chat.removeAttachment(i)">✕</span>
    </div>
  </div>
</template>

<style scoped>
.atts {
  display: flex;
  gap: 8px;
  padding: 6px 8px;
  background: #0d1116f0;
  border: 1px solid #6a5a3f;
  border-radius: 3px;
  width: fit-content;
  max-width: 100%;
}

.atts__chip {
  position: relative;
  width: 56px;
  height: 56px;
  border: 1px solid #2c4a7a;
  border-radius: 2px;
  background: #1a2334;
  overflow: visible;
}

.atts__thumb {
  width: 100%;
  height: 100%;
  object-fit: cover; /* PreserveAspectCrop */
  border-radius: 2px;
}

.atts__file {
  width: 100%;
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 2px;
  padding: 2px;
  box-sizing: border-box;
}

.atts__icon {
  width: 18px;
  height: 18px;
}

.atts__name {
  color: #c0d0ec;
  max-width: 100%;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.atts__x {
  position: absolute;
  top: -6px;
  right: -6px;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: #0d1116;
  border: 1px solid #6a5a3f;
  color: var(--war-error);
  font-size: 9px;
  display: flex;
  align-items: center;
  justify-content: center;
  user-select: none;
}

.atts__x:hover {
  border-color: var(--war-error);
  color: #ffb0a0;
}
</style>

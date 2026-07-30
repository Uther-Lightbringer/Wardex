<script setup lang="ts">
// File preview dialog (features/chat.md §6.5): frame_popup modal, Esc closes.
// Entry flow: preview_file → >2MB asks first (继续打开 / 外部打开 / 取消);
// binary/unreadable asks 系统默认方式打开. Three bodies: text (line-number
// gutter + editable, save when dirty; 256KB-truncated files are read-only),
// markdown (rendered ⇄ raw-editable toggle, unsaved edits kept), image
// (asset-protocol img, fit width with scroll). Text writes back as UTF-8 via
// save_preview (backend refuses binary/image). Edge/corner drag resizes
// (min 380×480, clamped to the window), persisted on pointerup. Closing
// tears down every copy of the content.
import { computed, ref, watch } from 'vue';
import { convertFileSrc } from '@tauri-apps/api/core';
import { cmd, openPath } from '../../lib/tauri';
import { renderMarkdown } from '../../lib/markdown';
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';
import WarDialog from '../war/WarDialog.vue';
import WarButton from '../war/WarButton.vue';

const ASK_SIZE = 2 * 1024 * 1024;
const MIN_W = 380;
const MIN_H = 480;

interface PreviewOutcome {
  ok: boolean;
  size: number;
  reason?: string;
  image?: boolean;
  text?: string;
  truncated?: boolean;
}

const chat = useChatStore();
const prefs = usePrefsStore();

type Phase = 'closed' | 'ask-size' | 'ask-open' | 'view';
const phase = ref<Phase>('closed');
const path = ref('');
const outcome = ref<PreviewOutcome | null>(null);

const fileName = computed(() => {
  const parts = path.value.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path.value;
});

// ---- entry: watch the shared preview path ----
watch(
  () => chat.previewPath,
  async (p) => {
    if (!p) {
      close();
      return;
    }
    path.value = p;
    try {
      const res = await cmd<PreviewOutcome>('preview_file', { path: p });
      if (chat.previewPath !== p) return; // superseded
      outcome.value = res;
      if (!res.ok) {
        phase.value = 'ask-open';
        return;
      }
      if ((res.size ?? 0) > ASK_SIZE && !res.image) {
        phase.value = 'ask-size';
        return;
      }
      enterView();
    } catch {
      outcome.value = { ok: false, size: 0, reason: 'unreadable' };
      phase.value = 'ask-open';
    }
  },
);

function enterView(): void {
  const res = outcome.value;
  if (!res?.ok) return;
  kind.value = res.image ? 'image' : isMarkdown(path.value) ? 'markdown' : 'text';
  // \r\n → \n on open so the dirty flag never lies (§6.5).
  raw.value = (res.text ?? '').replace(/\r\n/g, '\n');
  original.value = raw.value;
  dirty.value = false;
  statusLine.value = res.truncated ? '文件过大，仅显示前 256KB，内容只读' : '';
  mdShowRaw.value = false;
  wrap.value = false;
  phase.value = 'view';
}

function close(): void {
  phase.value = 'closed';
  outcome.value = null;
  raw.value = '';
  original.value = '';
  dirty.value = false;
  statusLine.value = '';
  if (chat.previewPath) chat.closePreview();
}

function externalOpen(): void {
  void openPath(path.value);
  close();
}

// ---- body state ----
const kind = ref<'text' | 'markdown' | 'image'>('text');
const raw = ref('');
const original = ref('');
const dirty = ref(false);
const statusLine = ref('');
const wrap = ref(false);
const mdShowRaw = ref(false);

function isMarkdown(p: string): boolean {
  const ext = p.split('.').pop()?.toLowerCase() ?? '';
  return ext === 'md' || ext === 'markdown';
}

const truncated = computed(() => outcome.value?.truncated === true);
const editable = computed(() => !truncated.value && kind.value !== 'image');
const canSave = computed(() => dirty.value && editable.value);

const lines = computed(() => raw.value.split('\n').length);

function onEdit(): void {
  dirty.value = raw.value !== original.value;
  if (dirty.value) statusLine.value = '';
  else statusLine.value = truncated.value ? '文件过大，仅显示前 256KB，内容只读' : '';
}

const gutterEl = ref<HTMLElement | null>(null);
const editorEl = ref<HTMLTextAreaElement | null>(null);
function syncGutter(): void {
  if (gutterEl.value && editorEl.value) gutterEl.value.scrollTop = editorEl.value.scrollTop;
}

const mdHtml = computed(() => renderMarkdown(raw.value));

async function save(): Promise<void> {
  try {
    const res = await cmd<{ ok: boolean; error?: string }>('save_preview', {
      path: path.value,
      content: raw.value,
    });
    if (res.ok) {
      original.value = raw.value;
      dirty.value = false;
      statusLine.value = '已保存';
    } else {
      statusLine.value = `保存失败：${res.error ?? '未知原因'}`;
    }
  } catch (e) {
    statusLine.value = `保存失败：${e}`;
  }
}

// ---- dialog geometry: A4 default or the persisted drag size ----
const dlgW = ref(0);
const dlgH = ref(0);
const dlgX = ref(0);
const dlgY = ref(0);

function placeDialog(): void {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  if (prefs.previewWidth > 0 && prefs.previewHeight > 0) {
    dlgW.value = Math.min(prefs.previewWidth, vw - 20);
    dlgH.value = Math.min(prefs.previewHeight, vh - 20);
  } else {
    // A4 portrait (210:297) fitted into 92% of the window (§6.5).
    let h = Math.round(vh * 0.92);
    let w = Math.round((h * 210) / 297);
    if (w > vw * 0.92) {
      w = Math.round(vw * 0.92);
      h = Math.round((w * 297) / 210);
    }
    dlgW.value = Math.max(MIN_W, w);
    dlgH.value = Math.max(MIN_H, h);
  }
  dlgX.value = Math.round((vw - dlgW.value) / 2);
  dlgY.value = Math.round((vh - dlgH.value) / 2);
}

watch(phase, (p) => {
  if (p === 'view') placeDialog();
});

// ---- edge / corner drag resize (12px edges, 26px corners) ----
type Zone = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';
let drag: { zone: Zone; x: number; y: number; w: number; h: number; l: number; t: number } | null =
  null;

function zoneAt(e: MouseEvent, el: HTMLElement): Zone | null {
  const r = el.getBoundingClientRect();
  const x = e.clientX - r.left;
  const y = e.clientY - r.top;
  const E = 12;
  const C = 26;
  const left = x <= E;
  const right = x >= r.width - E;
  const top = y <= E;
  const bottom = y >= r.height - E;
  const cornerX = x <= C || x >= r.width - C;
  const cornerY = y <= C || y >= r.height - C;
  if (cornerX && cornerY) {
    if (left && top) return 'nw';
    if (right && top) return 'ne';
    if (left && bottom) return 'sw';
    return 'se';
  }
  if (left) return 'w';
  if (right) return 'e';
  if (top) return 'n';
  if (bottom) return 's';
  return null;
}

function onEdgeDown(e: MouseEvent): void {
  const el = e.currentTarget as HTMLElement;
  const zone = zoneAt(e, el);
  if (!zone) return;
  e.preventDefault();
  drag = { zone, x: e.clientX, y: e.clientY, w: dlgW.value, h: dlgH.value, l: dlgX.value, t: dlgY.value };
  window.addEventListener('mousemove', onEdgeMove);
  window.addEventListener('mouseup', onEdgeUp, { once: true });
}

function onEdgeMove(e: MouseEvent): void {
  if (!drag) return;
  const dx = e.clientX - drag.x;
  const dy = e.clientY - drag.y;
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  let { w, h, l, t } = { w: drag.w, h: drag.h, l: drag.l, t: drag.t };
  if (drag.zone.includes('e')) w = drag.w + dx;
  if (drag.zone.includes('s')) h = drag.h + dy;
  if (drag.zone.includes('w')) {
    w = drag.w - dx;
    l = drag.l + dx;
  }
  if (drag.zone.includes('n')) {
    h = drag.h - dy;
    t = drag.t + dy;
  }
  if (w < MIN_W) {
    if (drag.zone.includes('w')) l -= MIN_W - w;
    w = MIN_W;
  }
  if (h < MIN_H) {
    if (drag.zone.includes('n')) t -= MIN_H - h;
    h = MIN_H;
  }
  // Clamp inside the window.
  w = Math.min(w, vw - 8);
  h = Math.min(h, vh - 8);
  l = Math.max(4, Math.min(l, vw - w - 4));
  t = Math.max(4, Math.min(t, vh - h - 4));
  dlgW.value = Math.round(w);
  dlgH.value = Math.round(h);
  dlgX.value = Math.round(l);
  dlgY.value = Math.round(t);
}

function onEdgeUp(): void {
  window.removeEventListener('mousemove', onEdgeMove);
  if (!drag) return;
  drag = null;
  void prefs.setPreviewSize(dlgW.value, dlgH.value); // persist on release
}

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape' && phase.value !== 'closed') {
    e.stopPropagation();
    close();
  }
}
watch(
  phase,
  (p) => {
    if (p !== 'closed') window.addEventListener('keydown', onKey, true);
    else window.removeEventListener('keydown', onKey, true);
  },
  { immediate: true },
);
</script>

<template>
  <!-- >2MB ask -->
  <WarDialog
    :open="phase === 'ask-size'"
    title-text="文件较大"
    :message-text="`${fileName} 约 ${(((outcome?.size ?? 0) / 1024 / 1024)).toFixed(1)} MB，超过 2MB。\n继续预览可能较慢，如何选择？`"
    @update:open="close()"
  >
    <WarButton skin="dialog" :width="150" text="继续打开" @activated="enterView" />
    <WarButton skin="dialog" :width="150" text="外部打开" @activated="externalOpen" />
    <WarButton skin="dialog" :width="150" text="取消" @activated="close" />
  </WarDialog>

  <!-- binary / unreadable ask -->
  <WarDialog
    :open="phase === 'ask-open'"
    title-text="无法直接预览"
    :message-text="`${fileName} 不是文本文件，无法直接预览。\n是否用系统默认方式打开？`"
    @update:open="close()"
  >
    <WarButton skin="dialog" :width="190" text="打开" @activated="externalOpen" />
    <WarButton skin="dialog" :width="190" text="取消" @activated="close" />
  </WarDialog>

  <!-- preview body -->
  <div v-if="phase === 'view'" class="pv-mask" @mousedown.self="close">
    <div
      class="pv"
      :style="{ width: dlgW + 'px', height: dlgH + 'px', left: dlgX + 'px', top: dlgY + 'px' }"
      @mousedown="onEdgeDown"
    >
      <div class="pv__frame"></div>
      <div class="pv__inner">
        <div class="pv__title war-outline-black" :style="{ fontSize: prefs.fs(14) + 'px' }">
          {{ fileName }}
        </div>

        <!-- toolbar -->
        <div class="pv__tools">
          <template v-if="kind === 'text'">
            <span class="pv__tool" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="wrap = !wrap">
              换行：{{ wrap ? '开' : '关' }}
            </span>
          </template>
          <template v-if="kind === 'markdown'">
            <span class="pv__tool" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="mdShowRaw = !mdShowRaw">
              {{ mdShowRaw ? '渲染预览' : '显示原文' }}
            </span>
          </template>
          <span
            v-if="canSave"
            class="pv__tool save"
            :style="{ fontSize: prefs.fs(11) + 'px' }"
            @click="save"
            >保存</span
          >
          <span class="pv__status" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ statusLine }}</span>
        </div>

        <!-- text body -->
        <div v-if="kind === 'text'" class="pv__body">
          <div ref="gutterEl" class="pv__gutter">
            <div v-for="n in lines" :key="n" class="pv__gutter-line">{{ n }}</div>
          </div>
          <textarea
            ref="editorEl"
            v-model="raw"
            class="pv__editor"
            :class="{ nowrap: !wrap }"
            :wrap="wrap ? 'soft' : 'off'"
            :readonly="!editable"
            spellcheck="false"
            @input="onEdit"
            @scroll="syncGutter"
          ></textarea>
        </div>

        <!-- markdown body -->
        <div v-else-if="kind === 'markdown'" class="pv__body">
          <textarea
            v-if="mdShowRaw"
            v-model="raw"
            class="pv__editor pv__editor--full"
            spellcheck="false"
            @input="onEdit"
          ></textarea>
          <div v-else class="pv__md md-body" v-html="mdHtml"></div>
        </div>

        <!-- image body -->
        <div v-else class="pv__body pv__body--image">
          <img :src="convertFileSrc(path)" draggable="false" />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.pv-mask {
  position: fixed;
  inset: 0;
  z-index: 90;
  background: #000000b0;
}

.pv {
  position: absolute;
}

.pv__frame {
  position: absolute;
  inset: 0;
  border: 88px 100px 90px 100px solid transparent; /* T R B L (slice 88/100/90/100) */
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.pv__inner {
  position: absolute;
  inset: 88px 100px 90px 100px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  background: var(--war-glass);
  padding: 8px 10px;
  box-sizing: border-box;
}

.pv__title {
  flex: none;
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-weight: bold;
  text-align: center;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  user-select: none;
}

.pv__tools {
  flex: none;
  display: flex;
  align-items: center;
  gap: 14px;
  height: 20px;
}

.pv__tool {
  color: var(--war-gold);
  font-family: SimSun, serif;
  user-select: none;
}

.pv__tool:hover {
  color: var(--war-gold-bright);
}

.pv__tool.save {
  color: #80f0a0;
}

.pv__status {
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pv__body {
  flex: 1;
  min-height: 0;
  display: flex;
  border: 1px solid #2a3344;
  background: #0b0d12;
}

.pv__gutter {
  flex: none;
  width: 48px;
  overflow: hidden;
  border-right: 1px solid #2a3344;
  background: #00000040;
  color: var(--war-text-faint);
  font-family: Consolas, monospace;
  font-size: 12px;
  line-height: 18px;
  text-align: right;
  padding: 4px 6px 4px 0;
  box-sizing: border-box;
  user-select: none;
}

.pv__gutter-line {
  height: 18px;
}

.pv__editor {
  flex: 1;
  min-width: 0;
  resize: none;
  border: none;
  outline: none;
  background: transparent;
  color: var(--war-text);
  font-family: Consolas, monospace;
  font-size: 12px;
  line-height: 18px;
  padding: 4px 8px;
  white-space: pre-wrap;
  overflow-wrap: break-word;
}

.pv__editor.nowrap {
  white-space: pre;
  overflow-wrap: normal;
  overflow-x: auto;
}

.pv__editor--full {
  width: 100%;
}

.pv__md {
  flex: 1;
  overflow-y: auto;
  padding: 8px 12px;
  color: var(--war-text);
  font-family: SimSun, serif;
  font-size: 13px;
}

.pv__body--image {
  overflow: auto;
  align-items: flex-start;
  justify-content: center;
  display: flex;
}

.pv__body--image img {
  max-width: 100%;
  object-fit: contain;
}
</style>

<script setup lang="ts">
// Diff detail dialog (GitLab-style): opened by clicking a commit in the
// version-control panel's 历史 tab or a changed file in the 更改 tab. Left: the commit's changed files with
// +add/−del counts; right: the selected file's unified diff (old/new line
// numbers, +/- sign column, green/red row tints, hunk headers).
// Data comes from git_diff_commit (one fetch for the whole commit — file
// switching is pure frontend filtering). Esc / mask click closes.
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import type { FileDiff, GitDiff } from './git-types';
import { usePrefsStore } from '../stores/prefs';
import WarButton from '../components/war/WarButton.vue';
import WarScrollBar from '../components/war/WarScrollBar.vue';

const props = withDefaults(
  defineProps<{
    open: boolean;
    /** e.g. "Add PRD documents" */
    subject: string;
    /** e.g. "ff7c0d9 · Sitr2022 · 2026-08-01" */
    meta: string;
    diff: GitDiff | null;
    loading: boolean;
    error: string;
  }>(),
  { diff: null },
);

const emit = defineEmits<{ (e: 'update:open', v: boolean): void }>();

const prefs = usePrefsStore();
const selectedPath = ref('');

const files = computed(() => props.diff?.files ?? []);

interface FileStat {
  adds: number;
  dels: number;
}

const stats = computed<Record<string, FileStat>>(() => {
  const m: Record<string, FileStat> = {};
  for (const f of files.value) {
    m[f.path] = {
      adds: f.lines.filter((l) => l.kind === 'add').length,
      dels: f.lines.filter((l) => l.kind === 'del').length,
    };
  }
  return m;
});

const selected = computed<FileDiff | null>(
  () => files.value.find((f) => f.path === selectedPath.value) ?? files.value[0] ?? null,
);

// Reset the selection to the first file whenever a new diff arrives.
watch(
  () => props.diff,
  (d) => {
    selectedPath.value = d?.files[0]?.path ?? '';
  },
);

function close(): void {
  emit('update:open', false);
}

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.stopPropagation();
    close();
  }
}

watch(
  () => props.open,
  (v) => {
    if (v) window.addEventListener('keydown', onKey, true);
    else window.removeEventListener('keydown', onKey, true);
  },
);
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true));

// ---- scroll targets (WC3 WarScrollBar) ----
const filesEl = ref<HTMLElement | null>(null);
const diffEl = ref<HTMLElement | null>(null);

// ---- edge/corner resizing (drag the stone frame) ----
const DEFAULT_W = 960;
const DEFAULT_H = 640;
const MIN_W = 520;
const MIN_H = 360;
const dlgW = ref(DEFAULT_W);
const dlgH = ref(DEFAULT_H);

type Dir = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';
const DIRS: Dir[] = ['n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw'];

let rsDir: Dir | null = null;
let rsX = 0;
let rsY = 0;
let rsW = 0;
let rsH = 0;
const rsActive = ref(false);

function clampSize(): void {
  dlgW.value = Math.min(Math.max(dlgW.value, MIN_W), window.innerWidth - 24);
  dlgH.value = Math.min(Math.max(dlgH.value, MIN_H), window.innerHeight - 24);
}

function onRsDown(dir: Dir, e: PointerEvent): void {
  rsDir = dir;
  rsActive.value = true;
  rsX = e.clientX;
  rsY = e.clientY;
  rsW = dlgW.value;
  rsH = dlgH.value;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  e.preventDefault();
  e.stopPropagation();
}

function onRsMove(e: PointerEvent): void {
  if (!rsDir) return;
  if (!(e.buttons & 1)) {
    rsDir = null;
    rsActive.value = false;
    return;
  }
  const dx = e.clientX - rsX;
  const dy = e.clientY - rsY;
  if (rsDir.includes('e')) dlgW.value = rsW + dx;
  if (rsDir.includes('w')) dlgW.value = rsW - dx;
  if (rsDir.includes('s')) dlgH.value = rsH + dy;
  if (rsDir.includes('n')) dlgH.value = rsH - dy;
  clampSize();
}

function onRsUp(): void {
  rsDir = null;
  rsActive.value = false;
}
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="gcd-mask" @mousedown.self="close">
      <div
        class="gcd"
        :style="{ width: dlgW + 'px', height: dlgH + 'px' }"
        :class="{ resizing: rsActive }"
      >
        <div class="gcd__frame"></div>

        <!-- edge/corner resize handles over the stone rim -->
        <div
          v-for="d in DIRS"
          :key="d"
          class="gcd__rs"
          :class="`gcd__rs--${d}`"
          @pointerdown="onRsDown(d, $event)"
          @pointermove="onRsMove"
          @pointerup="onRsUp"
          @pointercancel="onRsUp"
        ></div>

        <div class="gcd__inner">
          <!-- header: commit info + totals -->
          <div class="gcd__head">
            <div class="gcd__head-text">
              <div class="gcd__subject" :style="{ fontSize: prefs.fs(14) + 'px' }" :title="subject">
                {{ subject }}
              </div>
              <div class="gcd__meta" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ meta }}</div>
            </div>
            <div v-if="diff" class="gcd__totals" :style="{ fontSize: prefs.fs(11) + 'px' }">
              {{ files.length }} 个文件 ·
              <span class="gcd__add">+{{ files.reduce((n, f) => n + stats[f.path].adds, 0) }}</span>
              <span class="gcd__del">−{{ files.reduce((n, f) => n + stats[f.path].dels, 0) }}</span>
            </div>
            <span class="gcd__close" title="关闭" @click="close">✕</span>
          </div>

          <div v-if="error" class="gcd__error" :style="{ fontSize: prefs.fs(12) + 'px' }">{{ error }}</div>
          <div v-else-if="loading" class="gcd__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">加载中…</div>
          <div v-else-if="!diff || files.length === 0" class="gcd__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">
            （无差异）
          </div>

          <!-- body: file list | diff -->
          <div v-else class="gcd__body">
            <div class="gcd__files-wrap">
              <div ref="filesEl" class="gcd__files">
                <div
                  v-for="f in files"
                  :key="f.path"
                  class="gcd__file-row"
                  :class="{ active: selected?.path === f.path }"
                  :title="f.path"
                  @click="selectedPath = f.path"
                >
                  <span class="gcd__file-path" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ f.path }}</span>
                  <span class="gcd__file-stat" :style="{ fontSize: prefs.fs(10) + 'px' }">
                    <span class="gcd__add">+{{ stats[f.path].adds }}</span>
                    <span class="gcd__del">−{{ stats[f.path].dels }}</span>
                  </span>
                </div>
              </div>
              <WarScrollBar :target="filesEl" />
            </div>

            <div class="gcd__diff-wrap">
              <div ref="diffEl" class="gcd__diff">
                <template v-if="selected">
                  <div class="gcd__diff-head" :style="{ fontSize: prefs.fs(11) + 'px' }">
                    <span class="gcd__diff-path" :title="selected.path">{{ selected.path }}</span>
                    <span v-if="selected.binary" class="gcd__diff-bin">（二进制）</span>
                  </div>
                  <div
                    v-for="(l, i) in selected.lines"
                    :key="i"
                    class="gcd__line"
                    :class="`gcd__line--${l.kind}`"
                    :style="{ fontSize: prefs.fs(11) + 'px' }"
                  >
                    <template v-if="l.kind === 'add' || l.kind === 'del' || l.kind === 'ctx'">
                      <span class="gcd__ln">{{ l.old_lineno ?? '' }}</span>
                      <span class="gcd__ln">{{ l.new_lineno ?? '' }}</span>
                      <span class="gcd__sign">{{ l.kind === 'add' ? '+' : l.kind === 'del' ? '-' : ' ' }}</span>
                      <span class="gcd__code">{{ l.text }}</span>
                    </template>
                    <template v-else>
                      <span class="gcd__code gcd__code--wide">{{ l.text }}</span>
                    </template>
                  </div>
                  <div v-if="diff.truncated" class="gcd__trunc" :style="{ fontSize: prefs.fs(11) + 'px' }">
                    …（内容超过 64KB，已截断）
                  </div>
                </template>
              </div>
              <WarScrollBar :target="diffEl" />
            </div>
          </div>

          <!-- footer -->
          <div class="gcd__footer">
            <WarButton skin="dialog" :width="150" text="关闭" @activated="close" />
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.gcd-mask {
  position: fixed;
  inset: 0;
  z-index: 110;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.gcd {
  position: relative;
}

.gcd.resizing {
  user-select: none;
}

/* ---- resize handles over the stone rim (pointer-events cut through the frame) ---- */
.gcd__rs {
  position: absolute;
  z-index: 5;
}

.gcd__rs--n,
.gcd__rs--s {
  left: 48px;
  right: 48px;
  height: 22px;
  cursor: ns-resize;
}

.gcd__rs--n {
  top: 0;
}

.gcd__rs--s {
  bottom: 0;
}

.gcd__rs--e,
.gcd__rs--w {
  top: 48px;
  bottom: 48px;
  width: 22px;
  cursor: ew-resize;
}

.gcd__rs--e {
  right: 0;
}

.gcd__rs--w {
  left: 0;
}

.gcd__rs--ne,
.gcd__rs--nw,
.gcd__rs--se,
.gcd__rs--sw {
  width: 48px;
  height: 48px;
}

.gcd__rs--ne {
  top: 0;
  right: 0;
  cursor: nesw-resize;
}

.gcd__rs--nw {
  top: 0;
  left: 0;
  cursor: nwse-resize;
}

.gcd__rs--se {
  bottom: 0;
  right: 0;
  cursor: nwse-resize;
}

.gcd__rs--sw {
  bottom: 0;
  left: 0;
  cursor: nesw-resize;
}

/* frame_popup.png nine-slice (slice 88/100/90/100, center painted → fill) */
.gcd__frame {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 88px 100px 90px 100px;
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.gcd__inner {
  position: absolute;
  /* inside the gold rim (hole ≈ 56/60/52/58 visual) with a breathing gap */
  inset: 60px 64px 56px 62px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-height: 0;
  font-family: SimSun, serif;
}

.gcd__head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 12px;
}

.gcd__head-text {
  flex: 1;
  min-width: 0;
}

.gcd__subject {
  color: var(--war-gold);
  font-weight: bold;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.gcd__meta {
  color: var(--war-text-muted);
}

.gcd__totals {
  flex: none;
  color: var(--war-text-dim);
  white-space: nowrap;
}

.gcd__add {
  color: #7ec87e;
  margin-left: 4px;
}

.gcd__del {
  color: var(--war-error);
  margin-left: 4px;
}

.gcd__close {
  flex: none;
  color: var(--war-text-dim);
  padding: 2px 6px;
  user-select: none;
}

.gcd__close:hover {
  color: var(--war-gold-bright);
}

.gcd__error {
  color: var(--war-error);
  overflow-wrap: break-word;
}

.gcd__empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--war-text-faint);
}

.gcd__body {
  flex: 1;
  min-height: 0;
  display: flex;
  gap: 8px;
}

/* ---- left: changed files ---- */

.gcd__files-wrap {
  flex: none;
  width: 250px;
  display: flex;
  border: 1px solid #1a2230;
  background: #10141dcc;
}

.gcd__files {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none;
}

.gcd__file-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 8px;
  border-bottom: 1px solid #141a26;
  user-select: none;
}

.gcd__file-row:hover {
  background: #1a2334;
}

.gcd__file-row.active {
  background: #1c2a44;
  box-shadow: inset 2px 0 0 var(--war-gold-input);
}

.gcd__file-path {
  flex: 1;
  min-width: 0;
  color: var(--war-text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  direction: rtl; /* keep the tail (file name) visible */
  text-align: left;
}

.gcd__file-row.active .gcd__file-path {
  color: var(--war-gold);
}

.gcd__file-stat {
  flex: none;
  white-space: nowrap;
}

/* ---- right: selected file diff ---- */

.gcd__diff-wrap {
  flex: 1;
  min-width: 0;
  display: flex;
  border: 1px solid #1a2230;
  background: #0d111899;
}

.gcd__diff {
  flex: 1;
  min-width: 0;
  overflow: auto;
  scrollbar-width: none;
}

/* horizontal native scrollbar (long diff lines); vertical is the WC3 bar */
.gcd__diff::-webkit-scrollbar {
  width: 0;
  height: 10px;
}

.gcd__diff::-webkit-scrollbar-track {
  background: #0a0d14;
}

.gcd__diff::-webkit-scrollbar-thumb {
  background: #2a3448;
  border: 1px solid #141a26;
}

.gcd__diff::-webkit-scrollbar-thumb:hover {
  background: #3a4763;
}

.gcd__diff-head {
  position: sticky;
  top: 0;
  z-index: 1;
  padding: 4px 10px;
  background: #141a26;
  border-bottom: 1px solid #1a2230;
  color: var(--war-user-blue);
}

.gcd__diff-bin {
  color: var(--war-text-muted);
}

.gcd__line {
  display: flex;
  white-space: pre;
  font-family: Consolas, 'Cascadia Mono', monospace;
}

.gcd__ln {
  flex: none;
  width: 4ch;
  min-width: 4ch;
  margin-right: 4px;
  text-align: right;
  color: var(--war-text-faint);
  background: #ffffff06;
  user-select: none;
}

.gcd__sign {
  flex: none;
  width: 1ch;
  margin: 0 4px;
  user-select: none;
}

.gcd__code {
  flex: 1;
  padding-right: 8px;
}

.gcd__code--wide {
  padding-left: 12px;
}

.gcd__line--add {
  color: #9be39b;
  background: #28a74526;
}

.gcd__line--del {
  color: #e8a090;
  background: #d0342c26;
}

.gcd__line--ctx {
  color: var(--war-text-muted);
}

.gcd__line--hunk {
  color: var(--war-user-blue);
  background: #7eb6ff14;
}

.gcd__line--meta,
.gcd__line--eof {
  color: var(--war-text-faint);
}

.gcd__trunc {
  color: var(--war-gold);
  text-align: center;
  padding: 4px 0;
}

.gcd__footer {
  flex: none;
  display: flex;
  justify-content: center;
  padding-top: 2px;
}
</style>

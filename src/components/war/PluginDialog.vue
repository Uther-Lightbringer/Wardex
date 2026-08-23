<script setup lang="ts">
// Floating dialog surface for UI plugins (插件化改造 阶段②): the SAME
// sandboxed PluginPanel (srcdoc iframe) rendered in a draggable window
// instead of the side drawer. plugin.json opts in via "surface":
//   "dialog" — rail button opens this window directly
//   "both"   — drawer tab + bridge message {type:'window',op:'open'} promotes
// Position + size are freely draggable/resizable (corner handle) and
// persisted per plugin id in panelLayout prefs (dialogX/Y/W/H).
import { computed, onMounted, ref } from 'vue';
import PluginPanel from './PluginPanel.vue';
import { usePrefsStore } from '../../stores/prefs';

const props = defineProps<{
  pluginId: string;
  title: string;
  src: string;
  /** Initial position cascade index so stacked dialogs don't overlap. */
  index: number;
}>();

const emit = defineEmits<{ close: [] }>();
const prefs = usePrefsStore();

const MIN_W = 320;
const MIN_H = 240;

const pos = ref({ x: 160 + props.index * 32, y: 90 + props.index * 28 });
const size = ref({ w: 560, h: 420 });
onMounted(() => {
  const saved = prefs.panelLayout[props.pluginId];
  if (saved?.dialogW) size.value.w = saved.dialogW;
  if (saved?.dialogH) size.value.h = saved.dialogH;
  if (saved?.dialogX != null) pos.value.x = saved.dialogX;
  if (saved?.dialogY != null) pos.value.y = saved.dialogY;
  clampToViewport(true);
});
/** Keep the window fully inside the viewport; `recover` also pulls a
 * fully-offscreen window back to its cascade origin. */
function clampToViewport(recover = false): void {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  size.value.w = Math.min(size.value.w, vw - 8);
  size.value.h = Math.min(size.value.h, vh - 8);
  size.value.w = Math.max(MIN_W, size.value.w);
  size.value.h = Math.max(MIN_H, size.value.h);
  const off = recover && (pos.value.x >= vw || pos.value.y >= vh || pos.value.x + size.value.w <= 0 || pos.value.y + size.value.h <= 0);
  if (off) {
    pos.value.x = Math.max(0, 160 + props.index * 32);
    pos.value.y = Math.max(0, 90 + props.index * 28);
    return;
  }
  pos.value.x = Math.min(Math.max(pos.value.x, -size.value.w + 80), vw - 80);
  pos.value.y = Math.min(Math.max(pos.value.y, 0), vh - 40);
}
window.addEventListener('resize', () => clampToViewport());
/** Debounced persist of geometry to panelLayout prefs. */
let saveTimer: ReturnType<typeof setTimeout> | null = null;
function saveGeometry(): void {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    prefs.setPanelLayoutLocal(props.pluginId, {
      dialogX: Math.round(pos.value.x),
      dialogY: Math.round(pos.value.y),
      dialogW: Math.round(size.value.w),
      dialogH: Math.round(size.value.h),
    });
    void prefs.persistPanelLayout(props.pluginId);
  }, 400);
}

const dragging = ref(false);
let dragOff = { x: 0, y: 0 };

function onDragStart(e: MouseEvent): void {
  dragging.value = true;
  dragOff = { x: e.clientX - pos.value.x, y: e.clientY - pos.value.y };
  window.addEventListener('mousemove', onDragMove);
  window.addEventListener('mouseup', onDragEnd);
}
function onDragMove(e: MouseEvent): void {
  if (!dragging.value) return;
  pos.value.x = Math.max(0, e.clientX - dragOff.x);
  pos.value.y = Math.max(0, e.clientY - dragOff.y);
}
function onDragEnd(): void {
  dragging.value = false;
  window.removeEventListener('mousemove', onDragMove);
  window.removeEventListener('mouseup', onDragEnd);
  saveGeometry();
}

// 8-direction drag-resize: edges + corners via data-dir handles
// ("e"|"w"|"s"|"n"|"ne"|"nw"|"se"|"sw"). Delta from press point, clamped
// to [MIN_W/H .. viewport].
const resizing = ref(false);
let rDir = '';
let rs = { mx: 0, my: 0, x: 0, y: 0, w: 0, h: 0 };
function onResizeStart(e: MouseEvent, dir: string): void {
  resizing.value = true;
  rDir = dir;
  rs = { mx: e.clientX, my: e.clientY, x: pos.value.x, y: pos.value.y, w: size.value.w, h: size.value.h };
  window.addEventListener('mousemove', onResizeMove);
  window.addEventListener('mouseup', onResizeEnd);
}
function onResizeMove(e: MouseEvent): void {
  if (!resizing.value) return;
  const dx = e.clientX - rs.mx;
  const dy = e.clientY - rs.my;
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  let { x, y, w, h } = rs;
  if (rDir.includes('e')) w = Math.min(Math.max(rs.w + dx, MIN_W), vw - rs.x - 4);
  if (rDir.includes('s')) h = Math.min(Math.max(rs.h + dy, MIN_H), vh - rs.y - 4);
  if (rDir.includes('w')) {
    w = Math.min(Math.max(rs.w - dx, MIN_W), rs.x + rs.w - 4);
    x = rs.x + rs.w - w;
  }
  if (rDir.includes('n')) {
    h = Math.min(Math.max(rs.h - dy, MIN_H), rs.y + rs.h - 4);
    y = rs.y + rs.h - h;
  }
  pos.value = { x, y };
  size.value = { w, h };
}
function onResizeEnd(): void {
  resizing.value = false;
  window.removeEventListener('mousemove', onResizeMove);
  window.removeEventListener('mouseup', onResizeEnd);
  saveGeometry();
}

const style = computed(() => ({
  left: `${pos.value.x}px`,
  top: `${pos.value.y}px`,
  width: `${size.value.w}px`,
  height: `${size.value.h}px`,
  zIndex: `${1000 + props.index}`,
}));
</script>

<template>
  <div class="plugin-dialog" :class="{ dragging }" :style="style">
    <div class="plugin-dialog__bar" @mousedown.prevent="onDragStart">
      <span class="plugin-dialog__title">{{ title }}</span>
      <button class="plugin-dialog__close" title="关闭" @click.stop="emit('close')">×</button>
    </div>
    <div class="plugin-dialog__body">
      <PluginPanel :src="src" :title="title" :plugin-id="pluginId" />
    </div>
    <!-- 8-direction resize handles: 4 edges + 4 corners -->
    <div
      v-for="dir in ['n','s','e','w','ne','nw','se','sw']"
      :key="dir"
      class="plugin-dialog__rz"
      :class="`plugin-dialog__rz--${dir}`"
      @mousedown.prevent="onResizeStart($event, dir)"
    ></div>
  </div>
</template>

<style scoped>
.plugin-dialog {
  position: fixed;
  display: flex;
  flex-direction: column;
  background: var(--war-surface, #171a24);
  border: 1px solid var(--war-outline-brown, #5a3c1c);
  border-radius: 6px;
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.55);
  overflow: hidden;
}

.plugin-dialog__bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 30px;
  padding: 0 10px;
  cursor: move;
  user-select: none;
  background: rgba(20, 14, 6, 0.85);
  border-bottom: 1px solid var(--war-outline-brown, #5a3c1c);
}

.plugin-dialog__title {
  color: var(--war-gold, #e8c56a);
  font-size: 12px;
}

.plugin-dialog__close {
  border: 0;
  background: transparent;
  color: var(--war-text-dim, #9a9a9a);
  font-size: 16px;
  line-height: 1;
  cursor: pointer;
}
.plugin-dialog__close:hover {
  color: var(--war-gold, #e8c56a);
}

.plugin-dialog__body {
  flex: 1;
  min-height: 0;
}

.plugin-dialog__rz {
  position: absolute;
  z-index: 2;
}
.plugin-dialog__rz--n,
.plugin-dialog__rz--s {
  left: 8px;
  right: 8px;
  height: 5px;
  cursor: ns-resize;
}
.plugin-dialog__rz--e,
.plugin-dialog__rz--w {
  top: 8px;
  bottom: 8px;
  width: 5px;
  cursor: ew-resize;
}
.plugin-dialog__rz--n { top: -3px; }
.plugin-dialog__rz--s { bottom: -3px; }
.plugin-dialog__rz--e { right: -3px; }
.plugin-dialog__rz--w { left: -3px; }
.plugin-dialog__rz--ne,
.plugin-dialog__rz--nw,
.plugin-dialog__rz--se,
.plugin-dialog__rz--sw {
  width: 12px;
  height: 12px;
}
.plugin-dialog__rz--ne { top: -4px; right: -4px; cursor: nesw-resize; }
.plugin-dialog__rz--nw { top: -4px; left: -4px; cursor: nwse-resize; }
.plugin-dialog__rz--se {
  bottom: -4px;
  right: -4px;
  cursor: nwse-resize;
  background:
    linear-gradient(
      135deg,
      transparent 0 6px,
      var(--war-gold-dim, #a9882f) 6px 8px,
      transparent 8px 10px,
      var(--war-gold-dim, #a9882f) 10px
    );
  opacity: 0.7;
}
.plugin-dialog__rz--se:hover {
  opacity: 1;
}
.plugin-dialog__rz--sw { bottom: -4px; left: -4px; cursor: nesw-resize; }
</style>

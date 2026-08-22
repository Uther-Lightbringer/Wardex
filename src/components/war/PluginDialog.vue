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
});
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

// Corner drag-resize (bottom-right handle): delta from press point, clamped.
const resizing = ref(false);
let resizeStart = { mx: 0, my: 0, w: 0, h: 0 };
function onResizeStart(e: MouseEvent): void {
  resizing.value = true;
  resizeStart = { mx: e.clientX, my: e.clientY, w: size.value.w, h: size.value.h };
  window.addEventListener('mousemove', onResizeMove);
  window.addEventListener('mouseup', onResizeEnd);
}
function onResizeMove(e: MouseEvent): void {
  if (!resizing.value) return;
  size.value.w = Math.max(MIN_W, resizeStart.w + (e.clientX - resizeStart.mx));
  size.value.h = Math.max(MIN_H, resizeStart.h + (e.clientY - resizeStart.my));
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
    <div
      class="plugin-dialog__resize"
      @mousedown.prevent="onResizeStart"
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

.plugin-dialog__resize {
  position: absolute;
  right: 0;
  bottom: 0;
  width: 16px;
  height: 16px;
  cursor: nwse-resize;
  border: 0;
  background:
    linear-gradient(
      135deg,
      transparent 0 8px,
      var(--war-gold-dim, #a9882f) 8px 10px,
      transparent 10px 12px,
      var(--war-gold-dim, #a9882f) 12px
    );
  opacity: 0.7;
}
.plugin-dialog__resize:hover {
  opacity: 1;
}
</style>

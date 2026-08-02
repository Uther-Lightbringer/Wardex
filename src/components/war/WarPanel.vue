<script setup lang="ts">
// The ONE drawer panel of the info dock (docs/panels.md §3).
// Unified L2 fat-frame look — panel authors only write the content slot:
//   title bar: frame_fat_bar nine-slice, fixed 28px, collapse arrow (▶ =
//     slide back to the right), hover brightening, click anywhere collapses
//   content:   frame_fat_bar nine-slice (plain rivet rectangle, no top
//     crest — measured T23/R26/B22/L25, hole clean) with hole glass, 250ms
//     drawer slide (translateX), lazily mounted (closed = unmounted)
//   grip:      6px hot zone on the LEFT edge, three-stripe iron grip,
//     drag to resize (min 200px / max 60% of the chat area width); content
//     gets pointer-events: none while dragging
import { defineAsyncComponent, ref, watch } from 'vue';
import { PANEL_MAX_W, type PanelDef } from '../../panels/registry';
import WarFrame from './WarFrame.vue';

const MIN_W = 200;

const props = defineProps<{
  def: PanelDef;
  open: boolean;
  width: number;
  dockWidth: number; // px (chat area width) — for the 60% max constraint
}>();

const emit = defineEmits<{
  (e: 'toggle'): void;
  (e: 'resize', w: number): void;
  (e: 'resizeEnd', w: number): void;
}>();

// Lazy mount: the component code is only imported after the first expand;
// collapsing unmounts it again (zero requests / timers while collapsed).
// everOpened starts false so the immediate watcher also mounts panels that
// START open — initializing it from props.open would skip the mount for them.
const everOpened = ref(false);
const asyncComp = ref<ReturnType<typeof defineAsyncComponent> | null>(null);
watch(
  () => props.open,
  (v) => {
    if (v && !everOpened.value) {
      everOpened.value = true;
      asyncComp.value = defineAsyncComponent(props.def.component);
    }
  },
  { immediate: true },
);

// ---- drag-to-resize (left edge, X axis) ----
const dragging = ref(false);
let dragStartX = 0;
let dragStartW = 0;

function clampW(w: number): number {
  // PANEL_MAX_W keeps the whole right column ≤ the 300px action bay, so the
  // chat area is never squeezed; the 60% rule still applies on narrow windows.
  const max = props.dockWidth > 0 ? Math.min(props.dockWidth * 0.6, PANEL_MAX_W) : PANEL_MAX_W;
  return Math.max(MIN_W, Math.min(max, w));
}

function onGripDown(e: PointerEvent): void {
  dragging.value = true;
  dragStartX = e.clientX;
  dragStartW = props.width;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onGripMove(e: PointerEvent): void {
  if (!dragging.value) return;
  if (!(e.buttons & 1)) {
    onGripUp();
    return;
  }
  // Left-edge grip: dragging left widens, dragging right narrows.
  emit('resize', clampW(dragStartW - (e.clientX - dragStartX)));
}

function onGripUp(): void {
  if (!dragging.value) return;
  dragging.value = false;
  emit('resizeEnd', props.width);
}
</script>

<template>
  <div
    class="war-panel"
    :class="{ 'war-panel--open': open }"
    :style="{ width: width + 'px' }"
  >
    <!-- title bar -->
    <div class="war-panel__bar" @click="emit('toggle')">
      <div class="war-panel__bar-frame"></div>
      <img v-if="def.icon" class="war-panel__icon" :src="def.icon" draggable="false" />
      <span class="war-panel__title">{{ def.title }}</span>
      <span class="war-panel__spacer"></span>
      <span class="war-panel__arrow">▶</span>
    </div>

    <!-- content -->
    <div class="war-panel__body">
      <WarFrame
        v-if="everOpened"
        class="war-panel__frame"
        :class="{ dragging }"
        src="/assets/ui/frames/frame_fat_bar.png"
        :slice="[23, 26, 22, 25]"
        :hole="[23, 26, 22, 25]"
      >
        <component :is="asyncComp" v-if="open && asyncComp" />
      </WarFrame>
    </div>

    <!-- resize grip (left edge) -->
    <div
      v-if="open"
      class="war-panel__grip war-resize-handle"
      @pointerdown="onGripDown"
      @pointermove="onGripMove"
      @pointerup="onGripUp"
    >
      <span></span><span></span><span></span>
    </div>
  </div>
</template>

<style scoped>
.war-panel {
  position: relative;
  flex: none;
  display: flex;
  flex-direction: column;
  height: 100%;
  /* drawer slide: hidden off the right edge, slides in when open (420ms,
     synchronized with the dock width animation in WarDock) */
  transform: translateX(100%);
  transition: transform 420ms ease;
}

.war-panel--open {
  transform: translateX(0);
}

.war-panel__bar {
  position: relative;
  height: 28px;
  display: flex;
  align-items: center;
  padding: 0 14px;
  gap: 6px;
  user-select: none;
  cursor: pointer;
}

/* Open state: the title tucks INTO the panel frame — the bar overlaps the
   frame's top rim (its own thin iron strip hides) so 版本控制/工作区文件
   read as part of the iron panel instead of a separate floating strip. */
.war-panel--open .war-panel__bar {
  z-index: 6;
  margin-bottom: -26px;
}

.war-panel--open .war-panel__bar-frame {
  opacity: 0;
}

.war-panel__bar:hover {
  filter: brightness(1.18);
}

.war-panel__bar-frame {
  position: absolute;
  inset: 0;
  /* frame_fat_bar source slice 28/32/28/32 drawn as a thin title strip */
  border-style: solid;
  border-color: transparent;
  border-width: 4px 8px;
  border-image: url('/assets/ui/frames/frame_fat_bar.png') 28 32 28 32 stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.war-panel__icon {
  position: relative;
  width: 14px;
  height: 14px;
}

.war-panel__title {
  position: relative;
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-size: 12px;
  font-weight: bold;
  text-shadow:
    -1px 0 var(--war-outline-brown), 1px 0 var(--war-outline-brown),
    0 -1px var(--war-outline-brown), 0 1px var(--war-outline-brown);
}

.war-panel__spacer {
  flex: 1;
}

/* Drawer arrow: points right = slide back to the right (collapse). */
.war-panel__arrow {
  position: relative;
  color: var(--war-text-dim);
  font-size: 10px;
}

.war-panel__body {
  flex: 1;
  min-height: 0;
}

.war-panel__frame {
  height: 100%;
}

.war-panel__frame.dragging :deep(.war-frame__content) {
  pointer-events: none;
}

.war-panel__grip {
  position: absolute;
  left: -3px; /* hot zone straddles the content frame's left edge */
  top: 0;
  bottom: 0;
  width: 6px;
  display: flex;
  flex-direction: row;
  align-items: center;
  justify-content: center;
  gap: 1px;
  cursor: col-resize;
  touch-action: none;
  z-index: 5;
}

.war-panel__grip span {
  width: 1px;
  height: 36px;
  background: #6a5a3f;
  opacity: 0;
  transition: opacity 120ms;
}

.war-panel__grip:hover span {
  opacity: 1;
}
</style>

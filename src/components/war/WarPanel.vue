<script setup lang="ts">
// One accordion panel of the info dock (docs/panels.md §3).
// Unified L2 iron-frame look — panel authors only write the content slot:
//   title bar: frame_iron_bar nine-slice, fixed 28px, collapse arrow (▶/▼,
//     200ms rotate), hover brightening
//   content:   frame_iron_panel nine-slice with hole glass, 200ms collapse
//     slide animation, lazily mounted (collapsed = unmounted, no requests)
//   grip:      6px hot zone on the bottom edge, three-stripe iron grip,
//     drag to resize (min 80px / max 60% of the dock height); content gets
//     pointer-events: none while dragging
import { computed, defineAsyncComponent, ref, watch } from 'vue';
import type { PanelDef } from '../../panels/registry';
import WarFrame from './WarFrame.vue';

const MIN_H = 80;

const props = defineProps<{
  def: PanelDef;
  open: boolean;
  height: number;
  dockHeight: number; // px — for the 60% max constraint
}>();

const emit = defineEmits<{
  (e: 'toggle'): void;
  (e: 'resize', h: number): void;
  (e: 'resizeEnd', h: number): void;
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

const bodyH = computed(() => (props.open ? props.height : 0));

// ---- drag-to-resize ----
const dragging = ref(false);
let dragStartY = 0;
let dragStartH = 0;

function clampH(h: number): number {
  const max = props.dockHeight > 0 ? props.dockHeight * 0.6 : Number.MAX_SAFE_INTEGER;
  return Math.max(MIN_H, Math.min(max, h));
}

function onGripDown(e: PointerEvent): void {
  dragging.value = true;
  dragStartY = e.clientY;
  dragStartH = props.height;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onGripMove(e: PointerEvent): void {
  if (!dragging.value) return;
  if (!(e.buttons & 1)) {
    onGripUp();
    return;
  }
  emit('resize', clampH(dragStartH + (e.clientY - dragStartY)));
}

function onGripUp(): void {
  if (!dragging.value) return;
  dragging.value = false;
  emit('resizeEnd', props.height);
}
</script>

<template>
  <div class="war-panel" :class="{ 'war-panel--open': open }">
    <!-- title bar -->
    <div
      class="war-panel__bar"
      :class="{ static: def.alwaysOpen }"
      @click="def.alwaysOpen ? undefined : emit('toggle')"
    >
      <div class="war-panel__bar-frame"></div>
      <img v-if="def.icon" class="war-panel__icon" :src="def.icon" draggable="false" />
      <span class="war-panel__title">{{ def.title }}</span>
      <span class="war-panel__spacer"></span>
      <span v-if="!def.alwaysOpen" class="war-panel__arrow" :class="{ open }">▶</span>
    </div>

    <!-- content (200ms collapse slide) -->
    <div class="war-panel__body" :style="{ height: bodyH + 'px' }">
      <WarFrame
        v-if="everOpened"
        class="war-panel__frame"
        :class="{ dragging }"
        src="/assets/ui/frames/frame_iron_panel.png"
        :slice="[96, 110, 69, 108]"
        :hole="[56, 25, 21, 24]"
      >
        <component :is="asyncComp" v-if="open && asyncComp" />
      </WarFrame>
    </div>

    <!-- resize grip -->
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
}

.war-panel__bar {
  position: relative;
  height: 28px;
  display: flex;
  align-items: center;
  padding: 0 14px;
  gap: 6px;
  user-select: none;
}

/* Open state: the title tucks INTO the panel frame — the bar overlaps the
   frame's top rim (its own thin iron strip hides) so 版本控制/工作区文件
   read as part of the iron panel instead of a separate floating strip. */
.war-panel--open .war-panel__bar {
  z-index: 6;
  margin-bottom: -40px;
}

.war-panel--open .war-panel__bar-frame {
  opacity: 0;
}

.war-panel__bar:not(.static):hover {
  filter: brightness(1.18);
}

.war-panel__bar-frame {
  position: absolute;
  inset: 0;
  /* frame_iron_bar source slice 62/110/70/108 drawn as a thin title strip */
  border-style: solid;
  border-color: transparent;
  border-width: 4px 12px;
  border-image: url('/assets/ui/frames/frame_iron_bar.png') 62 110 70 108 stretch;
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

.war-panel__arrow {
  position: relative;
  color: var(--war-text-dim);
  font-size: 10px;
  transition: transform 200ms;
}

.war-panel__arrow.open {
  transform: rotate(90deg);
}

.war-panel__body {
  overflow: hidden;
  transition: height 200ms ease;
}

.war-panel__frame {
  height: 100%;
}

.war-panel__frame.dragging :deep(.war-frame__content) {
  pointer-events: none;
}

.war-panel__grip {
  position: relative;
  height: 6px;
  margin-top: -3px; /* hot zone straddles the content frame's bottom edge */
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 1px;
  touch-action: none;
  z-index: 5;
}

.war-panel__grip span {
  width: 36px;
  height: 1px;
  background: #6a5a3f;
  opacity: 0;
  transition: opacity 120ms;
}

.war-panel__grip:hover span {
  opacity: 1;
}
</style>

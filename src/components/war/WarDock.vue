<script setup lang="ts">
// Info panel dock (docs/panels.md §1.2): vertical accordion stack driven by
// the registry + panelLayout memory. Owns open/height state (defaults from
// PanelDef, overridden by user_prefs panelLayout) and persists changes via
// the prefs store — toggle persists immediately, drag-resize persists with a
// 300ms debounce after pointerup.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { panelRegistry, type PanelDef } from '../../panels/registry';
import { usePrefsStore } from '../../stores/prefs';
import WarPanel from './WarPanel.vue';

const prefs = usePrefsStore();

const root = ref<HTMLElement | null>(null);
const dockHeight = ref(0);
let ro: ResizeObserver | null = null;

onMounted(() => {
  if (!root.value) return;
  ro = new ResizeObserver(() => {
    dockHeight.value = root.value?.clientHeight ?? 0;
  });
  ro.observe(root.value);
});
onBeforeUnmount(() => ro?.disconnect());

const defs = computed<PanelDef[]>(() =>
  [...panelRegistry].sort(
    (a, b) => (prefs.panelLayout[a.id]?.order ?? a.order) - (prefs.panelLayout[b.id]?.order ?? b.order),
  ),
);

function isOpen(def: PanelDef): boolean {
  if (def.alwaysOpen) return true;
  return prefs.panelLayout[def.id]?.open ?? def.defaultOpen;
}

function heightOf(def: PanelDef): number {
  return prefs.panelLayout[def.id]?.height ?? def.defaultHeight;
}

function toggle(def: PanelDef): void {
  const next = !isOpen(def);
  prefs.setPanelLayoutLocal(def.id, { open: next });
  void prefs.persistPanelLayout(def.id);
}

function onResize(def: PanelDef, h: number): void {
  prefs.setPanelLayoutLocal(def.id, { height: Math.round(h) });
}

// 300ms debounce after pointerup before writing user_prefs.json
const persistTimers = new Map<string, ReturnType<typeof setTimeout>>();
function onResizeEnd(def: PanelDef, h: number): void {
  prefs.setPanelLayoutLocal(def.id, { height: Math.round(h) });
  const prev = persistTimers.get(def.id);
  if (prev) clearTimeout(prev);
  persistTimers.set(
    def.id,
    setTimeout(() => void prefs.persistPanelLayout(def.id), 300),
  );
}
onBeforeUnmount(() => persistTimers.forEach((t) => clearTimeout(t)));
</script>

<template>
  <div ref="root" class="war-dock">
    <WarPanel
      v-for="def in defs"
      :key="def.id"
      :def="def"
      :open="isOpen(def)"
      :height="heightOf(def)"
      :dock-height="dockHeight"
      @toggle="toggle(def)"
      @resize="(h: number) => onResize(def, h)"
      @resize-end="(h: number) => onResizeEnd(def, h)"
    />
  </div>
</template>

<style scoped>
.war-dock {
  display: flex;
  flex-direction: column;
  gap: 4px; /* panels spaced 4px, dock background shows through */
  height: 100%;
  overflow-y: auto;
  scrollbar-width: none;
}
</style>

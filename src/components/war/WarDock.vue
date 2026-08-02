<script setup lang="ts">
// Info panel dock (docs/panels.md §1.2): drawer-style dock — a narrow button
// rail (~44px) on the right edge plus ONE slide-in drawer (mutually
// exclusive). Open state is TRANSIENT (in-memory only, never persisted):
// every app start all panels are collapsed. Owns openId + width state
// (defaults from PanelDef, width overridden by user_prefs panelLayout) and
// persists width via the prefs store with a 300ms debounce after pointerup.
//
// Open/close animation: the dock width animates 44 ↔ 44+panelWidth (420ms
// ease) and the drawer content animates translateX(100%) → 0 with the same
// duration/easing, so the two look synchronized. On open the panel is
// mounted closed first and flipped open two frames later so the transition
// actually runs; on close it stays mounted (visibleId) for the slide-out.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { panelRegistry, PANEL_MAX_W, type PanelDef } from '../../panels/registry';
import { usePrefsStore } from '../../stores/prefs';
import WarPanel from './WarPanel.vue';

const RAIL_W = 44;
const prefs = usePrefsStore();

const root = ref<HTMLElement | null>(null);
const chatWidth = ref(0);
let ro: ResizeObserver | null = null;

onMounted(() => {
  // The 60% max-width constraint is relative to the whole chat area.
  const target = (root.value?.closest('.chat') as HTMLElement | null) ?? root.value;
  if (!target) return;
  ro = new ResizeObserver(() => {
    chatWidth.value = target.clientWidth;
  });
  ro.observe(target);
  // Startup: expand the registry's default-open panel (会话信息). Still
  // transient — closing it is not persisted, next start re-opens it.
  const def = defs.value.find((d) => d.defaultOpen);
  if (def) openPanel(def);
});
onBeforeUnmount(() => ro?.disconnect());

const defs = computed<PanelDef[]>(() =>
  [...panelRegistry].sort(
    (a, b) => (prefs.panelLayout[a.id]?.order ?? a.order) - (prefs.panelLayout[b.id]?.order ?? b.order),
  ),
);

// Drawer open state — transient, never written to panelLayout.
const openId = ref<string | null>(null);
// visibleId keeps the closing panel mounted for the 420ms slide-out.
const visibleId = ref<string | null>(null);
let closeTimer: ReturnType<typeof setTimeout> | null = null;
let switchTimer: ReturnType<typeof setTimeout> | null = null;
let openRaf = 0;

const visibleDef = computed(() => defs.value.find((d) => d.id === visibleId.value) ?? null);
const openDef = computed(() => defs.value.find((d) => d.id === openId.value) ?? null);

function widthOf(def: PanelDef): number {
  // clamp: persisted widths from before PANEL_MAX_W may exceed the cap
  return Math.min(prefs.panelLayout[def.id]?.width ?? def.defaultWidth, PANEL_MAX_W);
}

const dockW = computed(() => RAIL_W + (openDef.value ? widthOf(openDef.value) : 0));

function openPanel(def: PanelDef): void {
  if (closeTimer) {
    clearTimeout(closeTimer);
    closeTimer = null;
  }
  cancelAnimationFrame(openRaf);
  if (openId.value) {
    // Another panel is open: slide it back first, then slide the new one
    // out (sequenced, no direct swap).
    switchPanel(def);
    return;
  }
  visibleId.value = def.id;
  // Flip open two frames after mounting (translateX(100%)) so the slide-in
  // transition runs in sync with the dock width animation.
  openRaf = requestAnimationFrame(() => {
    openRaf = requestAnimationFrame(() => {
      if (visibleId.value === def.id) openId.value = def.id;
    });
  });
}

function closePanel(): void {
  if (!visibleId.value) return;
  openId.value = null;
  cancelAnimationFrame(openRaf);
  if (closeTimer) clearTimeout(closeTimer);
  closeTimer = setTimeout(() => {
    visibleId.value = null;
    closeTimer = null;
  }, 430);
}

// Close the current drawer, wait for its slide-out, then open the next one.
function switchPanel(def: PanelDef): void {
  closePanel();
  if (switchTimer) clearTimeout(switchTimer);
  switchTimer = setTimeout(() => {
    switchTimer = null;
    openPanel(def);
  }, 430);
}

function onRailClick(def: PanelDef): void {
  // A new click always cancels a pending sequenced open.
  if (switchTimer) {
    clearTimeout(switchTimer);
    switchTimer = null;
  }
  if (openId.value === def.id) closePanel();
  else openPanel(def);
}

function onResize(def: PanelDef, w: number): void {
  prefs.setPanelLayoutLocal(def.id, { width: Math.round(w) });
}

// 300ms debounce after pointerup before writing user_prefs.json
const persistTimers = new Map<string, ReturnType<typeof setTimeout>>();
function onResizeEnd(def: PanelDef, w: number): void {
  prefs.setPanelLayoutLocal(def.id, { width: Math.round(w) });
  const prev = persistTimers.get(def.id);
  if (prev) clearTimeout(prev);
  persistTimers.set(
    def.id,
    setTimeout(() => void prefs.persistPanelLayout(def.id), 300),
  );
}
onBeforeUnmount(() => {
  persistTimers.forEach((t) => clearTimeout(t));
  if (closeTimer) clearTimeout(closeTimer);
  if (switchTimer) clearTimeout(switchTimer);
  cancelAnimationFrame(openRaf);
});
</script>

<template>
  <div ref="root" class="war-dock" :style="{ width: dockW + 'px' }">
    <!-- drawer (slides in from the right, pushes the chat area narrower) -->
    <div class="war-dock__drawer">
      <WarPanel
        v-if="visibleDef"
        :key="visibleDef.id"
        :def="visibleDef"
        :open="openId === visibleDef.id"
        :width="widthOf(visibleDef)"
        :dock-width="chatWidth"
        @toggle="closePanel()"
        @resize="(w: number) => onResize(visibleDef!, w)"
        @resize-end="(w: number) => onResizeEnd(visibleDef!, w)"
      />
    </div>

    <!-- right-edge button rail -->
    <div class="war-dock__rail">
      <div
        v-for="def in defs"
        :key="def.id"
        class="war-dock__btn"
        :class="{ active: openId === def.id }"
        :title="def.title"
        @click="onRailClick(def)"
      >
        <div class="war-dock__btn-glass"></div>
        <div class="war-dock__btn-frame"></div>
        <img v-if="def.icon" class="war-dock__btn-icon" :src="def.icon" draggable="false" />
        <span v-else class="war-dock__btn-text">{{ def.title }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.war-dock {
  display: flex;
  flex-direction: row;
  height: 100%;
  /* dock width animation — synchronized with the drawer's translateX */
  transition: width 420ms ease;
}

.war-dock__drawer {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  display: flex;
  /* the panel hugs the rail; the wrapper clips it from the left */
  justify-content: flex-end;
}

.war-dock__rail {
  flex: none;
  width: 44px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding-top: 2px;
  overflow-y: auto;
  scrollbar-width: none;
}

.war-dock__btn {
  position: relative;
  flex: none;
  display: flex;
  align-items: center;
  justify-content: center;
  margin: 0 3px;
  padding: 12px 0;
  user-select: none;
}

/* fat-bar nine-slice tile + glass fill — the same ear-less iron family as
   the drawer frames, so the rail reads as a stack of small iron tabs */
.war-dock__btn-glass {
  position: absolute;
  inset: 6px 5px;
  z-index: 0;
  background: var(--war-glass);
  border-radius: 2px;
}

.war-dock__btn-frame {
  position: absolute;
  inset: 0;
  z-index: 1;
  border-style: solid;
  border-color: transparent;
  border-width: 8px 9px;
  border-image: url('/assets/ui/frames/frame_fat_bar.png') 28 32 28 32 stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.war-dock__btn:hover {
  filter: brightness(1.18);
}

.war-dock__btn:active {
  transform: translateY(1px);
}

.war-dock__btn.active {
  filter: brightness(1.35);
}

.war-dock__btn-icon {
  position: relative;
  z-index: 2;
  width: 18px;
  height: 18px;
}

.war-dock__btn-text {
  position: relative;
  z-index: 2;
  writing-mode: vertical-rl;
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-size: 13px;
  font-weight: bold;
  letter-spacing: 3px;
  text-shadow:
    -1px 0 var(--war-outline-brown), 1px 0 var(--war-outline-brown),
    0 -1px var(--war-outline-brown), 0 1px var(--war-outline-brown);
}

.war-dock__btn.active .war-dock__btn-text {
  color: var(--war-gold-bright);
}
</style>

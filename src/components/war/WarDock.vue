<script setup lang="ts">
// Info panel dock (docs/panels.md §1.2): drawer-style dock — a narrow button
// rail (~44px) on the right edge plus ONE slide-in drawer (mutually
// exclusive). Open state is TRANSIENT (in-memory only, never persisted):
// every app start all panels are collapsed. Width is SHARED by all dock
// tabs (one prefs.panelWidth, persisted via set_panel_width) — dragging
// 会话信息 applies the same width to 后台任务/待办/版本控制/工作区文件/
// 数据库. The open/close animation animates the dock width 44 ↔ 44+w
// (420ms ease) in sync with the drawer's translateX; while the user is
// DRAGGING the width the transition is disabled so the dock tracks the
// pointer freely (no lag), re-enabled on release.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { panelRegistry, PANEL_MAX_W, PANEL_DEFAULT_W, type PanelDef } from '../../panels/registry';
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

// One SHARED width for every dock tab (clamped by PANEL_MAX_W; stored
// values from before the cap may exceed it).
function widthOf(): number {
  return Math.min(prefs.panelWidth, PANEL_MAX_W);
}

const dockW = computed(() => RAIL_W + (openDef.value ? widthOf() : 0));

// True while the user is dragging the drawer width → disables the dock
// width transition so it follows the pointer with zero lag.
const dockDragging = ref(false);

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

// ---- width drag (shared across all panels, persisted on release) ----
function onResizeStart(): void {
  dockDragging.value = true;
}

function onResize(w: number): void {
  prefs.setPanelWidthLocal(w);
}

// `prefs.panelWidth` is already current (set live during the drag) — don't
// trust a possibly stale emitted value here.
function onResizeEnd(): void {
  dockDragging.value = false;
  prefs.setPanelWidthLocal(prefs.panelWidth);
  void prefs.setPanelWidth(prefs.panelWidth);
}

// Double-click the grip → back to the canonical default (like the rail).
function onResizeReset(): void {
  prefs.setPanelWidthLocal(PANEL_DEFAULT_W);
  void prefs.setPanelWidth(PANEL_DEFAULT_W);
}
</script>

<template>
  <div
    ref="root"
    class="war-dock"
    :class="{ 'war-dock--drag': dockDragging }"
    :style="{ width: dockW + 'px' }"
  >
    <!-- drawer (slides in from the right, pushes the chat area narrower) -->
    <div class="war-dock__drawer">
      <WarPanel
        v-if="visibleDef"
        :key="visibleDef.id"
        :def="visibleDef"
        :open="openId === visibleDef.id"
        :width="widthOf()"
        :dock-width="chatWidth"
        @toggle="closePanel()"
        @resize-start="onResizeStart()"
        @resize="(w: number) => onResize(w)"
        @resize-end="onResizeEnd()"
        @reset="onResizeReset()"
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

/* while the user drags the width, follow the pointer with zero lag */
.war-dock--drag {
  transition: none;
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

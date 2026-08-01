<script setup lang="ts">
// WC3-style dropdown (WarDropdown.qml): dropdown_bar2.png closed state (arrow
// is a separate overlay element), dropdown_panel2.png nine-slice list.
// Click the bar to toggle; selecting an option closes and emits.
// dropUp opens the list above the bar (for bars near the window bottom).
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';

const props = withDefaults(
  defineProps<{
    options: string[];
    modelValue?: number; // may stay -1 when displayText is driven externally
    displayText?: string; // overrides the options[modelValue] display
    dropUp?: boolean;
    rowHeight?: number;
    textSize?: number;
    /** Show a filter input at the top of the popup (long lists, e.g. models). */
    filterable?: boolean;
  }>(),
  { modelValue: -1, displayText: undefined, dropUp: false, rowHeight: 28, textSize: 13, filterable: false },
);

const emit = defineEmits<{
  (e: 'update:modelValue', v: number): void;
  (e: 'activated', index: number): void;
}>();

const open = ref(false);
const root = ref<HTMLElement | null>(null);
const pop = ref<HTMLElement | null>(null);
const filter = ref('');
const filterInput = ref<HTMLInputElement | null>(null);

/** Rows with their ORIGINAL option index, so filtering still emits the
 * correct index to the caller. */
const filteredRows = computed(() => {
  const q = filter.value.trim().toLowerCase();
  return props.options
    .map((opt, index) => ({ opt: String(opt), index }))
    .filter((r) => !q || r.opt.toLowerCase().includes(q));
});
// Fixed-position popup coords (teleported to body so no ancestor's
// overflow:hidden — e.g. WarFrame content — can clip the list).
const popStyle = ref<Record<string, string>>({});

const shownText = computed(() => {
  if (props.displayText !== undefined) return props.displayText;
  return props.modelValue >= 0 && props.modelValue < props.options.length
    ? String(props.options[props.modelValue])
    : '';
});

function toggle(): void {
  open.value = !open.value;
}

function select(index: number): void {
  open.value = false;
  emit('update:modelValue', index);
  emit('activated', index);
}

function measure(): void {
  const r = root.value?.getBoundingClientRect();
  if (!r) return;
  popStyle.value = {
    left: `${r.left}px`,
    width: `${Math.max(r.width, 120)}px`,
    ...(props.dropUp
      ? { bottom: `${window.innerHeight - r.top + 2}px` }
      : { top: `${r.bottom + 2}px` }),
  };
}

function onDocDown(e: MouseEvent): void {
  const t = e.target as Node;
  if (root.value?.contains(t) || pop.value?.contains(t)) return;
  open.value = false;
}

function closeOnShift(e?: Event): void {
  // Scrolls INSIDE the popup (wheel / scrollbar on a long filtered list) must
  // not close it — only page-level scrolls and resizes should.
  if (e && e.target instanceof Node && pop.value?.contains(e.target)) return;
  open.value = false;
}

/** Enter in the filter box picks the first visible match. */
function selectFirstFiltered(): void {
  const first = filteredRows.value[0];
  if (first) select(first.index);
}

watch(open, (v) => {
  if (v) {
    filter.value = '';
    void nextTick(() => {
      measure();
      if (props.filterable) filterInput.value?.focus();
    });
    document.addEventListener('mousedown', onDocDown, true);
    window.addEventListener('resize', closeOnShift);
    window.addEventListener('scroll', closeOnShift, true);
  } else {
    document.removeEventListener('mousedown', onDocDown, true);
    window.removeEventListener('resize', closeOnShift);
    window.removeEventListener('scroll', closeOnShift, true);
  }
});
onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onDocDown, true);
  window.removeEventListener('resize', closeOnShift);
  window.removeEventListener('scroll', closeOnShift, true);
});
</script>

<template>
  <div ref="root" class="war-dd">
    <!-- closed-state bar (border layer + label that spans the full width) -->
    <div class="war-dd__bar" @click="toggle">
      <div class="war-dd__bar-frame"></div>
      <span class="war-dd__bar-text" :style="{ fontSize: textSize + 'px' }">{{ shownText }}</span>
      <!-- gold arrow as a separate element: baked-in arrows get mangled by
           border-image edge stretching (double-arrow artifact in WebView2) -->
      <img class="war-dd__arrow" src="/assets/ui/dropdown/dropdown_arrow.png" alt="" />
    </div>

    <!-- expanded list (teleported: fixed coords from the bar's rect) -->
    <Teleport to="body">
      <div v-if="open" ref="pop" class="war-dd__pop" :style="popStyle">
        <div v-if="filterable" class="war-dd__filter">
          <input
            ref="filterInput"
            v-model="filter"
            class="war-inline-input war-dd__filter-input"
            placeholder="筛选…"
            spellcheck="false"
            @keydown.enter.prevent="selectFirstFiltered"
            @keydown.esc.stop="open = false"
            @keydown.stop
          />
        </div>
        <div class="war-dd__pop-inner" :class="{ 'war-dd__pop-inner--fixed': filterable }">
          <div
            v-for="row in filteredRows"
            :key="row.index"
            class="war-dd__row"
            :style="{ height: rowHeight + 'px' }"
            @click="select(row.index)"
          >
            <span class="war-highlight war-dd__row-glow"></span>
            <span
              class="war-dd__row-text"
              :class="{ current: row.index === modelValue }"
              :style="{ fontSize: textSize + 'px' }"
              >{{ row.opt }}</span
            >
          </div>
          <div v-if="filteredRows.length === 0" class="war-dd__empty">（无匹配）</div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.war-dd {
  position: relative;
  width: 140px; /* default; callers override via CSS */
  height: 32px;
}

.war-dd__bar {
  position: absolute;
  inset: 0;
}

.war-dd__bar-frame {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 12px 14px 12px 13px; /* T R B L (slice 20/22/20/22) */
  border-image: url('/assets/ui/dropdown/dropdown_bar2.png') 20 22 20 22 fill stretch;
  /* Solid navy fallback: WebView2 does not always paint the border-image
     center slice (fill), leaving the middle see-through without this.
     padding-box keeps the fallback out of the transparent cut corners. */
  background: #060d33 padding-box;
  box-sizing: border-box;
  pointer-events: none;
}

.war-dd__bar-text {
  position: absolute;
  left: 12px;
  right: 42px; /* clear of the arrow element */

  top: 50%;
  transform: translateY(-50%);
  font-family: SimSun, serif;
  font-weight: bold;
  color: var(--war-gold);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.war-dd__arrow {
  position: absolute;
  right: 10px;
  top: 50%;
  transform: translateY(-50%);
  height: 12px;
  pointer-events: none;
}

.war-dd__pop {
  position: fixed; /* coords come from popStyle (teleported to body) */
  z-index: 2000;
  border-style: solid;
  border-color: transparent;
  border-width: 13px 14px 12px 14px; /* T R B L (slice 21/23/20/23) */
  border-image: url('/assets/ui/dropdown/dropdown_panel2.png') 21 23 20 23 fill stretch;
  background: #060d33 padding-box; /* same fill fallback as the bar (center slice may not paint) */
  box-sizing: border-box;
}

.war-dd__pop-inner {
  padding: 10px 0;
  background: transparent;
  /* Long option lists (e.g. endpoint /models) cap at ~60% of the window and
     scroll, instead of running off the screen. */
  max-height: min(420px, 60vh);
  overflow-y: auto;
  scrollbar-width: thin;
  scrollbar-color: #4a5a8a #0a1230;
}

.war-dd__pop-inner::-webkit-scrollbar {
  width: 8px;
}

.war-dd__pop-inner::-webkit-scrollbar-track {
  background: #0a1230;
}

.war-dd__pop-inner::-webkit-scrollbar-thumb {
  background: #4a5a8a;
  border-radius: 4px;
}

/* Filterable popups pin the list to the full capped height: filtering must
   not shrink the panel and move the filter box (dropUp anchors the bottom
   edge, so a shrinking panel shifts everything above it). */
.war-dd__pop-inner--fixed {
  height: min(420px, 60vh);
}

.war-dd__filter {
  padding: 8px 10px 0;
}

.war-dd__filter-input {
  width: 100%;
  height: 26px;
  font-size: 13px;
  box-sizing: border-box;
}

.war-dd__empty {
  padding: 8px 14px;
  color: var(--war-text-faint);
  font-size: 12px;
  font-family: SimSun, serif;
}

.war-dd__row {
  position: relative;
  display: flex;
  align-items: center;
}

.war-dd__row-glow {
  opacity: 0;
}

.war-dd__row:hover .war-dd__row-glow {
  opacity: 1;
}

.war-dd__row-text {
  position: relative;
  padding-left: 14px;
  padding-right: 10px;
  font-family: SimSun, serif;
  color: var(--war-text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.war-dd__row:hover .war-dd__row-text,
.war-dd__row-text.current {
  color: var(--war-gold);
}

.war-dd__row-text.current {
  font-weight: bold;
}
</style>

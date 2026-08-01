<script setup lang="ts">
// WC3-style dropdown (WarDropdown.qml): dropdown_bar.png closed state (gold
// arrow baked into the right cap), dropdown_panel.png nine-slice list.
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
  }>(),
  { modelValue: -1, displayText: undefined, dropUp: false, rowHeight: 28, textSize: 13 },
);

const emit = defineEmits<{
  (e: 'update:modelValue', v: number): void;
  (e: 'activated', index: number): void;
}>();

const open = ref(false);
const root = ref<HTMLElement | null>(null);
const pop = ref<HTMLElement | null>(null);
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

function closeOnShift(): void {
  open.value = false;
}

watch(open, (v) => {
  if (v) {
    void nextTick(measure);
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
    </div>

    <!-- expanded list (teleported: fixed coords from the bar's rect) -->
    <Teleport to="body">
      <div v-if="open" ref="pop" class="war-dd__pop" :style="popStyle">
        <div class="war-dd__pop-inner">
          <div
            v-for="(opt, i) in options"
            :key="i"
            class="war-dd__row"
            :style="{ height: rowHeight + 'px' }"
            @click="select(i)"
          >
            <span class="war-highlight war-dd__row-glow"></span>
            <span
              class="war-dd__row-text"
              :class="{ current: i === modelValue }"
              :style="{ fontSize: textSize + 'px' }"
              >{{ opt }}</span
            >
          </div>
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
  border-width: 12px 46px 12px 29px; /* T R B L (slice 12/46/12/29) */
  border-image: url('/assets/ui/dropdown/dropdown_bar.png') 12 46 12 29 fill stretch;
  /* Solid navy fallback: WebView2 does not always paint the border-image
     center slice (fill), leaving the middle see-through without this. */
  background: #060d33;
  box-sizing: border-box;
  pointer-events: none;
}

.war-dd__bar-text {
  position: absolute;
  left: 12px;
  right: 44px; /* clear of the baked-in gold arrow cap */
  top: 50%;
  transform: translateY(-50%);
  font-family: SimSun, serif;
  font-weight: bold;
  color: var(--war-gold);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.war-dd__pop {
  position: fixed; /* coords come from popStyle (teleported to body) */
  z-index: 2000;
  border-style: solid;
  border-color: transparent;
  border-width: 14px 16px 13px 20px; /* T R B L (slice 14/16/13/20) */
  border-image: url('/assets/ui/dropdown/dropdown_panel.png') 14 16 13 20 fill stretch;
  background: #060d33; /* same fill fallback as the bar (center slice may not paint) */
  box-sizing: border-box;
}

.war-dd__pop-inner {
  padding: 10px 0;
  background: transparent;
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

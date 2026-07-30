<script setup lang="ts">
// WC3-style dropdown (WarDropdown.qml): dropdown_bar.png closed state (gold
// arrow baked into the right cap), dropdown_panel.png nine-slice list.
// Click the bar to toggle; selecting an option closes and emits.
// dropUp opens the list above the bar (for bars near the window bottom).
import { computed, onBeforeUnmount, ref, watch } from 'vue';

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

function onDocDown(e: MouseEvent): void {
  if (root.value && !root.value.contains(e.target as Node)) open.value = false;
}

watch(open, (v) => {
  if (v) document.addEventListener('mousedown', onDocDown, true);
  else document.removeEventListener('mousedown', onDocDown, true);
});
onBeforeUnmount(() => document.removeEventListener('mousedown', onDocDown, true));
</script>

<template>
  <div ref="root" class="war-dd">
    <!-- closed-state bar (border layer + label that spans the full width) -->
    <div class="war-dd__bar" @click="toggle">
      <div class="war-dd__bar-frame"></div>
      <span class="war-dd__bar-text" :style="{ fontSize: textSize + 'px' }">{{ shownText }}</span>
    </div>

    <!-- expanded list -->
    <div
      v-if="open"
      class="war-dd__pop"
      :class="{ 'war-dd__pop--up': dropUp }"
    >
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
  border: 12px 46px 12px 29px solid transparent; /* T R B L (slice 12/46/12/29) */
  border-image: url('/assets/ui/dropdown/dropdown_bar.png') 12 46 12 29 stretch;
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
  position: absolute;
  left: 0;
  top: calc(100% + 2px);
  width: max(100%, 120px);
  z-index: 60;
  border: 14px 16px 13px 20px solid transparent; /* T R B L (slice 14/16/13/20) */
  border-image: url('/assets/ui/dropdown/dropdown_panel.png') 14 16 13 20 stretch;
  box-sizing: border-box;
}

.war-dd__pop--up {
  top: auto;
  bottom: calc(100% + 2px);
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

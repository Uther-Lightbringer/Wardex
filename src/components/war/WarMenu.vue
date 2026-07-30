<script setup lang="ts">
// Context menu skinned with the dropdown expanded-panel nine-slice
// (WarMenu.qml). Min width 160, item height 28; highlighted item gold+bold
// over the KeyboardHighlight glow, disabled items #5a6272.
import { onBeforeUnmount, watch } from 'vue';

export interface WarMenuItem {
  label: string;
  disabled?: boolean;
}

const props = withDefaults(
  defineProps<{
    visible: boolean;
    x?: number; // client coords
    y?: number;
    items: WarMenuItem[];
  }>(),
  { x: 0, y: 0 },
);

const emit = defineEmits<{
  (e: 'update:visible', v: boolean): void;
  (e: 'select', index: number): void;
}>();

function pick(i: number): void {
  if (props.items[i]?.disabled) return;
  emit('update:visible', false);
  emit('select', i);
}

function onDocDown(): void {
  emit('update:visible', false);
}

watch(
  () => props.visible,
  (v) => {
    // Register after the opening click has finished bubbling.
    if (v) setTimeout(() => document.addEventListener('mousedown', onDocDown, true), 0);
    else document.removeEventListener('mousedown', onDocDown, true);
  },
);
onBeforeUnmount(() => document.removeEventListener('mousedown', onDocDown, true));
</script>

<template>
  <Teleport to="body">
    <div
      v-if="visible"
      class="war-menu"
      :style="{ left: x + 'px', top: y + 'px' }"
      @mousedown.stop
    >
      <div class="war-menu__inner">
        <div
          v-for="(item, i) in items"
          :key="i"
          class="war-menu__item"
          :class="{ disabled: item.disabled }"
          @click="pick(i)"
        >
          <span class="war-menu__glow"></span>
          <span class="war-menu__text">{{ item.label }}</span>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.war-menu {
  position: fixed;
  z-index: 120;
  min-width: 160px;
  border: 14px 16px 13px 20px solid transparent; /* T R B L (slice 14/16/13/20) */
  border-image: url('/assets/ui/dropdown/dropdown_panel.png') 14 16 13 20 stretch;
  box-sizing: border-box;
}

.war-menu__inner {
  padding: 10px 6px;
}

.war-menu__item {
  position: relative;
  height: 28px;
  display: flex;
  align-items: center;
}

.war-menu__glow {
  position: absolute;
  inset: 0;
  background: url('/assets/wc3_extracted/ui/GlueScreen-Button-KeyboardHighlight.png') 0 0 / 100% 100% no-repeat;
  mix-blend-mode: screen;
  opacity: 0;
  pointer-events: none;
}

.war-menu__item:hover:not(.disabled) .war-menu__glow {
  opacity: 1;
}

.war-menu__text {
  position: relative;
  padding-left: 8px;
  font-family: SimSun, serif;
  font-size: 13px;
  color: var(--war-text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.war-menu__item:hover:not(.disabled) .war-menu__text {
  color: var(--war-gold);
  font-weight: bold;
}

.war-menu__item.disabled .war-menu__text {
  color: var(--war-text-faint);
}
</style>

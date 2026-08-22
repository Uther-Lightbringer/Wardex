<script setup lang="ts">
// Context menu skinned with the dropdown expanded-panel nine-slice
// (WarMenu.qml). Min width 160, item height 28; highlighted item gold+bold
// over the KeyboardHighlight glow, disabled items #5a6272.
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { usePrefsStore } from '../../stores/prefs';
import { themeOf } from '../../lib/themes';

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

const prefs = usePrefsStore();
/** pure 风格：浅色 CSS 菜单（不贴图）。 */
const plain = computed(() => themeOf(prefs.uiStyle).kind === 'plain');

const root = ref<HTMLElement | null>(null);

function pick(i: number): void {
  if (props.items[i]?.disabled) return;
  emit('update:visible', false);
  emit('select', i);
}

function onDocDown(e: MouseEvent): void {
  // Clicks inside the menu must not count as "outside" — the document
  // listener is capture-phase, so it runs BEFORE the row's click and would
  // otherwise unmount the menu and swallow every selection.
  if (root.value?.contains(e.target as Node)) return;
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
      ref="root"
      class="war-menu"
      :class="{ 'is-plain': plain }"
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
  border-style: solid;
  border-color: transparent;
  border-width: 13px 14px 12px 14px; /* T R B L (slice 21/23/20/23) */
  border-image: url('/assets/ui/dropdown/dropdown_panel2.png') 21 23 20 23 fill stretch;
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

/* ---- pure 风格：浅色 CSS 菜单 ---- */
.war-menu.is-plain {
  background: var(--war-dd-pop-bg, #ffffff);
  border: 1px solid var(--war-dd-border, #c8cfd9);
  border-radius: 8px;
  box-shadow: 0 6px 20px rgba(15, 23, 42, 0.12);
}

.war-menu.is-plain .war-menu__inner {
  padding: 6px;
}

.war-menu.is-plain .war-menu__item {
  border-radius: 4px;
}

.war-menu.is-plain .war-menu__item:hover:not(.disabled) {
  background: var(--war-row-hover, #eef2f7);
}

.war-menu.is-plain .war-menu__glow {
  display: none;
}

.war-menu.is-plain .war-menu__text {
  color: var(--war-dd-text, #2a313c);
}

.war-menu.is-plain .war-menu__item:hover:not(.disabled) .war-menu__text {
  color: var(--war-gold);
  font-weight: bold;
}

.war-menu__item.disabled .war-menu__text {
  color: var(--war-text-faint);
}
</style>

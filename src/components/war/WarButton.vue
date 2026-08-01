<script setup lang="ts">
// Warcraft-menu style button (WarButton.qml). Three skins:
//   menu   — btn_normal/hover/pressed, artAspect 4.87 (default)
//   dialog — dialog_btn_*, artAspect 5.34
//   blue   — btn_blue_* (blue center strip only, no metal frame), artAspect 6.08
// Height is ALWAYS derived from width (width / artAspect) — never set from
// outside. Disabled or uiGate-busy: opacity 0.38, no hover/click/shortcut.
// Click plays the `click` SFX. Three stacked <img> swap on hover/pressed
// (no CSS filter simulation).
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { play } from '../../lib/sfx';
import { useUiStore } from '../../stores/ui';

const props = withDefaults(
  defineProps<{
    text: string;
    width?: number; // canonical menu width 276
    artAspect?: number;
    skin?: 'menu' | 'dialog' | 'blue';
    enabled?: boolean;
    /** Single-letter shortcut (main menu O/C/L/S/T/A, page B/L/R...). */
    shortcutKey?: string;
    /** Gate for letter shortcuts (e.g. only while this page is visible). */
    shortcutActive?: boolean;
  }>(),
  { width: 276, artAspect: 4.87, skin: 'menu', enabled: true, shortcutKey: '', shortcutActive: true },
);

const emit = defineEmits<{ (e: 'activated'): void }>();

const ui = useUiStore();
const hover = ref(false);
const pressed = ref(false);

const height = computed(() => Math.round(props.width / props.artAspect));
const labelSize = computed(() => Math.max(13, Math.min(19, Math.round(props.width * 0.075))));
const disabled = computed(() => !props.enabled || ui.busy);

const srcPrefix = computed(() =>
  props.skin === 'dialog' ? 'dialog_btn' : props.skin === 'blue' ? 'btn_blue' : 'btn',
);
const srcNormal = computed(() => `/assets/ui/buttons/${srcPrefix.value}_normal.png`);
const srcHover = computed(() => `/assets/ui/buttons/${srcPrefix.value}_hover.png`);
const srcPressed = computed(() => `/assets/ui/buttons/${srcPrefix.value}_pressed.png`);
const currentSrc = computed(() => {
  if (!disabled.value && pressed.value) return srcPressed.value;
  if (!disabled.value && hover.value) return srcHover.value;
  return srcNormal.value;
});

function trigger(): void {
  if (disabled.value) return;
  play('click');
  emit('activated');
}

function onKey(e: KeyboardEvent): void {
  if (!props.shortcutKey || !props.shortcutActive || disabled.value) return;
  if (e.ctrlKey || e.altKey || e.metaKey) return;
  const t = e.target as HTMLElement | null;
  if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA')) return;
  if (e.key.toUpperCase() === props.shortcutKey.toUpperCase()) trigger();
}

onMounted(() => window.addEventListener('keydown', onKey));
onBeforeUnmount(() => window.removeEventListener('keydown', onKey));
</script>

<template>
  <div
    class="war-btn"
    :class="{ disabled }"
    :style="{ width: width + 'px', height: height + 'px' }"
    @click="trigger"
    @mouseenter="hover = true"
    @mouseleave="hover = false; pressed = false"
    @mousedown="pressed = true"
    @mouseup="pressed = false"
  >
    <!-- blue skin: fill (not contain) so the strip can match an exact
         width×height box (e.g. the composer's 150x30 dropdown row) -->
    <img
      :src="currentSrc"
      :alt="text"
      draggable="false"
      :style="{ objectFit: skin === 'blue' ? 'fill' : 'contain' }"
    />
    <span class="war-btn__label" :style="{ fontSize: labelSize + 'px' }">{{ text }}</span>
  </div>
</template>

<style scoped>
.war-btn {
  position: relative;
  user-select: none;
}

.war-btn img {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.war-btn.disabled {
  opacity: 0.38;
  pointer-events: none;
}

.war-btn__label {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0 6%;
  font-family: SimSun, serif;
  font-weight: bold;
  color: var(--war-gold);
  text-shadow:
    -1px 0 var(--war-outline-brown), 1px 0 var(--war-outline-brown),
    0 -1px var(--war-outline-brown), 0 1px var(--war-outline-brown);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.war-btn:active .war-btn__label {
  color: #fff;
}

.war-btn.disabled .war-btn__label {
  color: #7a8070;
}
</style>

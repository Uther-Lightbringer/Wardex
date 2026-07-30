<script setup lang="ts">
// Chain-hung riveted steel panel, Warcraft-menu style (SteelPanel.qml).
// frame_tall/frame_short stretched whole (NOT nine-slice) + chain_link.png
// tiled vertically above, running off the window top when chainExtra is set.
// Fractions measured from the sprites (frame keeps the art's aspect ratio):
//   tall:  ratio 1000/835, insetX .071, insetTop .108, insetBottom .059
//   short: ratio  500/835, insetX .066, insetTop .214, insetBottom .081
// The component's box IS the frame; chains overflow above it — callers leave
// chainHeight (+ extras) of headroom via margin/padding.
import { computed } from 'vue';

const props = withDefaults(
  defineProps<{
    title?: string;
    chainHeight?: number;
    chainExtra?: number; // extra reach so chains run off the window top
    chainOverlapUp?: number; // plug chain tops into a panel hanging above
    variant?: 'tall' | 'short';
  }>(),
  { title: '', chainHeight: 96, chainExtra: 0, chainOverlapUp: 0, variant: 'tall' },
);

const ratio = computed(() => (props.variant === 'short' ? 500 / 835 : 1000 / 835));
const frameSrc = computed(() =>
  props.variant === 'short'
    ? '/assets/ui/frames/frame_short.png'
    : '/assets/ui/frames/frame_tall.png',
);
const insetX = computed(() => (props.variant === 'short' ? 0.066 : 0.071));
const insetTop = computed(() => (props.variant === 'short' ? 0.214 : 0.108));
const insetBottom = computed(() => (props.variant === 'short' ? 0.081 : 0.059));

const chainsStyle = computed(() => ({
  height: `${props.chainHeight + 22 + props.chainExtra + props.chainOverlapUp}px`,
}));

const glassStyle = computed(() => ({
  left: `${insetX.value * 100}%`,
  right: `${insetX.value * 100}%`,
  top: `${insetTop.value * 100}%`,
  bottom: `${insetBottom.value * 100}%`,
}));

const innerStyle = computed(() => ({
  left: `calc(${insetX.value * 100}% + 8px)`,
  right: `calc(${insetX.value * 100}% + 8px)`,
  top: `calc(${insetTop.value * 100}% + ${props.title ? 40 : 8}px)`,
  bottom: `calc(${insetBottom.value * 100}% + 8px)`,
}));
</script>

<template>
  <div class="steel">
    <!-- two chains, x-centers match the anchor plates in the frame art -->
    <div v-if="chainHeight > 0" class="steel__chains" :style="chainsStyle">
      <div class="steel__chain" style="left: calc(23.05% - 9.3%)"></div>
      <div class="steel__chain" style="left: calc(65.8% - 9.3%)"></div>
    </div>

    <img class="steel__frame" :src="frameSrc" :style="{ aspectRatio: String(1 / ratio) }" draggable="false" />

    <!-- dark recessed glass (frame interior is transparent) -->
    <div class="steel__glass" :style="glassStyle"></div>

    <div v-if="title" class="steel__title war-font-title war-outline-black">{{ title }}</div>

    <div class="steel__inner" :class="{ 'steel__inner--center': !title }" :style="innerStyle">
      <slot />
    </div>
  </div>
</template>

<style scoped>
.steel {
  position: relative;
}

.steel__chains {
  position: absolute;
  left: 0;
  right: 0;
  bottom: calc(100% - 22px); /* chain bottoms overlap the frame top by 22px */
  z-index: 2;
  pointer-events: none;
}

.steel__chain {
  position: absolute;
  bottom: 0;
  width: 18.6%; /* panel width × 0.186 */
  height: 100%;
  background: url('/assets/ui/misc/chain_link.png') 0 0 / 100% auto repeat-y;
}

.steel__frame {
  display: block;
  width: 100%;
}

.steel__glass {
  position: absolute;
  background: var(--war-glass);
  border: 1px solid #04050a;
}

/* The title sits 10px below the glass top edge (SteelPanel.qml). */
.steel__title {
  position: absolute;
  left: 0;
  right: 0;
  top: v-bind('`${insetTop * 100}%`');
  padding-top: 10px;
  text-align: center;
  color: var(--war-text-dim);
  font-size: 17px;
}

.steel__inner {
  position: absolute;
  display: flex;
  flex-direction: column;
  align-items: center;
}

.steel__inner--center {
  justify-content: center;
}
</style>

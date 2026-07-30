<script setup lang="ts">
// Generic nine-slice frame: Qt BorderImage → CSS border-image
// (docs/ui-design.md §2/§3). Slice order is [T, R, B, L] source pixels.
//
// Plain mode (no `hole`): border-image container, content sits inside the
// border band (center slice of these sprites is a transparent hole, so no
// `fill` keyword — visually identical to the Qt version).
//
// Hole mode (`hole` given): the FrameImage pixel-mode three-layer structure:
//   glass (z0, #0b0d12a6, tucked 8px under the rim so the ragged inner edge
//     never shows a gap)
//   iron  (z1, the border-image itself, pointer-events: none)
//   content (z2, inset = hole + 2px breathing gap, inner padding 10/8 +
//     contentLeftExtra / contentRightExtra)
// The iron is a separate absolutely-positioned layer (not a border on the
// root) so the glass can sit UNDER the rim — CSS paints a border-image above
// the element's own background but below positioned children otherwise.
import { computed, type CSSProperties } from 'vue';

const props = withDefaults(
  defineProps<{
    src: string; // /assets/... path
    slice: [number, number, number, number]; // [T, R, B, L] source pixels
    hole?: [number, number, number, number]; // [T, R, B, L] source-pixel rim inner edge
    /** Plain-mode content inset override (px). Defaults to `slice`; some
     * panels deliberately let content overlap the border's fade-out band
     * (e.g. frame_popup_small: gold rim inner edge is shallower than the
     * nine-slice cut). Ignored in hole mode. */
    inset?: [number, number, number, number];
    repeat?: 'stretch' | 'repeat';
    contentLeftExtra?: number;
    contentRightExtra?: number;
  }>(),
  { repeat: 'stretch', hole: undefined, inset: undefined, contentLeftExtra: 0, contentRightExtra: 0 },
);

// FrameImage.qml constants
const FILL_TUCK = 8; // how far the glass tucks under the rim (< rim thickness)
const CONTENT_GAP = 2; // breathing gap between rim inner edge and content clip

const ironStyle = computed<CSSProperties>(() => ({
  borderStyle: 'solid',
  borderColor: 'transparent',
  borderWidth: props.slice.map((v) => `${v}px`).join(' '),
  borderImageSource: `url('${props.src}')`,
  borderImageSlice: props.slice.join(' '),
  borderImageRepeat: props.repeat,
  boxSizing: 'border-box',
}));

const glassStyle = computed(() => {
  if (!props.hole) return {};
  const [t, r, b, l] = props.hole;
  return {
    inset: `${Math.max(2, t - FILL_TUCK)}px ${Math.max(2, r - FILL_TUCK)}px ${Math.max(
      2,
      b - FILL_TUCK,
    )}px ${Math.max(2, l - FILL_TUCK)}px`,
  };
});

const contentStyle = computed(() => {
  if (!props.hole) {
    const [t, r, b, l] = props.inset ?? props.slice;
    return { inset: `${t}px ${r}px ${b}px ${l}px` };
  }
  const [t, r, b, l] = props.hole;
  return {
    inset: `${t + CONTENT_GAP}px ${r + CONTENT_GAP}px ${b + CONTENT_GAP}px ${l + CONTENT_GAP}px`,
    padding: `8px ${10 + props.contentRightExtra}px 8px ${10 + props.contentLeftExtra}px`,
  };
});
</script>

<template>
  <div class="war-frame">
    <div v-if="hole" class="war-frame__glass" :style="glassStyle"></div>
    <div class="war-frame__iron" :style="ironStyle"></div>
    <div class="war-frame__content" :style="contentStyle"><slot /></div>
  </div>
</template>

<style scoped>
.war-frame {
  position: relative;
}

.war-frame__glass {
  position: absolute;
  z-index: 0;
  background: var(--war-glass);
  border-radius: 2px;
}

.war-frame__iron {
  position: absolute;
  inset: 0;
  z-index: 1;
  pointer-events: none;
}

.war-frame__content {
  position: absolute;
  z-index: 2;
  overflow: hidden;
  border-radius: 2px;
}
</style>

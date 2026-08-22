<script setup lang="ts">
// Generic nine-slice frame: Qt BorderImage → CSS border-image
// (docs/ui-design.md §2/§3). Slice order is [T, R, B, L] source pixels.
//
// Plain mode (no `hole`): border-image container, content sits inside the
// border band (most sprites' center slice is a transparent hole, so no
// `fill` keyword — pass `fill` only when the sprite's center is painted).
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
import { usePrefsStore } from '../../stores/prefs';
import { themeOf } from '../../lib/themes';

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
    /** Add the `fill` keyword to border-image-slice: keep the center slice
     * as the backdrop instead of discarding it. Only for sprites whose
     * center is painted (e.g. frame_popup_small's dark navy texture);
     * most sprites have a transparent hole there. */
    fill?: boolean;
    /** Height hugs content instead of filling the parent: content becomes
     * position:relative so the root (and the iron/glass riding inset:0)
     * sizes to it. For menu-like pages (Hub) that must not span the screen. */
    hug?: boolean;
    contentLeftExtra?: number;
    contentRightExtra?: number;
  }>(),
  {
    repeat: 'stretch',
    hole: undefined,
    inset: undefined,
    fill: false,
    hug: false,
    contentLeftExtra: 0,
    contentRightExtra: 0,
  },
);

const prefs = usePrefsStore();
/** pure 风格：不贴图，渲染纯 CSS 卡片（白底/浅色半透明 + 细边框 + 圆角）。 */
const plain = computed(() => themeOf(prefs.uiStyle).kind === 'plain');

// FrameImage.qml constants
const FILL_TUCK = 8; // how far the glass tucks under the rim (< rim thickness)
const CONTENT_GAP = 2; // breathing gap between rim inner edge and content clip

const ironStyle = computed<CSSProperties>(() => ({
  borderStyle: 'solid',
  borderColor: 'transparent',
  borderWidth: props.slice.map((v) => `${v}px`).join(' '),
  borderImageSource: `url('${props.src}')`,
  borderImageSlice: props.slice.join(' ') + (props.fill ? ' fill' : ''),
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

const contentStyle = computed<CSSProperties>(() => {
  // hug: content goes relative and pushes the root open; the hole/inset
  // values become margins so it still sits inside the rim opening.
  if (props.hug) {
    const [t, r, b, l] = props.hole ?? props.inset ?? props.slice;
    const gap = props.hole ? CONTENT_GAP : 0;
    return {
      position: 'relative',
      margin: `${t + gap}px ${r + gap}px ${b + gap}px ${l + gap}px`,
      padding: props.hole
        ? `8px ${10 + props.contentRightExtra}px 8px ${10 + props.contentLeftExtra}px`
        : undefined,
    };
  }
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
  <div class="war-frame" :class="{ 'is-plain': plain, 'is-hug': hug }">
    <template v-if="!plain">
      <div v-if="hole" class="war-frame__glass" :style="glassStyle"></div>
      <div class="war-frame__iron" :style="ironStyle"></div>
    </template>
    <div class="war-frame__content" :class="{ 'is-plain': plain }" :style="plain ? undefined : contentStyle"><slot /></div>
  </div>
</template>

<style scoped>
.war-frame {
  position: relative;
}

.war-frame.is-plain {
  background: var(--war-panel-bg, #ffffff);
  border: 1px solid var(--war-panel-border, #d4dae2);
  border-radius: 10px;
  box-shadow: 0 2px 10px var(--war-panel-shadow, rgba(15, 23, 42, 0.06));
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

.war-frame.is-plain .war-frame__content {
  position: absolute;
  inset: 0;
  overflow: hidden;
  border-radius: 8px;
  padding: 10px 12px;
}

/* plain + hug：内容转 relative，卡片高度由内容撑开 */
.war-frame.is-plain.is-hug .war-frame__content {
  position: relative;
}
</style>

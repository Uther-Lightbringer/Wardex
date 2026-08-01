<script setup lang="ts">
// WC3-style vertical scrollbar (WarScrollBar.qml): scroll_up/down arrows
// (22×22), stretched track, 9-slice thumb (fixed end caps, stretched flat
// middle so the art never distorts). When the content fits, the track+thumb
// hide and only the dimmed arrows remain — no more WC3 blackout bar.
//
// Unlike the Qt attached ScrollBar, the web version is given the scrollable
// element via the `target` prop (a plain div with overflow-y: auto +
// scrollbar-width: none).
import { computed, onBeforeUnmount, ref, watch } from 'vue';

const props = defineProps<{
  target: HTMLElement | null;
}>();

const scrollTop = ref(0);
const clientH = ref(0);
const scrollH = ref(0);

const size = computed(() => (scrollH.value > 0 ? clientH.value / scrollH.value : 0));
const scrollable = computed(() => size.value > 0 && size.value < 0.999);

const trackH = ref(0);
const barEl = ref<HTMLElement | null>(null);

const MIN_THUMB = 0.08; // ScrollBar minimumSize
const MIN_THUMB_PX = 40; // 9-slice caps are 18px top + 18px bottom — never go shorter

const thumbH = computed(() => {
  if (!scrollable.value || trackH.value <= 0) return 0;
  return Math.min(trackH.value, Math.max(MIN_THUMB_PX, Math.max(MIN_THUMB, size.value) * trackH.value));
});
const thumbY = computed(() => {
  if (!scrollable.value || scrollH.value <= clientH.value) return 0;
  const range = scrollH.value - clientH.value;
  return (scrollTop.value / range) * (trackH.value - thumbH.value);
});

let ro: ResizeObserver | null = null;
let mo: MutationObserver | null = null;

function refresh(): void {
  const t = props.target;
  if (!t) return;
  scrollTop.value = t.scrollTop;
  clientH.value = t.clientHeight;
  scrollH.value = t.scrollHeight;
}

function onScroll(): void {
  if (props.target) scrollTop.value = props.target.scrollTop;
}

watch(
  () => props.target,
  (t, old) => {
    old?.removeEventListener('scroll', onScroll);
    ro?.disconnect();
    mo?.disconnect();
    if (t) {
      t.addEventListener('scroll', onScroll, { passive: true });
      ro = new ResizeObserver(refresh);
      ro.observe(t);
      mo = new MutationObserver(refresh);
      mo.observe(t, { childList: true, subtree: true, characterData: true });
      refresh();
    }
  },
  { immediate: true },
);

watch(barEl, (el) => {
  if (!el) return;
  const r = new ResizeObserver(() => {
    trackH.value = el.clientHeight - 44; // arrows take 22 top + 22 bottom
  });
  r.observe(el);
});

onBeforeUnmount(() => {
  props.target?.removeEventListener('scroll', onScroll);
  ro?.disconnect();
  mo?.disconnect();
});

function step(dir: number): void {
  if (!scrollable.value || !props.target) return;
  props.target.scrollTop += dir * 60;
}

// ---- thumb dragging ----
let dragStartY = 0;
let dragStartScroll = 0;

function onThumbDown(e: PointerEvent): void {
  if (!scrollable.value || !props.target) return;
  dragStartY = e.clientY;
  dragStartScroll = props.target.scrollTop;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onThumbMove(e: PointerEvent): void {
  if (!scrollable.value || !props.target || dragStartY === 0) return;
  if (!(e.buttons & 1)) {
    dragStartY = 0;
    return;
  }
  const usable = trackH.value - thumbH.value;
  if (usable <= 0) return;
  const range = scrollH.value - clientH.value;
  props.target.scrollTop = dragStartScroll + ((e.clientY - dragStartY) / usable) * range;
}
</script>

<template>
  <div ref="barEl" class="war-scroll" :class="{ off: !scrollable }">
    <img
      class="war-scroll__arrow war-scroll__arrow--up"
      src="/assets/ui/scroll/scroll_up.png"
      draggable="false"
      @click="step(-1)"
    />
    <div class="war-scroll__track">
      <div
        v-if="scrollable"
        class="war-scroll__thumb"
        :style="{ height: thumbH + 'px', top: thumbY + 'px' }"
        @pointerdown="onThumbDown"
        @pointermove="onThumbMove"
      ></div>
    </div>
    <img
      class="war-scroll__arrow war-scroll__arrow--down"
      src="/assets/ui/scroll/scroll_down.png"
      draggable="false"
      @click="step(1)"
    />
  </div>
</template>

<style scoped>
.war-scroll {
  position: relative;
  width: 22px;
  height: 100%;
}

.war-scroll__arrow {
  position: absolute;
  left: 0;
  width: 22px;
  height: 22px;
}

.war-scroll__arrow--up {
  top: 0;
}

.war-scroll__arrow--down {
  bottom: 0;
}

.war-scroll__arrow:active {
  transform: scale(0.92);
}

.war-scroll.off .war-scroll__arrow {
  pointer-events: none;
}

.war-scroll__track {
  position: absolute;
  inset: 22px 3px; /* arrows vertically, 3px side insets */
  background: url('/assets/ui/scroll/scroll_track.png') 0 0 / 100% 100% no-repeat;
}

/* 内容装得下 → 隐藏轨道（只留一对置灰箭头占位，不再盖黑条） */
.war-scroll.off .war-scroll__track {
  visibility: hidden;
}

.war-scroll.off .war-scroll__arrow {
  opacity: 0.35;
}

/* thumb: 9-slice (18px caps + 14px sides fixed, flat middle stretches) so the
   end caps and rounded corners never distort no matter how long it gets */
.war-scroll__thumb {
  position: absolute;
  left: -2px; /* thumb art (68px wide source) slightly overlaps the track */
  width: calc(100% + 4px);
  box-sizing: border-box;
  border-style: solid;
  border-color: transparent;
  border-width: 18px 14px;
  border-image: url('/assets/ui/scroll/scroll_thumb.png') 18 14 fill stretch;
  user-select: none;
  touch-action: none;
}
</style>

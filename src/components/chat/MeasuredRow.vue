<script setup lang="ts">
// Windowing row wrapper: reports its rendered height to the virtual list via
// ResizeObserver. This is also what keeps scroll-follow pinned during
// streaming — direct DOM appends (R1) change the height without any Vue
// render, but the observer still fires.
import { onBeforeUnmount, onMounted, ref } from 'vue';

const props = defineProps<{ rowId: string }>();
const emit = defineEmits<{ (e: 'measure', id: string, h: number): void }>();

const el = ref<HTMLElement | null>(null);
let ro: ResizeObserver | null = null;

onMounted(() => {
  if (!el.value) return;
  ro = new ResizeObserver(() => {
    if (el.value) emit('measure', props.rowId, el.value!.offsetHeight);
  });
  ro.observe(el.value);
  emit('measure', props.rowId, el.value.offsetHeight);
});
onBeforeUnmount(() => ro?.disconnect());
</script>

<template>
  <div ref="el"><slot /></div>
</template>

<script setup lang="ts">
// Overlay page shell (ShellFrame.qml equivalent): the content band is
// embedded between the permanent window rails and rides nav.contentY —
// parked above the viewport before becoming visible, dropped in 750ms by the
// nav state machine. embed = how far panels tuck under the rails
// (ChatPage 34, Config/SessionSelect/Todo 52, default 50).
import { computed } from 'vue';
import { useNavStore } from '../stores/nav';

const props = withDefaults(defineProps<{ embed?: number; edgeW?: number }>(), {
  embed: 50,
  edgeW: 58,
});

const nav = useNavStore();
const contentLeft = computed(() => Math.max(0, props.edgeW - props.embed));
</script>

<template>
  <div class="page-shell">
    <div
      class="page-shell__band"
      :style="{
        left: contentLeft + 'px',
        right: contentLeft + 'px',
        transform: `translateY(${nav.contentY}px)`,
      }"
    >
      <slot />
    </div>
  </div>
</template>

<style scoped>
.page-shell {
  position: absolute;
  inset: 0;
}

.page-shell__band {
  position: absolute;
  top: 0;
  bottom: 0;
}
</style>

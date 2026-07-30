// Minimal element-size composable (ResizeObserver). Used where the old QML
// sized controls from their container (e.g. ChatPage action-bay buttons).
import { onBeforeUnmount, onMounted, ref, type Ref } from 'vue';

export function useElementSize(el: Ref<HTMLElement | null>) {
  const width = ref(0);
  const height = ref(0);
  let ro: ResizeObserver | null = null;

  onMounted(() => {
    if (!el.value) return;
    ro = new ResizeObserver(() => {
      width.value = el.value?.clientWidth ?? 0;
      height.value = el.value?.clientHeight ?? 0;
    });
    ro.observe(el.value);
  });
  onBeforeUnmount(() => ro?.disconnect());

  return { width, height };
}

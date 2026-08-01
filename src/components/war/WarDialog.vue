<script setup lang="ts">
// Modal dialog chrome from dialog_frame.png (WarDialog.qml).
// Layout fractions measured against the frame art (863×602, ≈1.43:1):
//   upper black gold-rim plate: x=12% y=14.5% w=76% h=35.5% (title + message)
//   lower blue button zone:     x=10% y=buttonZoneY(0.56) w=80% h=buttonZoneH(0.30)
// Mask: full-screen #000000b0. Esc closes. Web CSS cursor covers the old
// per-item cursor re-stamping automatically.
import { computed, onBeforeUnmount, watch } from 'vue';

const props = withDefaults(
  defineProps<{
    open: boolean;
    titleText?: string;
    messageText?: string;
    dialogWidth?: number; // capped at 90% of the window width
    buttonZoneY?: number;
    buttonZoneH?: number;
    /** ACP permission requests: Esc must NOT dismiss (closePolicy NoAutoClose). */
    noAutoClose?: boolean;
  }>(),
  { titleText: '', messageText: '', dialogWidth: 560, buttonZoneY: 0.56, buttonZoneH: 0.30, noAutoClose: false },
);

const emit = defineEmits<{ (e: 'update:open', v: boolean): void }>();

const FRAME_ASPECT = 863 / 602; // ≈1.4333

const dlgStyle = computed(() => ({
  width: `min(${props.dialogWidth}px, 90vw)`,
  aspectRatio: `${FRAME_ASPECT}`,
}));

// Title/message sizes derive from the plate height (= 35.5% of dialog height
// = 35.5% of width/1.4333). Factors folded: 0.355/1.4333*0.11 ≈ 0.02724,
// 0.355/1.4333*0.09 ≈ 0.02230.
const plateVars = computed(() => ({
  '--dlg-w': `min(${props.dialogWidth}px, 90vw)`,
}));

const zoneStyle = computed(() => ({
  top: `${props.buttonZoneY * 100}%`,
  height: `${props.buttonZoneH * 100}%`,
}));

function close(): void {
  emit('update:open', false);
}

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.stopPropagation();
    if (!props.noAutoClose) close();
  }
}

watch(
  () => props.open,
  (v) => {
    if (v) window.addEventListener('keydown', onKey, true);
    else window.removeEventListener('keydown', onKey, true);
  },
);
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true));
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="war-dialog-mask">
      <div class="war-dialog" :style="dlgStyle">
        <div class="war-dialog__frame"></div>

        <!-- Upper: title + message inside the art's black gold plate -->
        <div class="war-dialog__plate" :style="plateVars">
          <div class="war-dialog__plate-col">
            <div v-if="titleText" class="war-dialog__title">{{ titleText }}</div>
            <div v-if="messageText" class="war-dialog__msg">{{ messageText }}</div>
            <slot name="plate" />
          </div>
        </div>

        <!-- Lower: action buttons -->
        <div class="war-dialog__zone" :style="zoneStyle">
          <div class="war-dialog__zone-row"><slot /></div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.war-dialog-mask {
  position: fixed;
  inset: 0;
  z-index: 100;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.war-dialog {
  position: relative;
}

.war-dialog__frame {
  position: absolute;
  inset: 0;
  border: 56px solid transparent;
  border-image: url('/assets/ui/frames/dialog_frame.png') 56 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.war-dialog__plate {
  position: absolute;
  left: 12%;
  top: 14.5%;
  width: 76%;
  height: 35.5%;
  display: flex;
  align-items: center;
  justify-content: center;
}

.war-dialog__plate-col {
  width: calc(100% - 28px);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
}

.war-dialog__title {
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-weight: bold;
  font-size: max(15px, calc(var(--dlg-w) * 0.02724));
  text-align: center;
  text-shadow:
    -1px 0 var(--war-outline-dark), 1px 0 var(--war-outline-dark),
    0 -1px var(--war-outline-dark), 0 1px var(--war-outline-dark);
  overflow-wrap: break-word;
}

.war-dialog__msg {
  color: var(--war-text);
  font-family: SimSun, serif;
  font-size: max(13px, calc(var(--dlg-w) * 0.0223));
  text-align: center;
  white-space: pre-line;
  overflow-wrap: break-word;
  display: -webkit-box;
  -webkit-line-clamp: 6;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.war-dialog__zone {
  position: absolute;
  left: 10%;
  width: 80%;
  display: flex;
  align-items: center;
  justify-content: center;
}

.war-dialog__zone-row {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 16px;
  max-width: 100%;
}
</style>

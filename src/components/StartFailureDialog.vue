<script setup lang="ts">
// Agent 启动失败弹框 (`wardex://agentStartFailed`)。
//
// 只有后台会话才会走到这里：活跃会话的失败会出现在它自己的聊天气泡里
// （`chat://status` 按 sessionId 过滤，非活跃会话的状态前端直接丢弃）。
// 启动预热、项目待办会话、监控小窗、插件重放都可能在后端无人围观时 spawn
// 失败，静默失败等于「WarDex 启动了但 agent 是死的」而用户毫不知情。
//
// 视觉上复用「打开项目」弹框的语言：title plate + frame_popup 九宫格 + dialog
// 按钮皮；pure 风格走浅色 CSS 面板（themeOf + is-plain 覆盖）。
import { computed } from 'vue';
import WarButton from './war/WarButton.vue';
import { themeOf } from '../lib/themes';
import { usePrefsStore } from '../stores/prefs';
import type { StartFailure } from '../stores/ui';

const prefs = usePrefsStore();
/** 纯净风格：去 WC3 贴图框，走浅色 CSS 面板（WarButton 自带 plain 路径）。 */
const plain = computed(() => themeOf(prefs.uiStyle).kind === 'plain');

const props = defineProps<{ failure: StartFailure | null }>();
const emit = defineEmits<{
  (e: 'openSession', sessionId: string): void;
  (e: 'close'): void;
}>();

function openSession(): void {
  if (props.failure) emit('openSession', props.failure.sessionId);
}
</script>

<template>
  <Teleport to="body">
    <div v-if="failure" class="sf-mask" :class="{ 'is-plain': plain }" @keydown.esc.prevent="$emit('close')">
      <div class="sf" :class="{ 'is-plain': plain }">
        <div class="sf__title-plate">
          <span class="sf__title war-outline-gold">启动失败</span>
        </div>

        <div class="sf__body">
          <div class="sf__line">
            <span class="sf__k">会话</span>
            <span class="sf__v">{{ failure.sessionTitle || failure.sessionId }}</span>
          </div>
          <div class="sf__line">
            <span class="sf__k">Agent</span>
            <span class="sf__v">{{ failure.agentName }}</span>
          </div>
          <div class="sf__line" v-if="failure.projectDir">
            <span class="sf__k">项目</span>
            <span class="sf__v sf__v--path" :title="failure.projectDir">{{ failure.projectDir }}</span>
          </div>
          <div class="sf__frame">
            <div class="sf__frame-iron"></div>
            <div class="sf__err">{{ failure.error }}</div>
          </div>
          <div class="sf__hint">
            该会话在后台启动，这个错误不会显示在聊天窗口里。可在「Agent 配置 / Pi 插件」检查二进制后重试。
          </div>
        </div>

        <div class="sf__buttons">
          <WarButton
            skin="dialog"
            :width="180"
            :art-aspect="5.34"
            text="打开该会话"
            @activated="openSession()"
          />
          <WarButton skin="dialog" :width="180" :art-aspect="5.34" text="知道了" @activated="$emit('close')" />
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.sf-mask {
  position: fixed;
  inset: 0;
  z-index: 110;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.sf {
  width: min(560px, 86vw);
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.sf__title-plate {
  align-self: center;
  height: 44px;
  padding: 0 36px;
  display: flex;
  align-items: center;
  border-style: solid;
  border-color: transparent;
  border-width: 13px 14px 12px 14px;
  border-image: url('/assets/ui/dropdown/dropdown_panel2.png') 21 23 20 23 fill stretch;
  box-sizing: border-box;
}

.sf__title {
  color: var(--war-gold);
  font-size: 22px;
  font-weight: bold;
  font-family: SimSun, serif;
  letter-spacing: 6px;
}

.sf__body {
  padding: 14px 18px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.sf__line {
  display: flex;
  align-items: baseline;
  gap: 10px;
  min-width: 0;
}

.sf__k {
  flex: none;
  width: 44px;
  color: var(--war-text-faint);
  font-size: 13px;
  font-family: SimSun, serif;
}

.sf__v {
  flex: 1;
  min-width: 0;
  color: var(--war-text);
  font-size: 14px;
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sf__v--path {
  color: var(--war-text-muted);
  font-size: 13px;
  direction: rtl;
  text-align: left;
}

.sf__frame {
  position: relative;
  margin-top: 4px;
  min-height: 96px;
}

.sf__frame-iron {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 40px 34px 40px 34px;
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.sf__err {
  position: relative;
  padding: 12px 14px;
  max-height: 180px;
  overflow-y: auto;
  scrollbar-width: none;
  color: var(--war-error);
  font-size: 13px;
  font-family: SimSun, serif;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
}

.sf__hint {
  color: var(--war-text-faint);
  font-size: 12px;
  font-family: SimSun, serif;
  line-height: 1.6;
}

.sf__buttons {
  flex: none;
  display: flex;
  justify-content: center;
  gap: 16px;
}

/* ---- 纯净风格（plain）：去 WC3 贴图，浅色 CSS 面板 ---- */
.sf-mask.is-plain {
  background: rgba(15, 23, 42, 0.3);
}

.sf.is-plain {
  background: var(--war-dialog-bg);
  border: 1px solid var(--war-panel-border);
  border-radius: 12px;
  box-shadow: 0 12px 40px var(--war-panel-shadow);
  padding: 16px 20px;
  box-sizing: border-box;
}

.sf.is-plain .sf__title-plate {
  border: none;
  border-image: none;
  height: auto;
  padding: 0 0 4px;
}

.sf.is-plain .sf__title {
  font-family: inherit;
  font-size: 18px;
  letter-spacing: 3px;
}

.sf.is-plain .sf__frame-iron {
  display: none;
}

.sf.is-plain .sf__frame {
  background: var(--war-input-bg);
  border: 1px solid var(--war-panel-border);
  border-radius: 8px;
}

.sf.is-plain .sf__err {
  font-family: inherit;
}

.sf.is-plain .sf__v,
.sf.is-plain .sf__v--path,
.sf.is-plain .sf__k,
.sf.is-plain .sf__hint {
  font-family: inherit;
}

.sf.is-plain .sf__k {
  color: var(--war-text-muted);
}

.sf.is-plain .sf__v--path {
  direction: ltr;
  text-align: left;
}
</style>

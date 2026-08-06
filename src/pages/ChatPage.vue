<script setup lang="ts">
// Chat page (features/chat.md §1): far-left session rail, center chat panel
// (title row + message list), right info panel dock, bottom-left composer,
// bottom-right action bay. Floating widgets (attachment bar / send queue /
// sub-agent panel / rate-limit banner / back-to-bottom) overlay the center
// column at z≥26 and NEVER squeeze the composer height.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import PageShell from '../components/PageShell.vue';
import WarFrame from '../components/war/WarFrame.vue';
import WarButton from '../components/war/WarButton.vue';
import WarDock from '../components/war/WarDock.vue';
import WarDropdown from '../components/war/WarDropdown.vue';
import SessionRail from '../components/chat/SessionRail.vue';
import MessageList from '../components/chat/MessageList.vue';
import Composer from '../components/chat/Composer.vue';
import AttachmentBar from '../components/chat/AttachmentBar.vue';
import QuoteBar from '../components/chat/QuoteBar.vue';
import QueuePanel from '../components/chat/QueuePanel.vue';
import SubagentPanel from '../components/chat/SubagentPanel.vue';
import PermissionDialog from '../components/chat/PermissionDialog.vue';
import FilePreviewDialog from '../components/chat/FilePreviewDialog.vue';
import DueTodoOverlay from '../components/chat/DueTodoOverlay.vue';
import CodeSearchOverlay from '../components/chat/CodeSearchOverlay.vue';
import { useNavStore } from '../stores/nav';
import { usePrefsStore } from '../stores/prefs';
import { useChatStore } from '../stores/chat';
import { useSessionsStore } from '../stores/sessions';
import { useAgentsStore } from '../stores/agents';
import { useElementSize } from '../lib/useElementSize';
import { formatTokens } from '../lib/format';

const nav = useNavStore();
const prefs = usePrefsStore();
const chat = useChatStore();
const sessions = useSessionsStore();
const agentsStore = useAgentsStore();

onMounted(async () => {
  await chat.init();
  await sessions.refreshAgents();
  if (!agentsStore.loaded) void agentsStore.refresh();
  if (chat.projectDir) await sessions.refresh(chat.projectDir);
});

// Coming BACK to the chat page (kept-alive): re-pull the rail + agents so
// config-page edits (avatar/name/default) and background turn activity show
// up immediately (the old agentStore.revision bump equivalent).
watch(
  () => nav.page,
  (p) => {
    if (p !== 'chat') return;
    void sessions.refreshAgents();
    if (chat.projectDir) void sessions.refresh(chat.projectDir);
    void chat.refreshMeta();
  },
);

// ---- action-bay button sizing (same cap rule as the old ChatPage) ----
const actionBay = ref<HTMLElement | null>(null);
const { width: bayW } = useElementSize(actionBay);
const MENU_BTN_W = 276;
const actionBtnW = computed(() =>
  bayW.value > 0 ? Math.min(MENU_BTN_W, Math.floor(bayW.value * 0.98)) : MENU_BTN_W,
);

// ---- rail inline-rename ref: Esc belongs to the input while renaming ----
const rail = ref<{ renaming: string } | null>(null);

// ---- Ctrl+F code search / Ctrl+\ interface lookup overlay ----
const codeSearchKind = ref<'code' | 'iface' | null>(null);

function onPageKey(e: KeyboardEvent): void {
  // The page stays mounted (v-show) after leaving — only handle Esc while
  // actually visible. Dialogs capture-stop Esc first, so they close first.
  if (nav.page !== 'chat') return;
  if (e.ctrlKey && e.key.toLowerCase() === 'f') {
    // Block the WebView2 find bar: Ctrl+F is project code search here. The
    // overlay's own capture handler refocuses its input when already open.
    e.preventDefault();
    if (!chat.previewPath) codeSearchKind.value = 'code';
    return;
  }
  if (
    e.ctrlKey &&
    (e.key === '\\' || e.code === 'Backslash')
  ) {
    // Ctrl+\ → Java interface lookup (V1: heuristic declaration scan).
    e.preventDefault();
    if (!chat.previewPath) codeSearchKind.value = 'iface';
    return;
  }
  if (e.key === 'Escape' && !rail.value?.renaming) {
    void nav.goMain();
  }
}
onMounted(() => window.addEventListener('keydown', onPageKey));
onBeforeUnmount(() => window.removeEventListener('keydown', onPageKey));

// ---- title row: agent switcher dropdown (features/chat.md §6.1) ----
const CHAT_PROVIDERS = ['kimi', 'claude', 'codex', 'custom'];
function agentUsable(enabled: boolean, provider: string): boolean {
  return enabled && CHAT_PROVIDERS.includes(provider.trim().toLowerCase());
}

const agentOptions = computed(() => sessions.agents.map((a) => `${a.name} · ${a.provider}`));

const agentIndex = computed(() => {
  const cur = chat.meta?.agentId ?? sessions.runtimeStates[chat.sessionId]?.agentId;
  return sessions.agents.findIndex((a) => a.id === cur);
});

const agentDisplay = computed(() => `◆ ${chat.meta?.agentName || 'Agent'}`);

/** 子会话快捷返回：父会话标题（铁轨里查；查不到就空 tooltip）。 */
const parentTitle = computed(
  () => sessions.rail.find((s) => s.sessionId === chat.meta?.parentId)?.title ?? '',
);

function onAgentPick(i: number): void {
  const a = sessions.agents[i];
  if (!a || i === agentIndex.value) return;
  if (!agentUsable(a.enabled, a.provider)) return;
  void chat.switchAgent(a.id);
}

// ---- title row: thinking-effort dropdown (ACP configOptions) ----
// 强度候选 = 该 Agent 配置页勾选的 effortOptions（空 = 全部档位），再与
// CLI 实际声明的 picker 选项取交集：只显示配置的档位，且选择一定生效。
// kimi 报告 `id:"thinking"`（support_efforts 决定选项）；opencode 报告
// `id:"effort"`（其 model variants）。其他 ACP CLI 既不声明也不显示。
const thinkingOpt = computed(() =>
  chat.configOptions.find((o) => o.id === 'thinking' || o.id === 'effort'),
);
const curAgentId = computed(
  () => chat.meta?.agentId ?? sessions.runtimeStates[chat.sessionId]?.agentId ?? '',
);
const curAgent = computed(() => agentsStore.byId(curAgentId.value));
// `thinkingOpts` is the single filtered list the dropdown actually renders,
// so every index (dropdown emit, highlight, current-value clamp) stays aligned.
// Without this, WarDropdown emits an index into the FILTERED list while the
// handler read the UNFILTERED options — selecting "Max" sent "low".
// Before the ACP handshake reports configOptions, the available levels are the
// agent's own effortOptions (opencode variants / kimi support_efforts come from
// them anyway), so the dropdown appears immediately instead of waiting on the
// CLI. The CURRENT value still syncs from ACP when it arrives.
const EFFORT_DISPLAY: Record<string, string> = {
  low: 'Low',
  medium: 'Medium',
  high: 'High',
  xhigh: 'XHigh',
  max: 'Max',
};
const thinkingOpts = computed(() => {
  const effs = curAgent.value?.effortOptions ?? [];
  const reported = chat.configOptions.length > 0;
  if (!reported) {
    return effs.length ? effs.map((v) => ({ name: EFFORT_DISPLAY[v] ?? v, value: v })) : [];
  }
  const opts = thinkingOpt.value?.options ?? [];
  if (effs.length === 0) return opts;
  const allow = new Set(effs);
  return opts.filter((o) => allow.has(o.value));
});
const thinkingOptions = computed(() => thinkingOpts.value.map((o) => o.name));
const thinkingIndex = computed(() => {
  const cur = thinkingOpt.value?.currentValue ?? '';
  return thinkingOpts.value.findIndex((o) => o.value === cur);
});
const thinkingDisplay = computed(() => {
  const cur = thinkingOpt.value?.currentValue ?? '';
  const shown = thinkingOpts.value.find((o) => o.value === cur) ?? thinkingOpts.value[0];
  return `思考 ${shown?.name ?? cur}`;
});
// Config-option id before ACP: opencode reports `effort`, others `thinking`.
const thinkingOptionId = computed(
  () => thinkingOpt.value?.id ?? (curAgent.value?.provider === 'opencode' ? 'effort' : 'thinking'),
);

function onThinkingPick(i: number): void {
  const o = thinkingOpts.value[i];
  if (!o || o.value === thinkingOpt.value?.currentValue) return;
  void chat.setConfigOption(thinkingOptionId.value, o.value);
}

// ---- action bay: 刷新工作区 / 停止生成 dual state (§6.4, §8) ----
function onRefreshOrStop(): void {
  if (chat.status.busy) {
    void chat.cancel();
  } else {
    chat.workspaceRefreshSeq += 1;
  }
}

// ---- title row: 本会话 token 累计 (所有助手消息 usage 求和) ----
const sessionUsage = computed(() => {
  let input = 0;
  let output = 0;
  for (const r of chat.rows) {
    if (r.role !== 'assistant' || !r.usage) continue;
    input += r.usage.inputTokens;
    output += r.usage.outputTokens;
  }
  if (input === 0 && output === 0) return '';
  return `本会话 输入 ${formatTokens(input)} 输出 ${formatTokens(output)}`;
});

// ---- 铁框尺寸：输入框高度 / 操作台宽高，鼠标拖拽改 (0 = 响应式默认) ----
// 三个维度各自独立拖拽，主面板 flex:1 自动吸收剩余空间；拖拽中只改本地
// prefs（不落盘），松手持久化一次，双击恢复默认。与铁轨宽度同一套路。
// 边界与后端 prefs.rs 的 clamp 保持一致：太小不可用，太大挤没主面板。
const COMPOSER_H_MIN = 100;
const COMPOSER_H_MAX = 360;
const BAY_W_MIN = 180;
const BAY_W_MAX = 480;
const BAY_H_MIN = 100;
const BAY_H_MAX = 360;

const composerHeight = computed(() =>
  prefs.composerHeight > 0 ? prefs.composerHeight + 'px' : 'clamp(124px, 19%, 240px)',
);
const gridRows = computed(() =>
  prefs.actionBayHeight > 0
    ? `1fr ${prefs.actionBayHeight}px`
    : '1fr max(124px, min(max(190px, 18.5%), calc(100% - 248px)))',
);
const actionBayWidth = computed(() => (prefs.actionBayWidth > 0 ? prefs.actionBayWidth : 354));

const composerWrap = ref<HTMLElement | null>(null);
const bayWrap = ref<HTMLElement | null>(null);

// 输入框高度（上缘手柄，row-resize）：向上拖变高。
const composerDrag = ref(false);
let composerStartY = 0;
let composerStartH = 0;

function onComposerResizeDown(e: PointerEvent): void {
  composerDrag.value = true;
  composerStartY = e.clientY;
  composerStartH =
    prefs.composerHeight > 0
      ? prefs.composerHeight
      : (composerWrap.value?.getBoundingClientRect().height ?? 124);
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onComposerResizeMove(e: PointerEvent): void {
  if (!composerDrag.value) return;
  const h = Math.min(
    COMPOSER_H_MAX,
    Math.max(COMPOSER_H_MIN, Math.round(composerStartH - (e.clientY - composerStartY))),
  );
  if (h !== prefs.composerHeight) prefs.setComposerHeightLocal(h);
}

function onComposerResizeUp(): void {
  if (!composerDrag.value) return;
  composerDrag.value = false;
  void prefs.setComposerHeight(prefs.composerHeight);
}

function onComposerResizeReset(): void {
  prefs.setComposerHeightLocal(0);
  void prefs.setComposerHeight(0);
}

// 操作台宽度（左缘手柄，col-resize）：向左拖变宽。
const bayDragW = ref(false);
let bayStartX = 0;
let bayStartW = 0;

function onBayResizeDownX(e: PointerEvent): void {
  bayDragW.value = true;
  bayStartX = e.clientX;
  bayStartW = actionBayWidth.value;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onBayResizeMoveX(e: PointerEvent): void {
  if (!bayDragW.value) return;
  const w = Math.min(
    BAY_W_MAX,
    Math.max(BAY_W_MIN, Math.round(bayStartW - (e.clientX - bayStartX))),
  );
  if (w !== prefs.actionBayWidth) prefs.setActionBayWidthLocal(w);
}

function onBayResizeUpX(): void {
  if (!bayDragW.value) return;
  bayDragW.value = false;
  void prefs.setActionBayWidth(prefs.actionBayWidth);
}

function onBayResizeResetX(): void {
  prefs.setActionBayWidthLocal(0);
  void prefs.setActionBayWidth(0);
}

// 操作台高度（上缘手柄，row-resize）：向上拖变高。
const bayDragH = ref(false);
let bayStartY = 0;
let bayStartH = 0;

function onBayResizeDownY(e: PointerEvent): void {
  bayDragH.value = true;
  bayStartY = e.clientY;
  bayStartH =
    prefs.actionBayHeight > 0
      ? prefs.actionBayHeight
      : (bayWrap.value?.getBoundingClientRect().height ?? 190);
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onBayResizeMoveY(e: PointerEvent): void {
  if (!bayDragH.value) return;
  const h = Math.min(
    BAY_H_MAX,
    Math.max(BAY_H_MIN, Math.round(bayStartH - (e.clientY - bayStartY))),
  );
  if (h !== prefs.actionBayHeight) prefs.setActionBayHeightLocal(h);
}

function onBayResizeUpY(): void {
  if (!bayDragH.value) return;
  bayDragH.value = false;
  void prefs.setActionBayHeight(prefs.actionBayHeight);
}

function onBayResizeResetY(): void {
  prefs.setActionBayHeightLocal(0);
  void prefs.setActionBayHeight(0);
}
</script>

<template>
  <PageShell :embed="34">
    <div
      class="chat"
      :style="{ gridTemplateColumns: prefs.railWidth + 'px 1fr auto', gridTemplateRows: gridRows }"
    >
      <!-- far left: sessions of this project -->
      <WarFrame
        class="chat__rail"
        src="/assets/ui/frames/frame_fat_bar.png"
        :slice="[28, 32, 28, 32]"
        :hole="[24, 26, 24, 26]"
      >
        <SessionRail ref="rail" />
      </WarFrame>

      <!-- center column: chat panel + composer, floats overlay at z≥26 -->
      <div class="chat__center">
        <WarFrame
          class="chat__main"
          src="/assets/ui/frames/frame_iron_panel.png"
          :slice="[96, 110, 69, 108]"
          :hole="[56, 25, 21, 24]"
          :content-left-extra="4"
          :content-right-extra="4"
        >
          <div class="chat__main-col">
          <div class="chat__title-row">
            <span class="chat__title war-outline-gold" :style="{ fontSize: prefs.fs(14) + 'px' }">
              {{ chat.meta?.title || '新会话' }}
            </span>
            <span
              v-if="chat.meta?.parentId"
              class="chat__parent"
              :style="{ fontSize: prefs.fs(11) + 'px' }"
              :title="'跳转到父会话：' + parentTitle"
              @click="chat.jumpToParent()"
              >父会话 ▸</span
            >
            <span class="chat__status" :style="{ fontSize: prefs.fs(11) + 'px' }">
              {{ chat.sessionId ? chat.status.statusText : '' }}
            </span>
              <span v-if="sessionUsage" class="chat__usage" :style="{ fontSize: prefs.fs(11) + 'px' }">
                {{ sessionUsage }}
              </span>
            </div>
            <div class="chat__list">
              <MessageList v-if="chat.sessionId" />
              <div v-else class="chat__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">
                选择左侧会话，或点击「＋ 新会话」开始
              </div>
            </div>
          </div>
        </WarFrame>

        <!-- dropdown row above the composer: Agent → 思考（模型只读展示在右侧会话信息面板） -->
        <div v-if="chat.sessionId" class="chat__dd-row">
          <WarDropdown
            class="chat__agent-dd"
            :options="agentOptions"
            :model-value="agentIndex"
            :display-text="agentDisplay"
            :text-size="prefs.fs(12)"
            drop-up
            @update:model-value="onAgentPick"
          />
          <WarDropdown
            v-if="thinkingOptions.length > 0"
            class="chat__thinking-dd"
            :options="thinkingOptions"
            :model-value="thinkingIndex"
            :display-text="thinkingDisplay"
            :text-size="prefs.fs(12)"
            drop-up
            @update:model-value="onThinkingPick"
          />
        </div>

        <div ref="composerWrap" class="chat__composer-wrap" :style="{ height: composerHeight }">
          <div
            class="chat__composer-grip"
            :class="{ active: composerDrag }"
            title="拖动调整输入框高度（100~360px，双击恢复默认）"
            @pointerdown="onComposerResizeDown"
            @pointermove="onComposerResizeMove"
            @pointerup="onComposerResizeUp"
            @pointercancel="onComposerResizeUp"
            @dblclick="onComposerResizeReset"
          >
            <span></span><span></span><span></span>
          </div>
          <WarFrame
            class="chat__input"
            src="/assets/ui/frames/frame_iron_bar.png"
            :slice="[62, 110, 70, 108]"
            :hole="[22, 24, 21, 24]"
            :content-left-extra="4"
            :content-right-extra="4"
          >
            <Composer />
          </WarFrame>
        </div>

        <!-- floating stack above the composer (never squeezes its height).
             QuoteBar hugs the LEFT edge so the dropdown popups in the dd-row
             above stay clear -->
        <div class="chat__float" :style="{ bottom: 'calc(' + composerHeight + ' + 6px)' }">
          <QuoteBar class="chat__quote" />
          <AttachmentBar />
          <SubagentPanel />
          <QueuePanel />
        </div>

        <!-- rate-limit retry banner (§4.3) -->
        <div v-if="chat.retry.active" class="chat__retry" :style="{ fontSize: prefs.fs(12) + 'px' }">
          <span>请求被限流，{{ chat.retry.countdown }} 秒后自动重试（第 {{ chat.retry.attempt }}/{{ chat.retry.maxAttempts }} 次）</span>
          <span class="chat__retry-cancel" @click="chat.retryCancel()">取消重试</span>
        </div>
      </div>

      <!-- right: info panel dock -->
      <div class="chat__dock">
        <WarDock />
      </div>

      <!-- bottom right: action bay -->
      <div ref="bayWrap" class="chat__actions-wrap" :style="{ width: actionBayWidth + 'px' }">
        <WarFrame
          class="chat__actions"
          src="/assets/ui/frames/frame_iron_bar.png"
          :slice="[62, 110, 70, 108]"
          :hole="[22, 24, 21, 24]"
        >
          <div ref="actionBay" class="chat__actions-col">
            <WarButton
              :width="actionBtnW"
              :text="chat.status.busy ? '停止生成' : '刷新工作区(R)'"
              shortcut-key="R"
              :shortcut-active="nav.page === 'chat'"
              @activated="onRefreshOrStop"
            />
            <WarButton
              :width="actionBtnW"
              text="返回(B)"
              shortcut-key="B"
              :shortcut-active="nav.page === 'chat'"
              @activated="nav.goMain()"
            />
          </div>
        </WarFrame>
        <div
          class="chat__actions-grip-v"
          :class="{ active: bayDragW }"
          title="拖动调整操作台宽度（180~480px，双击恢复默认）"
          @pointerdown="onBayResizeDownX"
          @pointermove="onBayResizeMoveX"
          @pointerup="onBayResizeUpX"
          @pointercancel="onBayResizeUpX"
          @dblclick="onBayResizeResetX"
        >
          <span></span><span></span><span></span>
        </div>
        <div
          class="chat__actions-grip-h"
          :class="{ active: bayDragH }"
          title="拖动调整操作台高度（100~360px，双击恢复默认）"
          @pointerdown="onBayResizeDownY"
          @pointermove="onBayResizeMoveY"
          @pointerup="onBayResizeUpY"
          @pointercancel="onBayResizeUpY"
          @dblclick="onBayResizeResetY"
        >
          <span></span><span></span><span></span>
        </div>
      </div>
    </div>

    <!-- modals -->
    <PermissionDialog />
    <FilePreviewDialog />
    <CodeSearchOverlay v-if="codeSearchKind" :kind="codeSearchKind" @close="codeSearchKind = null" />
    <DueTodoOverlay />
  </PageShell>
</template>

<style scoped>
.chat {
  display: grid;
  /* right column is content-sized so the dock's width animation (44px rail
     ↔ 44px+drawer) pushes the chat area narrower instead of overlaying it */
  /* grid-template-columns / grid-template-rows come inline from prefs
     (railWidth / actionBayHeight, both draggable). */
  gap: 8px;
  height: 100%;
  padding: 2px 0 8px 8px; /* rail x=8 clears the permanent window rail */
  box-sizing: border-box;
}

.chat__rail {
  grid-row: 1 / 3;
  grid-column: 1;
  min-height: 0;
}

.chat__center {
  grid-row: 1 / 3;
  grid-column: 2;
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-height: 0;
}

.chat__main {
  flex: 1;
  min-height: 0;
}

/* composer: wrapped in chat__composer-wrap (the flex child that owns the
   height; the resize grip straddles its top edge) */
.chat__composer-wrap {
  position: relative;
  flex: none;
  min-height: 0;
}

.chat__input {
  width: 100%;
  height: 100%;
}

.chat__composer-grip {
  position: absolute;
  top: -3px;
  left: 8px;
  right: 8px;
  height: 6px;
  z-index: 5;
  display: flex;
  flex-direction: row;
  align-items: center;
  justify-content: center;
  gap: 1px;
  cursor: row-resize;
  border-radius: 3px;
  touch-action: none;
  user-select: none;
}

.chat__composer-grip span {
  width: 36px;
  height: 1px;
  background: #5cb380;
  opacity: 0.3;
  transition: opacity 120ms;
}

.chat__composer-grip:hover,
.chat__composer-grip.active {
  background: #5cb380;
  box-shadow: 0 0 4px #5cb38088;
}

.chat__composer-grip:hover span,
.chat__composer-grip.active span {
  opacity: 1;
}

.chat__dock {
  grid-row: 1;
  grid-column: 3;
  min-height: 0;
  /* shrink to the WarDock's animated width, hugging the right edge */
  justify-self: end;
  /* clear the permanent right window rail (PageShell embed=34 tucks the
     band's right 34px under the z40 edge frame) — the drawer buttons must
     not sit in that covered zone */
  margin-right: 34px;
}

.chat__actions-wrap {
  grid-row: 2;
  grid-column: 3;
  position: relative;
  /* width comes inline from prefs.actionBayWidth (draggable); fixed default
     354px so the two menu buttons reach the canonical MENU_BTN_W=276
     (frame 354 − insets 26/26 − content padding 10/10 ≈ 282 usable) */
  justify-self: end;
}

.chat__actions {
  width: 100%;
  height: 100%;
}

.chat__actions-grip-v,
.chat__actions-grip-h {
  position: absolute;
  z-index: 5;
  display: flex;
  flex-direction: row;
  align-items: center;
  justify-content: center;
  gap: 1px;
  border-radius: 3px;
  touch-action: none;
  user-select: none;
}

.chat__actions-grip-v {
  left: -3px; /* hot zone straddles the content frame's left edge */
  top: 0;
  bottom: 0;
  width: 6px;
  cursor: col-resize;
}

.chat__actions-grip-h {
  top: -3px; /* hot zone straddles the content frame's top edge */
  left: 0;
  right: 0;
  height: 6px;
  cursor: row-resize;
}

.chat__actions-grip-v span {
  width: 1px;
  height: 36px;
}

.chat__actions-grip-h span {
  width: 36px;
  height: 1px;
}

.chat__actions-grip-v span,
.chat__actions-grip-h span {
  background: #5cb380;
  opacity: 0.3;
  transition: opacity 120ms;
}

.chat__actions-grip-v:hover,
.chat__actions-grip-v.active,
.chat__actions-grip-h:hover,
.chat__actions-grip-h.active {
  background: #5cb380;
  box-shadow: 0 0 4px #5cb38088;
}

.chat__actions-grip-v:hover span,
.chat__actions-grip-v.active span,
.chat__actions-grip-h:hover span,
.chat__actions-grip-h.active span {
  opacity: 1;
}

.chat__main-col {
  display: flex;
  flex-direction: column;
  gap: 6px;
  height: 100%;
  min-height: 0;
}

.chat__title-row {
  flex: none;
  display: flex;
  align-items: baseline;
  gap: 10px;
  min-width: 0;
}

.chat__title {
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-weight: bold;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 45%;
}

.chat__status {
  flex: 1;
  min-width: 0;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.chat__parent {
  flex: none;
  color: var(--war-gold-bright);
  font-family: SimSun, serif;
  border: 1px solid var(--war-gold-dim);
  border-radius: 10px;
  padding: 0 8px;
  line-height: 16px;
  cursor: pointer;
  user-select: none;
}

.chat__parent:hover {
  border-color: var(--war-gold);
  background: #2a2a18;
}

.chat__usage {
  flex: none;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  white-space: nowrap;
}

.chat__dd-row {
  flex: none;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 6px;
}

.chat__agent-dd {
  flex: none;
  width: 180px;
  height: 30px;
}

.chat__thinking-dd {
  flex: none;
  width: 130px;
  height: 30px;
}

.chat__list {
  flex: 1;
  min-height: 0;
}

.chat__empty {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--war-text-faint);
  font-family: SimSun, serif;
}

.chat__float {
  position: absolute;
  left: 26px;
  right: 26px;
  /* bottom comes inline (composer height, draggable) */
  z-index: 30;
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 4px;
  pointer-events: none;
}

.chat__float > * {
  pointer-events: auto;
  width: min(480px, 100%);
}

/* quote bar: fit its content and pin to the LEFT — out of the dropdown
   popup's zone (the popup anchors to the composer's right side) */
.chat__float > .chat__quote {
  align-self: flex-start;
  width: auto;
}

.chat__retry {
  position: absolute;
  left: 30px;
  right: 30px;
  top: 12px;
  z-index: 32;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 6px 12px;
  background: #201018c0;
  border: 1px solid var(--war-gold);
  border-radius: 4px;
  color: var(--war-gold);
  font-family: SimSun, serif;
}

.chat__retry-cancel {
  flex: none;
  color: var(--war-text);
}

.chat__retry-cancel:hover {
  color: var(--war-gold-bright);
}

.chat__actions-col {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  overflow: hidden;
}
</style>

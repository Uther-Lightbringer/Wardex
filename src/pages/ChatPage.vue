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
import QueuePanel from '../components/chat/QueuePanel.vue';
import SubagentPanel from '../components/chat/SubagentPanel.vue';
import PermissionDialog from '../components/chat/PermissionDialog.vue';
import FilePreviewDialog from '../components/chat/FilePreviewDialog.vue';
import { useNavStore } from '../stores/nav';
import { usePrefsStore } from '../stores/prefs';
import { useChatStore } from '../stores/chat';
import { useSessionsStore } from '../stores/sessions';
import { useElementSize } from '../lib/useElementSize';

const nav = useNavStore();
const prefs = usePrefsStore();
const chat = useChatStore();
const sessions = useSessionsStore();

onMounted(async () => {
  await chat.init();
  await sessions.refreshAgents();
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

function onPageKey(e: KeyboardEvent): void {
  // The page stays mounted (v-show) after leaving — only handle Esc while
  // actually visible. Dialogs capture-stop Esc first, so they close first.
  if (nav.page !== 'chat') return;
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

function onAgentPick(i: number): void {
  const a = sessions.agents[i];
  if (!a || i === agentIndex.value) return;
  if (!agentUsable(a.enabled, a.provider)) return;
  void chat.switchAgent(a.id);
}

// ---- title row: thinking-effort dropdown (ACP configOptions, kimi) ----
// Shown only when the CLI advertised a "thinking" picker for this session
// (kimi always does; per-model support_efforts decide the choices — e.g.
// deepseek-reasoner exposes on/off style levels). Other ACP CLIs that do
// not advertise it simply never see the dropdown.
const thinkingOpt = computed(() => chat.configOptions.find((o) => o.id === 'thinking'));
const thinkingOptions = computed(() => (thinkingOpt.value?.options ?? []).map((o) => o.name));
const thinkingIndex = computed(() =>
  (thinkingOpt.value?.options ?? []).findIndex((o) => o.value === thinkingOpt.value?.currentValue),
);
const thinkingDisplay = computed(() => {
  const cur = thinkingOpt.value?.options.find((o) => o.value === thinkingOpt.value?.currentValue);
  return `思考 ${cur?.name ?? thinkingOpt.value?.currentValue ?? ''}`;
});

function onThinkingPick(i: number): void {
  const o = thinkingOpt.value?.options[i];
  if (!o || o.value === thinkingOpt.value?.currentValue) return;
  void chat.setConfigOption('thinking', o.value);
}

// ---- title row: model dropdown (ACP configOptions "model" picker) ----
// Same mechanics as the thinking dropdown; kimi always advertises it.
const modelOpt = computed(() => chat.configOptions.find((o) => o.id === 'model'));
const modelOptions = computed(() => (modelOpt.value?.options ?? []).map((o) => o.name));
const modelIndex = computed(() =>
  (modelOpt.value?.options ?? []).findIndex((o) => o.value === modelOpt.value?.currentValue),
);
const modelDisplay = computed(() => {
  const cur = modelOpt.value?.options.find((o) => o.value === modelOpt.value?.currentValue);
  return `模型 ${cur?.name ?? modelOpt.value?.currentValue ?? ''}`;
});

function onModelPick(i: number): void {
  const o = modelOpt.value?.options[i];
  if (!o || o.value === modelOpt.value?.currentValue) return;
  void chat.setConfigOption('model', o.value);
}

// ---- action bay: 刷新工作区 / 停止生成 dual state (§6.4, §8) ----
function onRefreshOrStop(): void {
  if (chat.status.busy) {
    void chat.cancel();
  } else {
    chat.workspaceRefreshSeq += 1;
  }
}
</script>

<template>
  <PageShell :embed="34">
    <div class="chat">
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
              <span class="chat__status" :style="{ fontSize: prefs.fs(11) + 'px' }">
                {{ chat.sessionId ? chat.status.statusText : '' }}
              </span>
              <span
                v-if="chat.sessionId"
                class="chat__seg-toggle"
                :style="{ fontSize: prefs.fs(11) + 'px' }"
                @click="chat.setAllSegsOpen(!chat.segCollapseOpen)"
                >{{ chat.segCollapseOpen ? '⊟ 全部折叠' : '⊞ 全部展开' }}</span
              >
              <WarDropdown
                v-if="chat.sessionId && modelOpt"
                class="chat__model-dd"
                :options="modelOptions"
                :model-value="modelIndex"
                :display-text="modelDisplay"
                :text-size="prefs.fs(12)"
                @update:model-value="onModelPick"
              />
              <WarDropdown
                v-if="chat.sessionId && thinkingOpt"
                class="chat__thinking-dd"
                :options="thinkingOptions"
                :model-value="thinkingIndex"
                :display-text="thinkingDisplay"
                :text-size="prefs.fs(12)"
                @update:model-value="onThinkingPick"
              />
              <WarDropdown
                v-if="chat.sessionId"
                class="chat__agent-dd"
                :options="agentOptions"
                :model-value="agentIndex"
                :display-text="agentDisplay"
                :text-size="prefs.fs(12)"
                @update:model-value="onAgentPick"
              />
            </div>
            <div class="chat__list">
              <MessageList v-if="chat.sessionId" />
              <div v-else class="chat__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">
                选择左侧会话，或点击「＋ 新会话」开始
              </div>
            </div>
          </div>
        </WarFrame>

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

        <!-- floating stack above the composer (never squeezes its height) -->
        <div class="chat__float">
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
    </div>

    <!-- modals -->
    <PermissionDialog />
    <FilePreviewDialog />
  </PageShell>
</template>

<style scoped>
.chat {
  display: grid;
  grid-template-columns: 188px 72fr 28fr;
  grid-template-rows: 1fr max(124px, 18.5%);
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

.chat__input {
  flex: none;
  height: clamp(124px, 19%, 240px);
}

.chat__dock {
  grid-row: 1;
  grid-column: 3;
  min-height: 0;
}

.chat__actions {
  grid-row: 2;
  grid-column: 3;
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

.chat__seg-toggle {
  flex: none;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  user-select: none;
  white-space: nowrap;
}

.chat__seg-toggle:hover {
  color: var(--war-gold);
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

.chat__model-dd {
  flex: none;
  width: 210px; /* model ids run long (kimi-code/kimi-for-coding); text ellipsizes */
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
  bottom: calc(clamp(124px, 19%, 240px) + 6px);
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

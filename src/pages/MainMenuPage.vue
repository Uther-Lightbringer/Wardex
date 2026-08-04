<script setup lang="ts">
// Main menu page (Main.qml menuLayer): right rail = chain-hung SteelPanel
// menu (打开项目/新建会话/加载会话/配置/待办 + 退出), left rail = recent
// projects. Both rails ride nav.menuY so they slide in lockstep.
// The layer is always mounted; interactions are gated unless page===main.
// Small windows shrink the whole menu via ui.uiScale (transform: scale).
import { computed, onMounted, ref } from 'vue';
import { getCurrentWindow } from '@tauri-apps/api/window';
import SteelPanel from '../components/war/SteelPanel.vue';
import WarButton from '../components/war/WarButton.vue';
import WarDialog from '../components/war/WarDialog.vue';
import RecentProjectsPanel from '../components/RecentProjectsPanel.vue';
import { useNavStore } from '../stores/nav';
import { useUiStore } from '../stores/ui';
import { useProjectsStore } from '../stores/projects';
import { useChatStore } from '../stores/chat';
import { useSessionsStore } from '../stores/sessions';
import { useMonitorStore } from '../stores/monitor';
import { cmd, isTauri } from '../lib/tauri';

const nav = useNavStore();
const ui = useUiStore();
const projects = useProjectsStore();
const chat = useChatStore();
const sessions = useSessionsStore();
const monitor = useMonitorStore();

// 等待审批角标：runtime_states 快照（重启兜底），事件标记由监控/聊天页监听维护。
onMounted(() => {
  void projects.load();
  void sessions.refreshRuntimeStates();
});

// Letter shortcuts only while the menu is on screen and idle — the old app
// had C/L/S/A stealing keys while chatting ("random leave" bug).
const menuKeysOn = computed(() => nav.page === 'main' && nav.phase === 'idle');

function onOpenProject(): void {
  if (nav.phase !== 'idle') return;
  ui.folderDialogOpen = true;
}

function onNewSession(): void {
  if (nav.phase !== 'idle') return;
  // startNewSession: a projectless (temp) session with the default agent;
  // no usable default → banner + jump to the config page (Main.qml:400-410).
  void chat.startProjectSession('').then((ok) => {
    if (ok) {
      void nav.goOverlay('chat');
    } else {
      ui.showBanner(chat.status.lastError || '请先配置默认 Kimi Agent');
      void nav.goOverlay('config');
    }
  });
}

// ---- recent project click: existence check first (Main.qml:437-446) ----
const missingDir = ref('');
const missingOpen = computed({
  get: () => missingDir.value !== '',
  set: (v: boolean) => {
    if (!v) missingDir.value = '';
  },
});

async function onRecentProjectClicked(path: string): Promise<void> {
  if (nav.phase !== 'idle') return;
  const exists = await cmd<boolean>('project_exists', { dir: path }, true);
  if (!exists) {
    missingDir.value = path;
    return;
  }
  await projects.open(path);
  const ok = await chat.startProjectSession(path);
  if (ok) void nav.goOverlay('chat');
  else ui.showBanner(chat.status.lastError || '无法在该目录创建会话');
}

async function confirmMissing(): Promise<void> {
  const dir = missingDir.value;
  missingDir.value = '';
  if (dir) await projects.remove(dir);
}

async function onExit(): Promise<void> {
  if (isTauri) await getCurrentWindow().close();
  else window.close();
}
</script>

<template>
  <div class="menu-layer" :class="{ inactive: nav.page !== 'main' }">
    <div class="menu-band" :style="{ transform: `translateY(${nav.menuY}px)` }">
      <!-- left rail: recent projects (scaled coordinate system 460x900) -->
      <div class="menu-left" :style="{ width: 460 * ui.uiScale + 'px' }">
        <div class="menu-left__inner" :style="{ transform: `scale(${ui.uiScale})` }">
          <div class="menu-left__panel">
            <RecentProjectsPanel @project-clicked="onRecentProjectClicked" />
          </div>
        </div>
      </div>

      <!-- right rail: menu + exit steel panels (scaled from 400 wide) -->
      <div class="menu-right" :style="{ width: 400 * ui.uiScale + 'px' }">
        <div class="menu-right__inner" :style="{ transform: `scale(${ui.uiScale})` }">
          <div class="menu-stack">
            <SteelPanel title="WarDex" :chain-height="50" :chain-extra="400" class="menu-panel">
              <div class="menu-buttons">
                <WarButton
                  :width="250"
                  text="打开项目(O)"
                  shortcut-key="O"
                  :shortcut-active="menuKeysOn"
                  @activated="onOpenProject"
                />
                <WarButton
                  :width="250"
                  text="新建会话(C)"
                  shortcut-key="C"
                  :shortcut-active="menuKeysOn"
                  @activated="onNewSession"
                />
                <WarButton
                  :width="250"
                  text="加载会话(L)"
                  shortcut-key="L"
                  :shortcut-active="menuKeysOn"
                  @activated="nav.goOverlay('sessionSelect')"
                />
                <div class="monitor-btn-wrap">
                  <WarButton
                    :width="250"
                    text="战场监控(M)"
                    shortcut-key="M"
                    :shortcut-active="menuKeysOn"
                    @activated="nav.goOverlay('monitor')"
                  />
                  <span v-if="monitor.permPendingCount > 0" class="monitor-badge">
                    {{ monitor.permPendingCount }}
                  </span>
                </div>
                <WarButton
                  :width="250"
                  text="配置(S)"
                  shortcut-key="S"
                  :shortcut-active="menuKeysOn"
                  @activated="nav.goOverlay('config')"
                />
                <WarButton
                  :width="250"
                  text="待办(T)"
                  shortcut-key="T"
                  :shortcut-active="menuKeysOn"
                  @activated="nav.goOverlay('todo')"
                />
              </div>
            </SteelPanel>

            <SteelPanel variant="short" :chain-height="24" :chain-overlap-up="18" class="exit-panel">
              <WarButton
                :width="280"
                text="退出(A)"
                shortcut-key="A"
                :shortcut-active="menuKeysOn"
                @activated="onExit"
              />
            </SteelPanel>
          </div>
        </div>
      </div>
    </div>

    <!-- 最近项目的目录已丢失：确定后移除该项目 -->
    <WarDialog
      v-model:open="missingOpen"
      title-text="项目不存在"
      :message-text="'目录已被删除或移动：\n' + missingDir + '\n该项目将从最近列表中移除。'"
    >
      <WarButton skin="dialog" :width="190" :art-aspect="5.34" text="确定" @activated="confirmMissing" />
    </WarDialog>
  </div>
</template>

<style scoped>
.menu-layer {
  position: absolute;
  inset: 0;
  z-index: 10;
}

.menu-layer.inactive {
  pointer-events: none;
}

.menu-band {
  position: absolute;
  inset: 0;
}

.menu-left {
  position: absolute;
  left: 0;
  top: 0;
  bottom: 0;
}

.menu-left__inner {
  width: 460px;
  height: 900px;
  transform-origin: top left;
}

.menu-left__panel {
  position: absolute;
  left: 76px;
  top: 96px;
  width: 360px;
  height: 460px;
}

.menu-right {
  position: absolute;
  right: 0;
  top: 0;
  bottom: 0;
}

.menu-right__inner {
  width: 400px;
  height: 100%;
  transform-origin: top right;
  margin-left: auto;
}

.menu-stack {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  margin-right: 36px;
  padding-top: 58px; /* chainHeight 50 + top margin 8 above menuPanel's frame */
}

.menu-panel {
  width: 344px;
  flex: none;
}

.menu-buttons {
  width: 250px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

/* 战场监控按钮的等待审批角标（原型 .war-btn .badge） */
.monitor-btn-wrap {
  position: relative;
  width: 250px;
}

.monitor-badge {
  position: absolute;
  right: -8px;
  top: -8px;
  min-width: 22px;
  height: 22px;
  line-height: 22px;
  border-radius: 11px;
  background: #c9a227;
  color: #1a1000;
  font-size: 13px;
  font-weight: bold;
  font-family: SimSun, serif;
  text-align: center;
  padding: 0 5px;
  border: 1px solid #ffe9a0;
  box-shadow: 0 0 8px #c9a227;
  box-sizing: border-box;
  pointer-events: none;
  animation: monitor-badge-pulse 1.2s infinite;
}

@keyframes monitor-badge-pulse {
  50% {
    transform: scale(1.15);
  }
}

.exit-panel {
  width: 344px;
  flex: none;
  /* exitPanel.top = menuPanel.bottom in the old layout */
}
</style>

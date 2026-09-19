<script setup lang="ts">
// App shell (Main.qml equivalent): background stack → main menu layer →
// overlay pages → PERMANENT iron rails (created once, never slide or get
// destroyed) → banner → modal dialogs.
//
// Pages are built on first visit (nav.visited) and kept resident (v-show) —
// the old cached-Loader behaviour. All navigation runs through the nav store
// three-stage transition (770ms up / popUp SFX 1280ms gate / 750ms drop).
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { getVersion } from '@tauri-apps/api/app';
import { listen } from '@tauri-apps/api/event';
import { isTauri } from './lib/tauri';
import { DEFAULT_BG } from './lib/background';
import { themeOf } from './lib/themes';
import MainMenuPage from './pages/MainMenuPage.vue';
import HubPage from './pages/HubPage.vue';
import ConfigPage from './pages/ConfigPage.vue';
import SettingsPage from './pages/SettingsPage.vue';
import SessionSelectPage from './pages/SessionSelectPage.vue';
import ChatPage from './pages/ChatPage.vue';
import TodoPage from './pages/TodoPage.vue';
import UsagePage from './pages/UsagePage.vue';
import MonitorPage from './pages/MonitorPage.vue';
import FolderBrowserDialog from './components/FolderBrowserDialog.vue';
import StartFailureDialog from './components/StartFailureDialog.vue';
import { preloadSfx } from './lib/sfx';
import { useNavStore, type PageId } from './stores/nav';
import { useUiStore } from './stores/ui';
import { hexToRgbTriple, usePrefsStore } from './stores/prefs';
import { useProjectsStore } from './stores/projects';
import { useChatStore } from './stores/chat';
import type { StartFailure } from './stores/ui';

const nav = useNavStore();
const ui = useUiStore();
const prefs = usePrefsStore();
const projects = useProjectsStore();
const chat = useChatStore();

const version = ref('0.3');

// ---- 界面风格（war | pure）：挂 data-theme 到 <html> 供全局 CSS 覆盖 ----
const theme = computed(() => themeOf(prefs.uiStyle));
watch(
  () => theme.value.id,
  (id) => document.documentElement.setAttribute('data-theme', id),
  { immediate: true },
);
// 对话页透明度（纯净风格）→ CSS 变量，warTheme.css 的 rgba() 直接引用。
watch(
  () => prefs.chatAlpha,
  (a) => document.documentElement.style.setProperty('--war-chat-alpha', String(a)),
  { immediate: true },
);
// 页面颜色（纯净风格表面层底色）→ hex + rgb 三元组两个变量：
//   --war-page-color  供需要原始 hex 的地方（如设置页色板高亮）；
//   --war-page-rgb    供 rgba(var(--war-page-rgb), alpha) 半透明表面。
watch(
  () => prefs.pageColor,
  (c) => {
    const root = document.documentElement;
    root.style.setProperty('--war-page-color', c);
    root.style.setProperty('--war-page-rgb', hexToRgbTriple(c));
  },
  { immediate: true },
);
// 背景亮度（纯净风格，作用于背景图/视频）→ --war-bg-brightness，.bg-img/.bg-video 的 filter 引用。
watch(
  () => prefs.bgBrightness,
  (v) => document.documentElement.style.setProperty('--war-bg-brightness', String(v)),
  { immediate: true },
);

/** 实际渲染的背景：war = 内置视频/自定义照旧；pure = 默认纯白，但用户
 * 自定义上传的背景（图片/视频）与 background.json 覆盖仍然显示（决策 1）。 */
const renderBg = computed(() => {
  const b = prefs.background;
  if (theme.value.kind !== 'plain') return b;
  if (prefs.bgType) return b; // 用户上传的自定义背景照常显示
  return b.source !== DEFAULT_BG.source ? b : null; // background.json 覆盖显示；否则纯白
});

const overlayPages: { id: PageId; comp: unknown }[] = [
  { id: 'hub', comp: HubPage },
  { id: 'settings', comp: SettingsPage },
  { id: 'config', comp: ConfigPage },
  { id: 'sessionSelect', comp: SessionSelectPage },
  { id: 'chat', comp: ChatPage },
  { id: 'todo', comp: TodoPage },
  { id: 'usage', comp: UsagePage },
  { id: 'monitor', comp: MonitorPage },
];

function onResize(): void {
  ui.updateUiScale(window.innerWidth, window.innerHeight);
}

function onFolderChosen(path: string): void {
  const purpose = ui.folderDialogPurpose;
  ui.folderDialogPurpose = 'open'; // reset for the next opener
  if (purpose === 'bind') {
    // Bind the current chat session to this project dir (docs: B option).
    void chat.bindProject(path).then((ok) => {
      if (!ok) ui.showBanner(chat.status.lastError || '无法关联该目录');
    });
    return;
  }
  // Open project → create its chat session, then drop into the chat page
  // (startProjectSession; a refused create only shows the banner).
  void projects.open(path);
  void chat.startProjectSession(path).then((ok) => {
    if (ok) void nav.goOverlay('chat');
    else ui.showBanner(chat.status.lastError || '无法在该目录创建会话');
  });
}

/** 弹框里「打开该会话」：把用户直接送到那个失败的后台会话。 */
function onStartFailureOpenSession(sessionId: string): void {
  ui.closeStartFailure();
  void chat.openSession(sessionId).then((ok) => {
    if (ok) void nav.goOverlay('chat');
    else ui.showBanner('无法打开该会话');
  });
}

let unlistenStartFailure: (() => void) | null = null;

onMounted(() => {
  preloadSfx();
  void prefs.load();
  void projects.load();
  onResize();
  if (isTauri) void getVersion().then((v) => (version.value = v));
  window.addEventListener('resize', onResize);
  // 后台会话 agent 启动失败（启动预热等）：后端单发事件，这里弹框 ——
  // `chat://status` 按 sessionId 过滤，非活跃会话的错误到不了用户眼前。
  if (isTauri) {
    void listen<StartFailure>('wardex://agentStartFailed', (e) => {
      ui.showStartFailure(e.payload);
    }).then((off) => {
      unlistenStartFailure = off;
    });
  }
});
onBeforeUnmount(() => {
  window.removeEventListener('resize', onResize);
  unlistenStartFailure?.();
  unlistenStartFailure = null;
});
</script>

<template>
  <div class="app" :data-theme="theme.id">
    <!-- background stack: gradient base → image/video → dim gradient (§8.2) -->
    <div class="bg-base" :class="{ 'is-pure': !renderBg }"></div>
    <img v-if="renderBg?.type === 'image'" class="bg-img" :src="renderBg.source" draggable="false" />
    <video
      v-else-if="renderBg?.type === 'video'"
      class="bg-video"
      :src="renderBg.source"
      autoplay
      muted
      loop
      playsinline
    ></video>
    <!-- TODO(phase-4): model background (Three.js glTF, 45s orbiting camera) -->
    <div class="bg-dim"></div>

    <!-- main menu (always mounted; slides via nav.menuY, input-gated) -->
    <MainMenuPage />

    <!-- overlay pages (hidden on main: pure keeps overlayY=0, so without
         v-show the transparent full-screen band would swallow menu clicks) -->
    <div class="overlay" v-show="nav.page !== 'main'" :style="{ transform: `translateY(${nav.overlayY}px)` }">
      <template v-for="p in overlayPages" :key="p.id">
        <div v-if="nav.visited[p.id]" v-show="nav.page === p.id" class="overlay__slot">
          <component :is="p.comp" />
        </div>
      </template>
    </div>

    <!-- permanent left/right iron rails: created once, z40, never slide -->
    <div v-if="theme.rails" class="rails">
      <img class="rails__l" src="/assets/ui/frames/frame_edge_left.png" draggable="false" />
      <img class="rails__r" src="/assets/ui/frames/frame_edge_right.png" draggable="false" />
    </div>

    <!-- banner notification -->
    <div v-if="ui.bannerText" class="banner">{{ ui.bannerText }}</div>

    <div class="version">WarDex v{{ version }} · Tauri 重写</div>

    <!-- 打开项目 folder browser -->
    <FolderBrowserDialog v-model:open="ui.folderDialogOpen" @folder-chosen="onFolderChosen" />

    <!-- 后台会话 agent 启动失败（启动预热 / 项目待办 / 监控小窗） -->
    <StartFailureDialog
      :failure="ui.startFailure"
      @close="ui.closeStartFailure()"
      @open-session="onStartFailureOpenSession"
    />
  </div>
</template>

<style scoped>
.app {
  position: relative;
  width: 100%;
  height: 100%;
  overflow: hidden;
}

.bg-base {
  position: absolute;
  inset: 0;
  background: linear-gradient(#0e2a22, #0a1a16 60%, #04070a);
  transition: background 200ms;
}

.bg-base.is-pure {
  background: #f4f6f8;
}

.bg-img,
.bg-video {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover; /* Qt PreserveAspectCrop */
  /* 背景亮度（纯净风格专属；war 下变量缺省 = 1 无影响） */
  filter: brightness(var(--war-bg-brightness, 1));
}

.bg-dim {
  position: absolute;
  inset: 0;
  background: linear-gradient(#00000000, #00000020 55%, #00000090);
}

.overlay {
  position: absolute;
  inset: 0;
  z-index: 20;
}

.overlay__slot {
  position: absolute;
  inset: 0;
}

.rails {
  position: absolute;
  inset: 0;
  z-index: 40;
  pointer-events: none;
}

.rails img {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 58px;
  height: 100%;
}

.rails__l {
  left: 0;
}

.rails__r {
  right: 0;
}

.banner {
  position: absolute;
  z-index: 50;
  left: 50%;
  transform: translateX(-50%);
  top: 24px;
  max-width: calc(100% - 80px);
  height: 40px;
  padding: 0 20px;
  display: flex;
  align-items: center;
  border-radius: 4px;
  background: #201018c0;
  border: 1px solid var(--war-gold);
  color: var(--war-gold);
  font-size: 14px;
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.version {
  position: absolute;
  left: 68px; /* rail width 58px + 10px margin, clear of the left iron rail */
  bottom: 10px;
  z-index: 30;
  color: #5a6472;
  font-size: 12px;
  font-family: SimSun, serif;
}
</style>

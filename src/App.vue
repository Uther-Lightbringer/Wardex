<script setup lang="ts">
// App shell (Main.qml equivalent): background stack → main menu layer →
// overlay pages → PERMANENT iron rails (created once, never slide or get
// destroyed) → banner → modal dialogs.
//
// Pages are built on first visit (nav.visited) and kept resident (v-show) —
// the old cached-Loader behaviour. All navigation runs through the nav store
// three-stage transition (770ms up / popUp SFX 1280ms gate / 750ms drop).
import { onBeforeUnmount, onMounted, ref } from 'vue';
import { getVersion } from '@tauri-apps/api/app';
import { isTauri } from './lib/tauri';
import MainMenuPage from './pages/MainMenuPage.vue';
import ConfigPage from './pages/ConfigPage.vue';
import SessionSelectPage from './pages/SessionSelectPage.vue';
import ChatPage from './pages/ChatPage.vue';
import TodoPage from './pages/TodoPage.vue';
import UsagePage from './pages/UsagePage.vue';
import MonitorPage from './pages/MonitorPage.vue';
import FolderBrowserDialog from './components/FolderBrowserDialog.vue';
import { DEFAULT_BG, loadBackground, type BgConfig } from './lib/background';
import { preloadSfx } from './lib/sfx';
import { useNavStore, type PageId } from './stores/nav';
import { useUiStore } from './stores/ui';
import { usePrefsStore } from './stores/prefs';
import { useProjectsStore } from './stores/projects';
import { useChatStore } from './stores/chat';

const nav = useNavStore();
const ui = useUiStore();
const prefs = usePrefsStore();
const projects = useProjectsStore();
const chat = useChatStore();

const bg = ref<BgConfig>(DEFAULT_BG);
const version = ref('0.3');

const overlayPages: { id: PageId; comp: unknown }[] = [
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

onMounted(() => {
  preloadSfx();
  void prefs.load();
  void projects.load();
  void loadBackground().then((c) => (bg.value = c));
  onResize();
  if (isTauri) void getVersion().then((v) => (version.value = v));
  window.addEventListener('resize', onResize);
});
onBeforeUnmount(() => window.removeEventListener('resize', onResize));
</script>

<template>
  <div class="app">
    <!-- background stack: gradient base → image/video → dim gradient (§8.2) -->
    <div class="bg-base"></div>
    <img v-if="bg.type === 'image'" class="bg-img" :src="bg.source" draggable="false" />
    <video
      v-else-if="bg.type === 'video'"
      class="bg-video"
      :src="bg.source"
      autoplay
      muted
      loop
      playsinline
    ></video>
    <!-- TODO(phase-4): model background (Three.js glTF, 45s orbiting camera) -->
    <div class="bg-dim"></div>

    <!-- main menu (always mounted; slides via nav.menuY, input-gated) -->
    <MainMenuPage />

    <!-- overlay pages -->
    <div class="overlay" :style="{ transform: `translateY(${nav.overlayY}px)` }">
      <template v-for="p in overlayPages" :key="p.id">
        <div v-if="nav.visited[p.id]" v-show="nav.page === p.id" class="overlay__slot">
          <component :is="p.comp" />
        </div>
      </template>
    </div>

    <!-- permanent left/right iron rails: created once, z40, never slide -->
    <div class="rails">
      <img class="rails__l" src="/assets/ui/frames/frame_edge_left.png" draggable="false" />
      <img class="rails__r" src="/assets/ui/frames/frame_edge_right.png" draggable="false" />
    </div>

    <!-- banner notification -->
    <div v-if="ui.bannerText" class="banner">{{ ui.bannerText }}</div>

    <div class="version">WarDex v{{ version }} · Tauri 重写</div>

    <!-- 打开项目 folder browser -->
    <FolderBrowserDialog v-model:open="ui.folderDialogOpen" @folder-chosen="onFolderChosen" />
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
}

.bg-img,
.bg-video {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover; /* Qt PreserveAspectCrop */
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

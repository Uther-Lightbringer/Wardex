<script setup lang="ts">
// Hub page (更多功能): mid-level menu grouping the three secondary pages
// 配置 / 用量统计 / 待办 behind one main-menu entry. A plain overlay page —
// rides the standard nav three-stage transition (pull up, drop down) via
// PageShell like Config/Usage; Esc returns to the main menu.
import { computed, onBeforeUnmount, onMounted } from 'vue';
import PageShell from '../components/PageShell.vue';
import WarFrame from '../components/war/WarFrame.vue';
import WarButton from '../components/war/WarButton.vue';
import { useNavStore } from '../stores/nav';
import { usePrefsStore } from '../stores/prefs';
import { useUiStore } from '../stores/ui';
import { themeOf } from '../lib/themes';

const nav = useNavStore();
const prefs = usePrefsStore();
const ui = useUiStore();

const keysOn = computed(() => nav.page === 'hub');

// 与主菜单右侧 SteelPanel 对齐：菜单面板在「400px 右轨、右缩 36、顶 58、
// 面板宽 344」的 uiScale 坐标系里（MainMenuPage.vue .menu-stack/.menu-panel）。
// PageShell 在 war 下右侧内缩 6px（edgeW 58 - embed 52），纯净下 0，要扣掉。
const plain = computed(() => themeOf(prefs.uiStyle).kind === 'plain');
const frameWidth = computed(() => Math.round(344 * ui.uiScale));
const padTop = computed(() => Math.round(58 * ui.uiScale));
const padRight = computed(() => Math.max(0, Math.round(36 * ui.uiScale) - (plain.value ? 0 : 6)));

// Esc → back to main menu (same as the 返回(B) button).
function onPageKey(e: KeyboardEvent): void {
  if (nav.page !== 'hub') return;
  if (e.key === 'Escape') void nav.goMain();
}
onMounted(() => window.addEventListener('keydown', onPageKey));
onBeforeUnmount(() => window.removeEventListener('keydown', onPageKey));
</script>

<template>
  <PageShell :embed="52">
    <div class="hub" :style="{ paddingTop: padTop + 'px', paddingRight: padRight + 'px' }">
      <WarFrame
        class="hub__frame"
        :style="{ width: frameWidth + 'px' }"
        src="/assets/ui/frames/frame_iron_panel.png"
        :slice="[96, 110, 69, 108]"
        :hole="[56, 25, 21, 24]"
        :content-left-extra="16"
        hug
      >
        <div class="hub__col">
          <div class="hub__title war-font-title war-outline-black" :style="{ fontSize: prefs.fs(18) + 'px' }">
            更多功能
          </div>
          <div class="hub__buttons">
            <WarButton
              :width="250"
              text="设置(S)"
              shortcut-key="S"
              :shortcut-active="keysOn"
              @activated="nav.goOverlay('settings')"
            />
            <WarButton
              :width="250"
              text="Agent 配置(C)"
              shortcut-key="C"
              :shortcut-active="keysOn"
              @activated="nav.goOverlay('config')"
            />
            <WarButton
              :width="250"
              text="用量统计(U)"
              shortcut-key="U"
              :shortcut-active="keysOn"
              @activated="nav.goOverlay('usage')"
            />
            <WarButton
              :width="250"
              text="待办(T)"
              shortcut-key="T"
              :shortcut-active="keysOn"
              @activated="nav.goOverlay('todo')"
            />
          </div>
          <div class="hub__actions">
            <WarButton
              :width="250"
              text="返回(B)"
              shortcut-key="B"
              :shortcut-active="keysOn"
              @activated="nav.goMain()"
            />
          </div>
        </div>
      </WarFrame>
    </div>
  </PageShell>
</template>

<style scoped>
.hub {
  display: flex;
  align-items: flex-start; /* 顶部距离 = 主菜单面板的 58px（uiScale 缩放） */
  justify-content: flex-end; /* 靠右，与主菜单右侧面板同侧同宽 */
  height: 100%;
  padding: 0 0 8px 0; /* top/right 由内联样式按 uiScale 注入 */
  box-sizing: border-box;
}

.hub__frame {
  /* 宽度内联注入 = 主菜单面板 344px × uiScale；高度 hug 按内容撑开 */
}

.hub__col {
  display: flex;
  flex-direction: column;
}

.hub__title {
  flex: none;
  color: var(--war-text-dim);
  text-align: center;
}

.hub__buttons {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 14px;
  padding: 18px 0;
}

.hub__actions {
  display: flex;
  justify-content: center;
  padding-bottom: 10px;
}
</style>

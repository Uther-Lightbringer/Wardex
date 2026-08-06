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

const nav = useNavStore();
const prefs = usePrefsStore();

const keysOn = computed(() => nav.page === 'hub');

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
    <div class="hub">
      <WarFrame
        class="hub__frame"
        src="/assets/ui/frames/frame_iron_panel.png"
        :slice="[96, 110, 69, 108]"
        :hole="[56, 25, 21, 24]"
        :content-left-extra="16"
      >
        <div class="hub__col">
          <div class="hub__title war-font-title war-outline-black" :style="{ fontSize: prefs.fs(18) + 'px' }">
            更多功能
          </div>
          <div class="hub__buttons">
            <WarButton
              :width="276"
              text="配置(S)"
              shortcut-key="S"
              :shortcut-active="keysOn"
              @activated="nav.goOverlay('config')"
            />
            <WarButton
              :width="276"
              text="用量统计(U)"
              shortcut-key="U"
              :shortcut-active="keysOn"
              @activated="nav.goOverlay('usage')"
            />
            <WarButton
              :width="276"
              text="待办(T)"
              shortcut-key="T"
              :shortcut-active="keysOn"
              @activated="nav.goOverlay('todo')"
            />
          </div>
          <div class="hub__actions">
            <WarButton
              :width="276"
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
  align-items: flex-start; /* 挂耳紧贴窗口顶 */
  justify-content: flex-end; /* 靠右，与主菜单右侧面板同侧 */
  height: 100%;
  padding: 0 36px 8px 0;
  box-sizing: border-box;
}

.hub__frame {
  width: 480px;
  height: 460px;
}

.hub__col {
  display: flex;
  flex-direction: column;
  height: 100%;
}

.hub__title {
  flex: none;
  color: var(--war-text-dim);
  text-align: center;
}

.hub__buttons {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 14px;
}

.hub__actions {
  flex: none;
  display: flex;
  justify-content: center;
}
</style>

<script setup lang="ts">
// Left rail of the main menu: recent projects in the half-scale popup frame
// (RecentProjectsPanel.qml). Click → open a session in that directory.
// Right-click WarMenu: 重命名 (alias only, disk untouched) / 从列表移除.
// Row hover: KeyboardHighlight glow at 0.55 opacity (mix-blend screen).
import { nextTick, ref } from 'vue';
import WarFrame from './war/WarFrame.vue';
import WarMenu, { type WarMenuItem } from './war/WarMenu.vue';
import { folderBaseName, useProjectsStore } from '../stores/projects';

const emit = defineEmits<{ (e: 'projectClicked', path: string): void }>();

const projects = useProjectsStore();

// ---- inline rename ----
const editingPath = ref('');
const editName = ref('');
const editInput = ref<HTMLInputElement | null>(null);

function startRename(path: string): void {
  editingPath.value = path;
  editName.value = projects.displayName(path);
  void nextTick(() => {
    editInput.value?.focus();
    editInput.value?.select();
  });
}

function confirmRename(path: string): void {
  editingPath.value = '';
  const n = editName.value.trim();
  // Renaming back to the folder's own name = clearing the alias
  void projects.setAlias(path, n === folderBaseName(path) ? '' : n);
}

// ---- context menu ----
const menuVisible = ref(false);
const menuX = ref(0);
const menuY = ref(0);
const menuTarget = ref('');
const menuItems: WarMenuItem[] = [{ label: '重命名' }, { label: '从列表移除' }];

function onContextMenu(e: MouseEvent, path: string): void {
  menuTarget.value = path;
  menuX.value = e.clientX;
  menuY.value = e.clientY;
  menuVisible.value = true;
}

function onMenuSelect(i: number): void {
  if (i === 0) startRename(menuTarget.value);
  else if (i === 1) void projects.remove(menuTarget.value);
}

function formatDate(ms: number): string {
  if (!ms) return '';
  const d = new Date(Number(ms));
  return `${d.getMonth() + 1}-${d.getDate()}`;
}
</script>

<template>
  <WarFrame
    class="recent"
    src="/assets/ui/frames/frame_popup_small.png"
    :slice="[44, 50, 45, 50]"
    :inset="[23, 33, 24, 31]"
  >
    <div class="recent__col">
      <div class="recent__title war-font-title war-outline-black">最近项目</div>

      <div class="recent__list">
        <div
          v-for="p in projects.recent"
          :key="p.path"
          class="recent__item"
          :class="{ editing: editingPath === p.path }"
          @click="editingPath !== p.path && emit('projectClicked', p.path)"
          @contextmenu.prevent="onContextMenu($event, p.path)"
        >
          <span class="recent__glow"></span>
          <template v-if="editingPath !== p.path">
            <span class="recent__name">{{ projects.displayName(p.path) }}</span>
            <span class="recent__date">{{ formatDate(p.lastOpenedAt) }}</span>
            <span class="recent__path">{{ p.path }}</span>
          </template>
          <input
            v-else
            ref="editInput"
            v-model="editName"
            class="war-inline-input recent__edit"
            @keydown.enter.prevent="confirmRename(p.path)"
            @keydown.esc.stop="editingPath = ''"
            @click.stop
          />
        </div>

        <div v-if="projects.recent.length === 0" class="recent__empty">暂无最近项目</div>
      </div>
    </div>

    <WarMenu
      v-model:visible="menuVisible"
      :x="menuX"
      :y="menuY"
      :items="menuItems"
      @select="onMenuSelect"
    />
  </WarFrame>
</template>

<style scoped>
.recent {
  width: 100%;
  height: 100%;
}

.recent__col {
  display: flex;
  flex-direction: column;
  height: 100%;
  gap: 10px;
}

.recent__title {
  text-align: center;
  color: var(--war-text-dim);
  font-size: 17px;
  flex: none;
}

.recent__list {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 6px;
  overflow-y: auto;
  scrollbar-width: none;
}

.recent__item {
  position: relative;
  flex: none;
  height: 52px;
  background: #10141dcc;
  border: 1px solid #1a2230;
  box-sizing: border-box;
}

.recent__item:hover {
  background: #1a2334;
  border-color: #2c4a7a;
}

.recent__item.editing {
  border-color: var(--war-gold-input);
}

.recent__glow {
  position: absolute;
  inset: 0;
  background: url('/assets/wc3_extracted/ui/GlueScreen-Button-KeyboardHighlight.png') 0 0 / 100% 100% no-repeat;
  mix-blend-mode: screen;
  opacity: 0;
  pointer-events: none;
}

.recent__item:hover:not(.editing) .recent__glow {
  opacity: 0.55;
}

.recent__name {
  position: absolute;
  left: 10px;
  top: 6px;
  right: 46px;
  color: var(--war-gold);
  font-size: 15px;
  font-weight: bold;
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.recent__date {
  position: absolute;
  right: 8px;
  top: 9px;
  color: #6d7688;
  font-size: 11px;
}

.recent__path {
  position: absolute;
  left: 10px;
  right: 8px;
  bottom: 6px;
  color: #8b93a6;
  font-size: 11px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  direction: rtl; /* elide in the middle-ish: keep the tail (folder name) visible */
  text-align: left;
}

.recent__edit {
  position: absolute;
  left: 8px;
  top: 4px;
  right: 46px;
  height: 24px;
  font-size: 14px;
}

.recent__empty {
  margin: auto;
  color: var(--war-text-faint);
  font-size: 13px;
  font-family: SimSun, serif;
}
</style>

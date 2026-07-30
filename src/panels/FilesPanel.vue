<script setup lang="ts">
// Workspace file tree (features/chat.md §6.4): flat visible-row tree with
// lazy directory expansion (one list_workspace_dir call per expand, children
// inserted after the parent; collapse drops descendants). No filesystem
// watcher — the root re-reads on session switch / manual refresh, and a
// re-read collapses all expansion state. Click a file → preview dialog;
// right-click → 打开（系统默认方式）.
import { computed, onMounted, ref, watch } from 'vue';
import { cmd, openPath } from '../lib/tauri';
import { useChatStore } from '../stores/chat';
import { usePrefsStore } from '../stores/prefs';
import WarMenu from '../components/war/WarMenu.vue';

interface DirEntry {
  name: string;
  dir: boolean;
}

interface TreeRow {
  rel: string; // workspace-relative path
  name: string;
  dir: boolean;
  depth: number;
  expanded: boolean;
  hasKids: boolean; // directory known to have visible children
}

const chat = useChatStore();
const prefs = usePrefsStore();

const rows = ref<TreeRow[]>([]);
const loaded = new Set<string>(); // rel paths of expanded dirs ('' = root)

const root = computed(() => chat.meta?.workDir || chat.meta?.projectDir || chat.projectDir);

async function loadLevel(rel: string): Promise<DirEntry[]> {
  try {
    return await cmd<DirEntry[]>('list_workspace_dir', { root: root.value, rel }, []);
  } catch (e) {
    console.warn('[files] list_workspace_dir failed', e);
    return [];
  }
}

/** Re-read the root; all expansion state collapses (§6.4). */
async function reload(): Promise<void> {
  rows.value = [];
  loaded.clear();
  if (!root.value) return;
  const entries = await loadLevel('');
  loaded.add('');
  rows.value = entries.map((e) => ({
    rel: e.name,
    name: e.name,
    dir: e.dir,
    depth: 0,
    expanded: false,
    hasKids: true, // unknown until expanded — show the arrow
  }));
}

async function toggleDir(row: TreeRow, index: number): Promise<void> {
  if (row.expanded) {
    // Collapse: drop every descendant row (deeper rel prefixed by rel + '/').
    const prefix = row.rel + '/';
    let count = 0;
    while (index + 1 + count < rows.value.length && rows.value[index + 1 + count].rel.startsWith(prefix)) {
      count++;
    }
    rows.value.splice(index + 1, count);
    loaded.delete(row.rel);
    row.expanded = false;
    row.hasKids = true;
    return;
  }
  const entries = await loadLevel(row.rel);
  loaded.add(row.rel);
  const kids: TreeRow[] = entries.map((e) => ({
    rel: row.rel + '/' + e.name,
    name: e.name,
    dir: e.dir,
    depth: row.depth + 1,
    expanded: false,
    hasKids: true,
  }));
  rows.value.splice(index + 1, 0, ...kids);
  row.expanded = true;
  row.hasKids = kids.length > 0; // empty dirs lose their arrow
}

function onRowClick(row: TreeRow, index: number): void {
  if (row.dir) {
    void toggleDir(row, index);
  } else {
    chat.openPreview(joinAbs(row.rel));
  }
}

function joinAbs(rel: string): string {
  const base = root.value.replace(/[\\/]+$/, '');
  return base + '\\' + rel.replace(/\//g, '\\');
}

// ---- right-click: 打开（系统默认方式） ----
const menuVisible = ref(false);
const menuX = ref(0);
const menuY = ref(0);
const menuRow = ref<TreeRow | null>(null);

function onContextMenu(e: MouseEvent, row: TreeRow): void {
  menuRow.value = row;
  menuX.value = e.clientX;
  menuY.value = e.clientY;
  menuVisible.value = true;
}

function onMenuSelect(): void {
  if (menuRow.value) void openPath(joinAbs(menuRow.value.rel));
}

onMounted(reload);
watch(() => chat.sessionId, reload); // sessionSwitch
watch(() => chat.workspaceRefreshSeq, reload); // 刷新工作区 button
</script>

<template>
  <div class="tree">
    <div class="tree__bar">
      <span class="tree__refresh" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="reload">刷新</span>
    </div>
    <div class="tree__list">
      <div v-if="rows.length === 0" class="tree__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">
        暂无产出文件
      </div>
      <div
        v-for="(row, i) in rows"
        :key="row.rel"
        class="tree__row"
        :style="{ paddingLeft: row.depth * 14 + 2 + 'px' }"
        @click="onRowClick(row, i)"
        @contextmenu.prevent="onContextMenu($event, row)"
      >
        <span v-if="row.dir" class="tree__arrow">{{ row.expanded ? '▼' : '▶' }}</span>
        <span v-else class="tree__arrow-file"></span>
        <img
          class="tree__icon"
          :src="row.dir ? '/assets/wc3_extracted/ui/icon-folder.png' : '/assets/wc3_extracted/ui/icon-file.png'"
          draggable="false"
        />
        <span
          class="tree__name"
          :class="{ dir: row.dir }"
          :style="{ fontSize: prefs.fs(12) + 'px' }"
          :title="row.rel"
          >{{ row.name }}</span
        >
      </div>
    </div>

    <WarMenu
      v-model:visible="menuVisible"
      :x="menuX"
      :y="menuY"
      :items="[{ label: '打开（系统默认方式）' }]"
      @select="onMenuSelect"
    />
  </div>
</template>

<style scoped>
.tree {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  font-family: SimSun, serif;
}

.tree__bar {
  flex: none;
  display: flex;
  justify-content: flex-end;
}

.tree__refresh {
  color: var(--war-gold);
  user-select: none;
}

.tree__refresh:hover {
  color: var(--war-gold-bright);
}

.tree__list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
}

.tree__empty {
  color: var(--war-text-faint);
  text-align: center;
  padding: 8px 0;
}

.tree__row {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 22px;
  user-select: none;
}

.tree__row:hover {
  background: #32509633;
}

.tree__arrow {
  flex: none;
  width: 12px;
  color: var(--war-text-muted);
  font-size: 9px;
  text-align: center;
}

.tree__arrow-file {
  flex: none;
  width: 12px;
}

.tree__icon {
  flex: none;
  width: 14px;
  height: 14px;
}

.tree__name {
  color: #d0d6e0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tree__name.dir {
  color: #e8d9a0;
}
</style>

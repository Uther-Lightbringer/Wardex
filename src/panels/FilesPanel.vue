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
import WarScrollBar from '../components/war/WarScrollBar.vue';

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

// ---- scroll target (WC3 WarScrollBar) ----
const listEl = ref<HTMLElement | null>(null);
const bigListEl = ref<HTMLElement | null>(null);

// ---- expand overlay: the 222px dock drawer is too narrow for deep trees -
// opens the same tree state in a large centered dialog (Esc / mask close).
const big = ref(false);
function onBigKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.stopPropagation();
    big.value = false;
  }
}
watch(big, (v) => {
  if (v) window.addEventListener('keydown', onBigKey, true);
  else window.removeEventListener('keydown', onBigKey, true);
});

const root = computed(() => chat.meta?.workDir || chat.meta?.projectDir || chat.projectDir);

async function loadLevel(rel: string): Promise<DirEntry[]> {
  try {
    return await cmd<DirEntry[]>('list_workspace_dir', { root: root.value, rel }, []);
  } catch (e) {
    console.warn('[files] list_workspace_dir failed', e);
    return [];
  }
}

/** Re-read the root, preserving expansion state best-effort: previously
 * expanded dirs that still exist are re-expanded in place (turnEnd
 * auto-refresh would otherwise collapse the tree after every reply). */
async function reload(): Promise<void> {
  if (!root.value) {
    rows.value = [];
    loaded.clear();
    return;
  }
  const expandedDirs = [...loaded]
    .filter((r) => r !== '')
    .sort((a, b) => a.split('/').length - b.split('/').length); // parents first
  const entries = await loadLevel('');
  loaded.clear();
  loaded.add('');
  rows.value = entries.map((e) => ({
    rel: e.name,
    name: e.name,
    dir: e.dir,
    depth: 0,
    expanded: false,
    hasKids: true, // unknown until expanded — show the arrow
  }));
  for (const rel of expandedDirs) {
    const idx = rows.value.findIndex((r) => r.rel === rel && r.dir && !r.expanded);
    if (idx >= 0) await toggleDir(rows.value[idx], idx);
  }
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
watch(() => chat.turnSeq, reload); // turnEnd — the agent may have written files
watch(() => chat.workspaceRefreshSeq, reload); // 刷新工作区 button

// Empty-state copy: guide project-less sessions toward opening a project.
const emptyText = computed(() =>
  chat.meta?.projectDir ? '暂无产出文件' : '（未关联项目——从主菜单打开项目后，此处显示项目文件）',
);
</script>

<template>
  <div class="tree">
    <div class="tree__bar">
      <span class="tree__refresh" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="reload">刷新</span>
      <span class="tree__refresh" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="big = true">放大</span>
    </div>
    <div class="tree__list-wrap">
      <div ref="listEl" class="tree__list">
        <div v-if="rows.length === 0" class="tree__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">
          {{ emptyText }}
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
      <WarScrollBar :target="listEl" />
    </div>

    <WarMenu
      v-model:visible="menuVisible"
      :x="menuX"
      :y="menuY"
      :items="[{ label: '打开（系统默认方式）' }]"
      @select="onMenuSelect"
    />
  </div>

  <!-- expand overlay: the same tree at a large size for many/deep files.
       Teleported out of the drawer — the drawer's translateX transform would
       otherwise capture position:fixed. -->
  <Teleport to="body">
    <div v-if="big" class="files-big-mask" @mousedown.self="big = false">
    <div class="files-big">
      <WarFrame
        class="files-big__frame"
        src="/assets/ui/frames/frame_fat_bar.png"
        :slice="[23, 26, 22, 25]"
        :hole="[23, 26, 22, 25]"
      >
        <div class="files-big__col">
          <div class="files-big__bar">
            <img
              class="files-big__icon"
              src="/assets/wc3_extracted/ui/icon-folder.png"
              draggable="false"
            />
            <span class="files-big__title" :style="{ fontSize: prefs.fs(14) + 'px' }">工作区文件</span>
            <span class="files-big__spacer"></span>
            <span class="files-big__tool" :style="{ fontSize: prefs.fs(12) + 'px' }" @click="reload">刷新</span>
            <span class="files-big__tool" :style="{ fontSize: prefs.fs(12) + 'px' }" @click="big = false">✕ 关闭</span>
          </div>
          <div class="files-big__wrap">
            <div ref="bigListEl" class="tree__list">
              <div v-if="rows.length === 0" class="tree__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">
                {{ emptyText }}
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
                  :style="{ fontSize: prefs.fs(14) + 'px' }"
                  :title="row.rel"
                  >{{ row.name }}</span
                >
              </div>
            </div>
            <WarScrollBar :target="bigListEl" />
          </div>
        </div>
      </WarFrame>
    </div>
    </div>
  </Teleport>
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
  gap: 10px;
}

.tree__refresh {
  color: var(--war-gold);
  user-select: none;
}

.tree__refresh:hover {
  color: var(--war-gold-bright);
}

.tree__list-wrap {
  flex: 1;
  min-height: 0;
  display: flex;
}

.tree__list {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none; /* native bar hidden — the WC3 WarScrollBar replaces it */
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

/* ---- expand overlay ---- */
.files-big-mask {
  position: fixed;
  inset: 0;
  z-index: 90;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.files-big {
  width: min(880px, 92vw);
  height: min(640px, 78vh);
}

.files-big__frame {
  width: 100%;
  height: 100%;
}

.files-big__col {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  gap: 4px;
}

.files-big__bar {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  user-select: none;
}

.files-big__icon {
  width: 18px;
  height: 18px;
}

.files-big__title {
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-weight: bold;
  text-shadow:
    -1px 0 var(--war-outline-brown), 1px 0 var(--war-outline-brown),
    0 -1px var(--war-outline-brown), 0 1px var(--war-outline-brown);
}

.files-big__spacer {
  flex: 1;
}

.files-big__tool {
  color: var(--war-gold);
  font-family: SimSun, serif;
  user-select: none;
}

.files-big__tool:hover {
  color: var(--war-gold-bright);
}

.files-big__wrap {
  flex: 1;
  min-height: 0;
  display: flex;
}

.files-big__wrap .tree__row {
  height: 26px;
}
</style>

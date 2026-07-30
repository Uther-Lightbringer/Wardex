<script setup lang="ts">
// Session select page (features/sessions-and-config.md 第一部分):
// left = search + full-text results + project-grouped session list, right top
// = selected-session summary, right bottom = 进入会话/返回 action bay.
//   - Groups: newest-active group first; sessions pinned-first then
//     updatedAt desc (stable). projectDir "" → 临时会话（无项目）.
//   - One search box drives BOTH the instant title filter (group-level
//     hiding, collapsed ignored) and the 500ms-debounced full-text scan
//     (search_messages; generation double-guard against stale results).
//   - Single click selects (summary only); double click enters; the right
//     click menu mirrors the chat rail (pin / rename / copy / ask / delete).
//   - Enter-session guard (Main.qml enterSessionFromSelect): ignored while a
//     page transition or a previous enter is in flight; a missing project
//     dir opens the 项目不存在 dialog (confirm deletes the session).
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import PageShell from '../components/PageShell.vue';
import WarFrame from '../components/war/WarFrame.vue';
import WarButton from '../components/war/WarButton.vue';
import WarMenu, { type WarMenuItem } from '../components/war/WarMenu.vue';
import WarDialog from '../components/war/WarDialog.vue';
import { cmd } from '../lib/tauri';
import { useNavStore } from '../stores/nav';
import { usePrefsStore } from '../stores/prefs';
import { useSessionsStore, type SearchHit, type SessionIndexRow } from '../stores/sessions';
import { folderBaseName, useProjectsStore } from '../stores/projects';
import { useChatStore } from '../stores/chat';
import { useUiStore } from '../stores/ui';

const nav = useNavStore();
const prefs = usePrefsStore();
const sessions = useSessionsStore();
const projects = useProjectsStore();
const chat = useChatStore();
const ui = useUiStore();

// ---------------------------------------------------------------------------
// Grouping (spec §2.1)
// ---------------------------------------------------------------------------

interface Group {
  projectDir: string;
  sessions: SessionIndexRow[];
}

const collapsed = ref<Record<string, boolean>>({}); // runtime only, never persisted
const query = ref('');
const searchFocused = ref(false);

const titleQuery = computed(() => query.value.trim().toLowerCase());

const groups = computed<Group[]>(() => {
  const q = titleQuery.value;
  const byDir = new Map<string, SessionIndexRow[]>();
  // sessions.all is updatedAt-desc already → group order = first-seen order,
  // which IS "latest session of the group first" (spec §2.1).
  for (const s of sessions.all) {
    if (q && !s.title.toLowerCase().includes(q)) continue;
    const list = byDir.get(s.projectDir);
    if (list) list.push(s);
    else byDir.set(s.projectDir, [s]);
  }
  const out: Group[] = [];
  for (const [projectDir, list] of byDir) {
    // Pinned first, stable (time order kept inside each pin class).
    out.push({ projectDir, sessions: [...list].sort((a, b) => Number(b.pinned) - Number(a.pinned)) });
  }
  return out;
});

function groupName(dir: string): string {
  if (!dir) return '临时会话（无项目）';
  return projects.displayName(dir);
}

/** Group rows shown under the header: searching ignores the collapsed map. */
function visibleSessions(g: Group): SessionIndexRow[] {
  if (titleQuery.value) return g.sessions;
  return collapsed.value[g.projectDir] ? [] : g.sessions;
}

/** Qt ElideMiddle equivalent (CSS cannot do true middle elision). */
function elideMiddle(s: string, max = 44): string {
  const chars = [...s];
  if (chars.length <= max) return s;
  const head = Math.ceil((max - 1) / 2);
  const tail = Math.floor((max - 1) / 2);
  return chars.slice(0, head).join('') + '…' + chars.slice(chars.length - tail).join('');
}

function fmtTime(ms: number): string {
  if (!ms) return '';
  const d = new Date(Number(ms));
  const p = (n: number): string => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

// ---------------------------------------------------------------------------
// Selection + summary (spec §4)
// ---------------------------------------------------------------------------

const selectedId = ref('');
const selectedRow = computed(() => sessions.all.find((s) => s.id === selectedId.value));

function selectFirstIfNeeded(): void {
  if (selectedId.value && sessions.all.some((s) => s.id === selectedId.value)) return;
  const first = groups.value[0]?.sessions[0];
  selectedId.value = first?.id ?? '';
}

async function reload(): Promise<void> {
  await sessions.reloadAll();
  selectFirstIfNeeded();
}

let unlistenSessions: UnlistenFn | null = null;

onMounted(async () => {
  void projects.load();
  await reload();
  // sessionsChanged → rebuild groups, keep (or re-fallback) the selection.
  unlistenSessions = await listen('store://sessions', () => void reload());
});
onBeforeUnmount(() => unlistenSessions?.());

// Page enter (kept-alive afterwards): rescan the index + fallback selection.
watch(
  () => nav.page,
  (p) => {
    if (p === 'sessionSelect') void reload();
  },
);

// ---------------------------------------------------------------------------
// Enter session (spec §4 guard chain)
// ---------------------------------------------------------------------------

const entering = ref(false);
const missingSessionId = ref('');
const missingPath = ref('');
const missingOpen = computed({
  get: () => missingSessionId.value !== '',
  set: (v: boolean) => {
    if (!v) missingSessionId.value = '';
  },
});

async function enterSession(id: string): Promise<void> {
  // Ignore while a transition or a previous enter is in flight (old crash:
  // openSession x3 during the swap animation).
  if (nav.phase !== 'idle' || entering.value) return;
  if (!id) {
    ui.showBanner('无效的会话 ID');
    return;
  }
  const pdir = sessions.all.find((s) => s.id === id)?.projectDir ?? '';
  if (pdir) {
    const exists = await cmd<boolean>('project_exists', { dir: pdir }, true);
    if (!exists) {
      missingSessionId.value = id;
      missingPath.value = pdir;
      return;
    }
  }
  entering.value = true;
  try {
    // Deferred: leave the input event stack before opening, then navigate
    // (the old code used two Qt.callLater ticks).
    await nextTick();
    const ok = await chat.openSession(id);
    if (!ok) {
      ui.showBanner('无法打开会话');
      return;
    }
    await nextTick();
    await nav.goOverlay('chat');
  } finally {
    entering.value = false;
  }
}

/** 项目不存在 (session): confirm closes the runtime and deletes the session. */
async function confirmMissingSession(): Promise<void> {
  const id = missingSessionId.value;
  missingSessionId.value = '';
  if (!id) return;
  await chat.deleteSession(id); // delete_session closes the runtime first
  if (selectedId.value === id) selectedId.value = '';
  await reload();
}

// ---------------------------------------------------------------------------
// ＋新会话 in a project group (spec §2.2): same path as the main menu's
// open-recent-project, including the directory existence check.
// ---------------------------------------------------------------------------

const missingProjectDir = ref('');
const missingProjectOpen = computed({
  get: () => missingProjectDir.value !== '',
  set: (v: boolean) => {
    if (!v) missingProjectDir.value = '';
  },
});

async function newSessionIn(dir: string): Promise<void> {
  if (nav.phase !== 'idle' || entering.value) return;
  const exists = await cmd<boolean>('project_exists', { dir }, true);
  if (!exists) {
    missingProjectDir.value = dir;
    return;
  }
  entering.value = true;
  try {
    await projects.open(dir);
    const ok = await chat.startProjectSession(dir);
    if (!ok) {
      ui.showBanner(chat.status.lastError || '无法在该目录创建会话');
      return;
    }
    await nav.goOverlay('chat');
  } finally {
    entering.value = false;
  }
}

async function confirmMissingProject(): Promise<void> {
  const dir = missingProjectDir.value;
  missingProjectDir.value = '';
  if (dir) await projects.remove(dir);
}

// ---------------------------------------------------------------------------
// Search (spec §3): instant title filter + debounced full-text scan
// ---------------------------------------------------------------------------

const searching = ref(false);
const searched = ref(false);
const hits = ref<SearchHit[]>([]);
let debounceTimer: ReturnType<typeof setTimeout> | null = null;

const searchBlockVisible = computed(
  () => titleQuery.value !== '' && (searching.value || searched.value || hits.value.length > 0),
);

async function runSearch(): Promise<void> {
  const q = query.value.trim();
  if (!q) {
    // Cancel any in-flight scan and reset the block immediately.
    void sessions.searchMessages('');
    hits.value = [];
    searching.value = false;
    searched.value = false;
    return;
  }
  searching.value = true;
  const r = await sessions.searchMessages(q);
  if (r === null) return; // superseded by a newer query — drop
  hits.value = r;
  searching.value = false;
  searched.value = true;
}

watch(query, () => {
  if (debounceTimer) clearTimeout(debounceTimer);
  if (!query.value.trim()) {
    void runSearch();
    return;
  }
  debounceTimer = setTimeout(() => void runSearch(), 500);
});
onBeforeUnmount(() => {
  if (debounceTimer) clearTimeout(debounceTimer);
});

/** Enter triggers the scan immediately (kills the debounce timer). */
function onSearchEnter(): void {
  if (debounceTimer) clearTimeout(debounceTimer);
  void runSearch();
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/** Snippet with the first hit wrapped gold+bold (StyledText equivalent). */
function highlightSnippet(snippet: string): string {
  const q = query.value.trim();
  const esc = escapeHtml(snippet);
  if (!q) return esc;
  const idx = esc.toLowerCase().indexOf(q.toLowerCase());
  if (idx < 0) return esc;
  return `${esc.slice(0, idx)}<b class="hl">${esc.slice(idx, idx + q.length)}</b>${esc.slice(idx + q.length)}`;
}

// ---------------------------------------------------------------------------
// Session row context menu (same semantics as the chat rail, spec §2.3)
// ---------------------------------------------------------------------------

const menuVisible = ref(false);
const menuX = ref(0);
const menuY = ref(0);
const menuSession = ref<SessionIndexRow | null>(null);

const menuItems = computed<WarMenuItem[]>(() => {
  const s = menuSession.value;
  if (!s) return [];
  return [
    { label: s.pinned ? '取消置顶' : '置顶会话' },
    { label: '重命名会话' },
    { label: '复制会话内容' },
    { label: '基于此提问' },
    { label: '删除会话' },
  ];
});

function onContextMenu(e: MouseEvent, s: SessionIndexRow): void {
  menuSession.value = s;
  menuX.value = e.clientX;
  menuY.value = e.clientY;
  menuVisible.value = true;
}

function onMenuSelect(i: number): void {
  const s = menuSession.value;
  if (!s) return;
  switch (i) {
    case 0:
      void sessions.setPinned(s.id, !s.pinned).then(reload);
      break;
    case 1:
      startRename(s);
      break;
    case 2:
      void sessions.copyTranscript(s.id).then((err) => {
        if (err) ui.showBanner(err);
      });
      break;
    case 3:
      void askBasedOn(s);
      break;
    case 4:
      deleteTarget.value = s;
      break;
  }
}

/** 基于此提问: new empty session in the same project + composer prefill. */
async function askBasedOn(s: SessionIndexRow): Promise<void> {
  sessions.pendingComposerText = `基于会话「${s.title}」：`;
  chat.projectDir = s.projectDir;
  const ok = await chat.newSession();
  if (!ok) {
    ui.showBanner(chat.status.lastError || '无法创建会话');
    return;
  }
  await nav.goOverlay('chat');
}

// ---- delete confirm ----
const deleteTarget = ref<SessionIndexRow | null>(null);
const deleteOpen = computed({
  get: () => deleteTarget.value !== null,
  set: (v: boolean) => {
    if (!v) deleteTarget.value = null;
  },
});

async function confirmDelete(): Promise<void> {
  const s = deleteTarget.value;
  deleteTarget.value = null;
  if (!s) return;
  await chat.deleteSession(s.id); // handles the active-session case too
  if (selectedId.value === s.id) selectedId.value = '';
  await reload();
}

// ---------------------------------------------------------------------------
// Inline rename: session title (§2.3) and project alias (§2.4)
// ---------------------------------------------------------------------------

const renamingId = ref('');
const renameText = ref('');

function startRename(s: SessionIndexRow): void {
  renamingId.value = s.id;
  renameText.value = s.title;
}

async function commitRename(): Promise<void> {
  const id = renamingId.value;
  const title = renameText.value.trim().slice(0, 48);
  renamingId.value = '';
  if (!id || !title) return;
  try {
    await sessions.rename(id, title);
    await reload(); // selected row's summary refreshes with the list
  } catch (e) {
    console.warn('[sessionSelect] rename failed', e);
  }
}

const renamingDir = ref('');
const aliasText = ref('');

function startAliasRename(dir: string): void {
  renamingDir.value = dir;
  aliasText.value = groupName(dir);
}

async function commitAliasRename(): Promise<void> {
  const dir = renamingDir.value;
  const n = aliasText.value.trim();
  renamingDir.value = '';
  if (!dir) return;
  // Same-as-folder-name = clearing the alias (independent persistence).
  await projects.setAlias(dir, n === folderBaseName(dir) ? '' : n);
}

// ---- group header context menu (project groups only): 重命名项目 ----
const groupMenuVisible = ref(false);
const groupMenuX = ref(0);
const groupMenuY = ref(0);
const groupMenuDir = ref('');
const groupMenuItems: WarMenuItem[] = [{ label: '重命名项目' }];

function onGroupContextMenu(e: MouseEvent, dir: string): void {
  if (!dir) return; // the temp group has no rename
  groupMenuDir.value = dir;
  groupMenuX.value = e.clientX;
  groupMenuY.value = e.clientY;
  groupMenuVisible.value = true;
}

// ---------------------------------------------------------------------------
// Esc: inline edits and the focused search box get it first (spec §4)
// ---------------------------------------------------------------------------

function onPageKey(e: KeyboardEvent): void {
  if (nav.page !== 'sessionSelect') return;
  if (e.key !== 'Escape') return;
  if (renamingId.value || renamingDir.value) return; // inputs stop their own Esc
  if (searchFocused.value) {
    (document.activeElement as HTMLElement | null)?.blur();
    return;
  }
  void nav.goMain();
}
onMounted(() => window.addEventListener('keydown', onPageKey));
onBeforeUnmount(() => window.removeEventListener('keydown', onPageKey));

const pageKeysOn = computed(() => nav.page === 'sessionSelect');
</script>

<template>
  <PageShell :embed="52">
    <div class="sel">
      <!-- left: history sessions -->
      <WarFrame
        class="sel__left"
        src="/assets/ui/frames/frame_iron_panel.png"
        :slice="[96, 110, 69, 108]"
        :hole="[56, 25, 21, 24]"
        :content-left-extra="16"
      >
        <div class="sel__col">
          <div class="sel__title war-font-title war-outline-black" :style="{ fontSize: prefs.fs(18) + 'px' }">
            历史会话
          </div>
          <input
            v-model="query"
            class="war-input sel__search"
            placeholder="搜索会话标题与内容…"
            :style="{ fontSize: prefs.fs(13) + 'px' }"
            @focus="searchFocused = true"
            @blur="searchFocused = false"
            @keydown.enter.prevent="onSearchEnter"
          />

          <!-- full-text results block (spec §3.2) -->
          <div v-if="searchBlockVisible" class="sel__results">
            <div class="sel__results-head" :style="{ fontSize: prefs.fs(12) + 'px' }">
              {{ searching ? '全文搜索中…' : `全文搜索结果（${hits.length}）` }}
            </div>
            <div class="sel__results-list">
              <div
                v-for="(h, i) in hits"
                :key="i"
                class="sel__hit"
                @click="enterSession(h.sessionId)"
              >
                <div class="sel__hit-top">
                  <span class="sel__hit-title" :style="{ fontSize: prefs.fs(12) + 'px' }">{{ h.sessionTitle }}</span>
                  <span class="sel__hit-time" :style="{ fontSize: prefs.fs(10) + 'px' }">{{ fmtTime(h.timestamp) }}</span>
                </div>
                <div v-if="h.titleOnly" class="sel__hit-snip" :style="{ fontSize: prefs.fs(11) + 'px' }">
                  （仅标题命中）
                </div>
                <!-- eslint-disable-next-line vue/no-v-html -- snippet is HTML-escaped before highlighting -->
                <div v-else class="sel__hit-snip" :style="{ fontSize: prefs.fs(11) + 'px' }" v-html="highlightSnippet(h.snippet)"></div>
              </div>
              <div v-if="!searching && searched && hits.length === 0" class="sel__no-hit" :style="{ fontSize: prefs.fs(12) + 'px' }">
                无匹配内容
              </div>
            </div>
          </div>

          <!-- grouped session list -->
          <div class="sel__list">
            <template v-for="g in groups" :key="g.projectDir">
              <!-- group header -->
              <div
                class="sel__group"
                @click="collapsed[g.projectDir] = !collapsed[g.projectDir]"
                @contextmenu.prevent="onGroupContextMenu($event, g.projectDir)"
              >
                <span class="sel__arrow" :style="{ fontSize: prefs.fs(11) + 'px' }">
                  {{ collapsed[g.projectDir] && !titleQuery ? '▶' : '▼' }}
                </span>
                <img class="sel__folder" src="/assets/wc3_extracted/ui/icon-folder.png" draggable="false" />
                <template v-if="renamingDir === g.projectDir">
                  <input
                    v-model="aliasText"
                    class="war-inline-input sel__alias-edit"
                    :style="{ fontSize: prefs.fs(12) + 'px' }"
                    v-focus
                    @keydown.enter.prevent="commitAliasRename"
                    @keydown.esc.stop.prevent="renamingDir = ''"
                    @click.stop
                  />
                </template>
                <template v-else>
                  <span class="sel__group-name" :style="{ fontSize: prefs.fs(13) + 'px' }">{{ groupName(g.projectDir) }}</span>
                  <span class="sel__group-path" :style="{ fontSize: prefs.fs(10) + 'px' }">
                    {{ g.projectDir ? elideMiddle(g.projectDir) : '' }}
                  </span>
                  <span
                    v-if="g.projectDir"
                    class="sel__group-new"
                    :style="{ fontSize: prefs.fs(11) + 'px' }"
                    @click.stop="newSessionIn(g.projectDir)"
                  >＋新会话</span>
                </template>
              </div>

              <!-- session rows -->
              <div
                v-for="s in visibleSessions(g)"
                :key="s.id"
                class="sel__row"
                :class="{ selected: s.id === selectedId }"
                @click="selectedId = s.id"
                @dblclick="enterSession(s.id)"
                @contextmenu.prevent="onContextMenu($event, s)"
              >
                <span v-if="s.pinned" class="sel__pin">📌</span>
                <template v-if="renamingId === s.id">
                  <input
                    v-model="renameText"
                    class="war-inline-input sel__rename"
                    :style="{ fontSize: prefs.fs(13) + 'px' }"
                    maxlength="48"
                    v-focus
                    @keydown.enter.prevent="commitRename"
                    @keydown.esc.stop.prevent="renamingId = ''"
                    @click.stop
                    @dblclick.stop
                  />
                </template>
                <template v-else>
                  <span class="sel__row-title" :style="{ fontSize: prefs.fs(13) + 'px' }">{{ s.title }}</span>
                  <span class="sel__row-provider" :style="{ fontSize: prefs.fs(10) + 'px' }">{{ s.provider }}</span>
                  <span class="sel__row-time" :style="{ fontSize: prefs.fs(10) + 'px' }">{{ fmtTime(s.updatedAt) }}</span>
                  <span v-if="sessions.unreadIds.includes(s.id)" class="sel__new-badge" :style="{ fontSize: prefs.fs(9) + 'px' }">NEW</span>
                </template>
              </div>
            </template>

            <!-- empty states (spec §3.1) -->
            <div v-if="groups.length === 0" class="sel__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">
              {{ titleQuery ? '无标题匹配的会话' : '暂无历史会话\n请先「新建会话」或「打开项目」' }}
            </div>
          </div>
        </div>
      </WarFrame>

      <!-- right top: summary -->
      <WarFrame
        class="sel__right-top"
        src="/assets/ui/frames/frame_iron_panel.png"
        :slice="[96, 110, 69, 108]"
        :hole="[56, 25, 21, 24]"
      >
        <div v-if="selectedRow" class="sel__summary">
          <div class="sel__summary-title" :style="{ fontSize: prefs.fs(15) + 'px' }">{{ selectedRow.title }}</div>
          <div class="sel__summary-grid" :style="{ fontSize: prefs.fs(12) + 'px' }">
            <span class="k">Agent:</span><span class="v">{{ selectedRow.agentName || 'Agent' }}</span>
            <span class="k">Provider:</span><span class="v">{{ selectedRow.provider }}</span>
            <span class="k">消息数:</span><span class="v">{{ selectedRow.messageCount }}</span>
            <span class="k">更新:</span><span class="v">{{ fmtTime(selectedRow.updatedAt) }}</span>
            <span class="k">项目:</span>
            <span class="v">{{ selectedRow.projectDir ? elideMiddle(selectedRow.projectDir, 36) : '（临时会话，无项目）' }}</span>
          </div>
          <div class="sel__summary-abstract" :style="{ fontSize: prefs.fs(12) + 'px' }">
            <span class="k">摘要:</span>
            <span class="v">{{ selectedRow.summary || '（无）' }}</span>
          </div>
        </div>
        <div v-else class="sel__summary-hint" :style="{ fontSize: prefs.fs(14) + 'px' }">
          选择左侧会话\n查看概略信息
        </div>
      </WarFrame>

      <!-- right bottom: action bar -->
      <WarFrame
        class="sel__right-bottom"
        src="/assets/ui/frames/frame_iron_bar.png"
        :slice="[62, 110, 70, 108]"
        :hole="[22, 24, 21, 24]"
      >
        <div class="sel__actions">
          <WarButton
            :width="276"
            text="进入会话(L)"
            shortcut-key="L"
            :shortcut-active="pageKeysOn"
            :enabled="!!selectedRow && !entering"
            @activated="selectedRow && enterSession(selectedRow.id)"
          />
          <WarButton
            :width="276"
            text="返回(B)"
            shortcut-key="B"
            :shortcut-active="pageKeysOn"
            @activated="nav.goMain()"
          />
        </div>
      </WarFrame>
    </div>

    <!-- session row menu -->
    <WarMenu v-model:visible="menuVisible" :x="menuX" :y="menuY" :items="menuItems" @select="onMenuSelect" />
    <!-- group header menu -->
    <WarMenu
      v-model:visible="groupMenuVisible"
      :x="groupMenuX"
      :y="groupMenuY"
      :items="groupMenuItems"
      @select="startAliasRename(groupMenuDir)"
    />

    <!-- delete confirm -->
    <WarDialog
      v-model:open="deleteOpen"
      title-text="删除会话"
      :message-text="'确定删除这条会话及其全部消息吗？\n该操作不可撤销。'"
    >
      <WarButton skin="dialog" :width="190" :art-aspect="5.34" text="删除" @activated="confirmDelete" />
      <WarButton skin="dialog" :width="190" :art-aspect="5.34" text="取消" @activated="deleteTarget = null" />
    </WarDialog>

    <!-- session's project dir is gone: confirm deletes the session -->
    <WarDialog
      v-model:open="missingOpen"
      title-text="项目不存在"
      :message-text="'该会话的项目目录已被删除或移动：\n' + missingPath + '\n点击确定后将删除这条会话。'"
    >
      <WarButton skin="dialog" :width="190" :art-aspect="5.34" text="确定" @activated="confirmMissingSession" />
    </WarDialog>

    <!-- ＋新会话 target dir is gone: confirm removes it from recents -->
    <WarDialog
      v-model:open="missingProjectOpen"
      title-text="项目不存在"
      :message-text="'目录已被删除或移动：\n' + missingProjectDir + '\n该项目将从最近列表中移除。'"
    >
      <WarButton skin="dialog" :width="190" :art-aspect="5.34" text="确定" @activated="confirmMissingProject" />
    </WarDialog>
  </PageShell>
</template>

<script lang="ts">
// v-focus: focus + select-all an inline edit input when it mounts.
export default {
  directives: {
    focus: {
      mounted(el: HTMLElement) {
        el.focus();
        (el as HTMLInputElement).select?.();
      },
    },
  },
};
</script>

<style scoped>
.sel {
  display: grid;
  grid-template-columns: 62fr 38fr;
  grid-template-rows: 1fr max(188px, 20%);
  gap: 10px;
  height: 100%;
  padding-top: 4px;
  padding-bottom: 8px;
  box-sizing: border-box;
}

.sel__left {
  grid-row: 1; /* old layout: bottom-left region stays empty (background shows) */
  grid-column: 1;
  min-height: 0;
}

.sel__right-top {
  grid-row: 1;
  grid-column: 2;
  min-height: 0;
}

.sel__right-bottom {
  grid-row: 2;
  grid-column: 2;
}

.sel__col {
  display: flex;
  flex-direction: column;
  gap: 10px;
  height: 100%;
}

.sel__title {
  color: var(--war-text-dim);
  flex: none;
}

.sel__search {
  flex: none;
  height: 30px;
}

/* ---- full-text results block ---- */
.sel__results {
  flex: none;
  max-height: 150px;
  display: flex;
  flex-direction: column;
  border: 1px solid #2a3344;
  background: #10141f99;
}

.sel__results-head {
  flex: none;
  padding: 4px 8px;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  border-bottom: 1px solid #2a3344;
}

.sel__results-list {
  flex: 1;
  min-height: 0;
  max-height: 120px;
  overflow-y: auto;
  scrollbar-width: none;
}

.sel__hit {
  padding: 4px 8px;
}

.sel__hit:hover {
  background: #32509640;
}

.sel__hit-top {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.sel__hit-title {
  flex: 1;
  min-width: 0;
  color: var(--war-gold);
  font-weight: bold;
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sel__hit-time {
  flex: none;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
}

.sel__hit-snip {
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sel__hit-snip :deep(.hl) {
  color: var(--war-gold);
  font-weight: bold;
}

.sel__no-hit {
  padding: 8px;
  text-align: center;
  color: var(--war-text-faint);
  font-family: SimSun, serif;
}

/* ---- grouped list ---- */
.sel__list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.sel__group {
  flex: none;
  height: 30px;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 4px;
  user-select: none;
}

.sel__group:hover {
  background: #32509633;
}

.sel__arrow {
  flex: none;
  width: 12px;
  color: var(--war-gold);
  font-family: SimSun, serif;
}

.sel__folder {
  flex: none;
  width: 18px;
  height: 14px;
}

.sel__group-name {
  flex: none;
  max-width: 40%;
  color: var(--war-gold);
  font-weight: bold;
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sel__group-path {
  flex: 1;
  min-width: 0;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sel__group-new {
  flex: none;
  padding: 1px 6px;
  color: var(--war-gold);
  font-family: SimSun, serif;
  border: 1px solid #2a3344;
  border-radius: 2px;
  background: #10141f;
}

.sel__group-new:hover {
  border-color: var(--war-gold-input);
  color: var(--war-gold-bright);
}

.sel__alias-edit {
  width: 160px;
  height: 22px;
}

.sel__row {
  flex: none;
  height: 40px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 8px 0 22px; /* indent under the group header */
  user-select: none;
}

.sel__row:hover {
  background: #32509633;
}

.sel__row.selected {
  background: #1a3a6e;
}

.sel__pin {
  flex: none;
  font-size: 10px;
}

.sel__row-title {
  flex: 1;
  min-width: 0;
  color: #e8ecf4;
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sel__row.selected .sel__row-title {
  color: #ffffff;
}

.sel__row-provider {
  flex: none;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
}

.sel__row-time {
  flex: none;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
}

.sel__new-badge {
  flex: none;
  padding: 0 6px;
  border-radius: 7px;
  background: #2a5cb0;
  color: #dbe7ff;
  font-family: SimSun, serif;
  line-height: 14px;
}

.sel__rename {
  flex: 1;
  height: 24px;
}

.sel__empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  text-align: center;
  white-space: pre-line;
  color: #5a6472;
  font-family: SimSun, serif;
}

/* ---- right summary ---- */
.sel__summary {
  height: 100%;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 4px 2px;
  overflow: hidden;
}

.sel__summary-title {
  flex: none;
  color: var(--war-gold);
  font-weight: bold;
  font-family: SimSun, serif;
  overflow-wrap: break-word;
}

.sel__summary-grid {
  flex: none;
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 4px 10px;
}

.sel__summary-grid .k,
.sel__summary-abstract .k {
  color: var(--war-text-muted);
  font-family: SimSun, serif;
}

.sel__summary-grid .v,
.sel__summary-abstract .v {
  color: var(--war-text);
  font-family: SimSun, serif;
  overflow-wrap: break-word;
}

.sel__summary-abstract {
  display: flex;
  gap: 10px;
  align-items: baseline;
}

.sel__summary-hint {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  text-align: center;
  white-space: pre-line;
  color: var(--war-text-faint);
  font-family: SimSun, serif;
}

.sel__actions {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
}
</style>

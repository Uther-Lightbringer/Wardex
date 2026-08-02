<script setup lang="ts">
// Left session rail (features/chat.md §5): sessions of the current project
// bucketed into rail GROUPS — the built-in "默认会话" (not stored, not
// deletable, always first) plus user groups from groups.json (per-project).
//
// Group rendering: each group is a header row (collapse toggle, session
// count, ＋ new-session, right-click rename/delete) followed by its session
// tree. Sessions render under the group of their TOP-LEVEL ancestor, so
// moving any node moves the whole sub-session subtree. Sub-session trees
// stay collapsed by default (expandedIds empty); opening a parent
// auto-expands its subtree, opening a child reveals its ancestor chain (the
// active row is always visible via isAncestorOfActive — stateless, so manual
// ▸/▾ collapses survive rail refreshes). Parent rows carry a gold
// child-count badge.
//
// Drag-to-group: press-and-hold (or drag beyond 8px) on a row starts a drag
// with a floating ghost; the group header under the cursor glows green; on
// release the session moves to that group. Quick clicks keep the normal
// open behavior (the click is resolved in pointerup, not @click, so a
// drag can never be mistaken for a click).
//
// Right-click menus: session rows (pin / rename / copy / ask / jump-parent /
// delete with confirm), group headers (rename / delete with confirm).
// Status dots (running green / waiting gold / idle gray, 550ms breathing),
// unread "NEW ·" prefix, instant title filter, WC3 scrollbar, draggable rail
// width handle (green on hover; double-click resets to 240).
import { computed, ref, watch } from 'vue';
import { useChatStore } from '../../stores/chat';
import { useSessionsStore, type RailSession } from '../../stores/sessions';
import { usePrefsStore } from '../../stores/prefs';
import WarMenu, { type WarMenuItem } from '../war/WarMenu.vue';
import WarDialog from '../war/WarDialog.vue';
import WarButton from '../war/WarButton.vue';
import WarScrollBar from '../war/WarScrollBar.vue';

const chat = useChatStore();
const sessions = useSessionsStore();
const prefs = usePrefsStore();

const filter = ref('');
/** 最深可视嵌套：数据不限深度，视觉压平到 3 级。 */
const MAX_DEPTH = 3;
/** WC3 滚动条目标（rail 列表）。 */
const listEl = ref<HTMLElement | null>(null);

// ---- rail width drag (right edge handle; persisted once on release) ----
// 手柄在滚动条左侧：拖拽中只改本地 prefs.railWidth（不落盘），松手才
// setRailWidth 持久化；hover / 拖动中手柄变绿。双击恢复默认 240。
const RAIL_MIN = 180;
const RAIL_MAX = 340;
const RAIL_DEFAULT = 240;
const railDrag = ref(false);
let dragStartX = 0;
let dragStartW = 0;

function onResizeDown(e: PointerEvent): void {
  railDrag.value = true;
  dragStartX = e.clientX;
  dragStartW = prefs.railWidth;
  e.preventDefault();
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onResizeMove(e: PointerEvent): void {
  if (!railDrag.value) return;
  const w = Math.round(Math.min(RAIL_MAX, Math.max(RAIL_MIN, dragStartW + (e.clientX - dragStartX))));
  if (w !== prefs.railWidth) prefs.setRailWidthLocal(w);
}

function onResizeUp(): void {
  if (!railDrag.value) return;
  railDrag.value = false;
  void prefs.setRailWidth(prefs.railWidth);
}

function onResizeReset(): void {
  prefs.setRailWidthLocal(RAIL_DEFAULT);
  void prefs.setRailWidth(RAIL_DEFAULT);
}

// ---- groups ----
interface RailNode {
  s: RailSession;
  depth: number;
}

/** 组头列表：默认分组 + 当前项目的自定义组。 */
const groupHeaders = computed(() => [
  { id: '', name: '默认分组' },
  ...sessions.groups.map((g) => ({ id: g.id, name: g.name })),
]);

/** 会话的归属组 = 其顶层祖先的 groupId（'' = 默认组）。 */
function rootGroupId(s: RailSession): string {
  let cur = s;
  const seen = new Set<string>();
  while (cur.parentId && !seen.has(cur.sessionId)) {
    seen.add(cur.sessionId);
    const p = railById(cur.parentId);
    if (!p) break;
    cur = p;
  }
  return cur.groupId || '';
}

function railById(id: string): RailSession | undefined {
  return sessions.rail.find((s) => s.sessionId === id);
}

/** 当前会话的祖先链（stateless）：即使手动收起，活动行永远可见。 */
function isAncestorOfActive(id: string): boolean {
  let cur = chat.sessionId ? railById(chat.sessionId) : undefined;
  while (cur?.parentId) {
    if (cur.parentId === id) return true;
    cur = railById(cur.parentId);
  }
  return false;
}

/** 直系子会话数（徽标与折叠钮共用）。 */
const childCounts = computed(() => {
  const m = new Map<string, number>();
  for (const s of sessions.rail) {
    if (!s.parentId) continue;
    m.set(s.parentId, (m.get(s.parentId) ?? 0) + 1);
  }
  return m;
});
function childCount(id: string): number {
  return childCounts.value.get(id) ?? 0;
}

/** 组内会话总数（顶层 + 整棵子树）。 */
function groupSessionCount(gid: string): number {
  return sessions.rail.filter((s) => rootGroupId(s) === gid).length;
}

// ---- session tree per group ----
function treeOfGroup(gid: string): RailNode[] {
  const f = filter.value.trim().toLowerCase();
  const byParent = new Map<string, RailSession[]>();
  for (const s of sessions.rail) {
    const k = s.parentId || '';
    const list = byParent.get(k);
    if (list) list.push(s);
    else byParent.set(k, [s]);
  }
  const kidsOf = (id: string): RailSession[] =>
    (byParent.get(id) ?? []).slice().sort((a, b) => a.createdAt - b.createdAt);

  const out: RailNode[] = [];
  const walk = (pid: string, depth: number): void => {
    for (const s of kidsOf(pid)) {
      if (f && !s.title.toLowerCase().includes(f)) continue;
      out.push({ s, depth: Math.min(depth, MAX_DEPTH) });
      if (expandedIds.value.has(s.sessionId) || isAncestorOfActive(s.sessionId)) {
        walk(s.sessionId, depth + 1);
      }
    }
  };
  // Top level: keep the rail order (pinned first, updatedAt desc); sessions
  // whose parent is missing (deleted/orphaned meta) float up as top-level.
  for (const s of sessions.rail) {
    if (rootGroupId(s) !== gid) continue;
    if (s.parentId && sessions.rail.some((p) => p.sessionId === s.parentId)) continue;
    if (f && !s.title.toLowerCase().includes(f)) continue;
    out.push({ s, depth: 0 });
    if (expandedIds.value.has(s.sessionId) || isAncestorOfActive(s.sessionId)) {
      walk(s.sessionId, 1);
    }
  }
  return out;
}

// ---- sub-session expand/collapse ----
/** 手动展开的父会话集合；默认空 = 全部收起。 */
const expandedIds = ref(new Set<string>());

function isExpanded(id: string): boolean {
  return expandedIds.value.has(id) || isAncestorOfActive(id);
}

function toggleExpanded(id: string): void {
  const set = new Set(expandedIds.value);
  if (set.has(id)) set.delete(id);
  else set.add(id);
  expandedIds.value = set;
}

// 打开（切到）一个会话 → 展开它的整棵子树（父会话的子会话自动可见）。
watch(
  () => chat.sessionId,
  (id) => {
    if (!id) return;
    const set = new Set(expandedIds.value);
    const dive = (pid: string): void => {
      for (const k of sessions.rail.filter((s) => s.parentId === pid)) {
        set.add(k.sessionId);
        dive(k.sessionId);
      }
    };
    dive(id);
    expandedIds.value = set;
  },
  { immediate: true },
);

// ---- group expand/collapse (runtime only, like the project groups) ----
const collapsedGroups = ref(new Set<string>());

function groupCollapsed(gid: string): boolean {
  return collapsedGroups.value.has(gid);
}

function toggleGroup(gid: string): void {
  const set = new Set(collapsedGroups.value);
  if (set.has(gid)) set.delete(gid);
  else set.add(gid);
  collapsedGroups.value = set;
}

// ---- drag a session row into a group (press-and-hold, or move >8px) ----
const LONG_PRESS_MS = 400;
const dragGhost = ref<{ x: number; y: number; title: string } | null>(null);
const hoverGroupId = ref<string | null>(null);
let dragState: { x: number; y: number; s: RailSession } | null = null;
let dragTimer: ReturnType<typeof setTimeout> | null = null;
let dragging = false;

function onRowDown(e: PointerEvent, s: RailSession): void {
  if (renamingId.value) return;
  if (e.button !== 0) return;
  const t = e.target as HTMLElement;
  // 交互子元素不参与拖拽（折叠钮 / 徽标 / 重命名输入 / 宽度手柄）
  if (t.closest('.rail__tgl, .rail__badge, .rail__rename, .rail__resize')) return;
  dragState = { x: e.clientX, y: e.clientY, s };
  dragging = false;
  hoverGroupId.value = null;
  if (dragTimer) clearTimeout(dragTimer);
  dragTimer = setTimeout(() => enterDrag(), LONG_PRESS_MS);
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onRowMove(e: PointerEvent): void {
  if (!dragState) return;
  if (!dragging) {
    const dx = e.clientX - dragState.x;
    const dy = e.clientY - dragState.y;
    if (Math.abs(dx) > 8 || Math.abs(dy) > 8) {
      if (dragTimer) {
        clearTimeout(dragTimer);
        dragTimer = null;
      }
      enterDrag();
    }
    return;
  }
  dragGhost.value = { x: e.clientX + 12, y: e.clientY + 12, title: dragState.s.title };
  const el = document.elementFromPoint(e.clientX, e.clientY);
  const g = el?.closest<HTMLElement>('.rail__group');
  hoverGroupId.value = g ? (g.dataset.gid ?? null) : null;
}

function onRowUp(): void {
  if (dragTimer) {
    clearTimeout(dragTimer);
    dragTimer = null;
  }
  const st = dragState;
  dragState = null;
  if (!st) return;
  if (dragging) {
    // drop: only a real group header accepts the move (blank = cancel)
    const gid = hoverGroupId.value;
    dragging = false;
    hoverGroupId.value = null;
    dragGhost.value = null;
    document.body.classList.remove('rail-dragging');
    if (gid !== null) {
      const cur = rootGroupId(st.s);
      if (cur !== gid) void chat.moveSessionGroup(st.s.sessionId, gid);
    }
    return;
  }
  // Quick click (no hold, no movement) → normal open.
  onPick(st.s);
}

function enterDrag(): void {
  if (!dragState || dragging) return;
  dragging = true;
  document.body.classList.add('rail-dragging');
  dragGhost.value = {
    x: dragState.x + 12,
    y: dragState.y + 12,
    title: dragState.s.title,
  };
}

function cancelDrag(): void {
  if (dragTimer) {
    clearTimeout(dragTimer);
    dragTimer = null;
  }
  dragState = null;
  dragging = false;
  hoverGroupId.value = null;
  dragGhost.value = null;
  document.body.classList.remove('rail-dragging');
}

function subLine(s: RailSession): string {
  const dot = sessions.dotState(s.sessionId);
  const unread = sessions.unreadIds.includes(s.sessionId);
  let line = `${s.messageCount} 条`;
  if (dot === 'running') line += ' · 执行中';
  if (dot === 'waiting') line += ' · 等待确认';
  if (unread) line = 'NEW · ' + line;
  return line;
}

function onPick(s: RailSession): void {
  if (s.sessionId === chat.sessionId || renamingId.value) return;
  // Deferred a tick (old code: 延迟一拍执行避免事件栈内切页).
  void Promise.resolve().then(() => chat.openSession(s.sessionId));
}

// ---- group actions ----
function newInGroup(gid: string): void {
  void chat.newSession(gid);
}

const newGroupOpen = ref(false);
const newGroupName = ref('');

function startNewGroup(): void {
  newGroupOpen.value = true;
  newGroupName.value = '';
}

async function commitNewGroup(): Promise<void> {
  // 防重入：提交后 input 卸载会再派发一次 blur，重复提交会建出两个组。
  if (!newGroupOpen.value) return;
  newGroupOpen.value = false;
  const n = newGroupName.value.trim();
  newGroupName.value = '';
  if (!n) return;
  await sessions.createGroup(chat.projectDir, n);
}

function cancelNewGroup(): void {
  newGroupOpen.value = false;
}

const renamingGroupId = ref<string | null>(null);
const groupRenameText = ref('');

async function commitGroupRename(): Promise<void> {
  const id = renamingGroupId.value;
  const n = groupRenameText.value.trim().slice(0, 48);
  renamingGroupId.value = null;
  if (!id || !n) return;
  await sessions.renameGroup(id, n, chat.projectDir);
}

// ---- group context menu (default group has none) ----
const groupMenuVisible = ref(false);
const groupMenuX = ref(0);
const groupMenuY = ref(0);
const groupMenu = ref<{ id: string; name: string; count: number } | null>(null);

const groupMenuItems = computed<WarMenuItem[]>(() => {
  const g = groupMenu.value;
  if (!g) return [];
  return [{ label: '重命名组' }, { label: `删除组（${g.count} 条会话）` }];
});

function onGroupContextMenu(e: MouseEvent, g: { id: string; name: string }): void {
  if (!g.id) return; // the default group is not deletable / renamable
  groupMenu.value = { id: g.id, name: g.name, count: groupSessionCount(g.id) };
  groupMenuX.value = e.clientX;
  groupMenuY.value = e.clientY;
  groupMenuVisible.value = true;
}

function onGroupMenuSelect(i: number): void {
  const g = groupMenu.value;
  if (!g) return;
  if (i === 0) {
    renamingGroupId.value = g.id;
    groupRenameText.value = g.name;
  } else if (i === 1) {
    groupDeleteTarget.value = g;
  }
}

// ---- group delete confirm (cascade) ----
const groupDeleteTarget = ref<{ id: string; name: string; count: number } | null>(null);
const groupDeleteOpen = computed({
  get: () => groupDeleteTarget.value !== null,
  set: (v: boolean) => {
    if (!v) groupDeleteTarget.value = null;
  },
});

async function confirmGroupDelete(): Promise<void> {
  const g = groupDeleteTarget.value;
  groupDeleteTarget.value = null;
  if (!g) return;
  await sessions.deleteGroup(g.id, chat.projectDir);
  // 若当前会话落在被删组里（后端已清 active），切到剩下的会话。
  await sessions.refresh(chat.projectDir);
  if (chat.sessionId && !sessions.rail.some((r) => r.sessionId === chat.sessionId)) {
    const next = sessions.rail[0]?.sessionId ?? '';
    if (next) void chat.openSession(next);
    else {
      chat.sessionId = '';
      chat.meta = null;
      chat.rows = [];
    }
  }
}

// ---- session context menu ----
const menuVisible = ref(false);
const menuX = ref(0);
const menuY = ref(0);
const menuSession = ref<RailSession | null>(null);

const menuItems = computed<WarMenuItem[]>(() => {
  const s = menuSession.value;
  if (!s) return [];
  const items: WarMenuItem[] = [
    { label: s.pinned ? '取消置顶' : '置顶会话' },
    { label: '重命名会话' },
    { label: '复制会话内容' },
    { label: '基于此提问' },
  ];
  if (s.parentId) items.push({ label: '跳转父会话' });
  items.push({ label: '删除会话' });
  return items;
});

function onContextMenu(e: MouseEvent, s: RailSession): void {
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
      void sessions.setPinned(s.sessionId, !s.pinned).then(() => sessions.refresh(chat.projectDir));
      break;
    case 1:
      renamingId.value = s.sessionId;
      renameText.value = s.title;
      break;
    case 2:
      void sessions.copyTranscript(s.sessionId).then((err) => {
        if (err) chat.status = { ...chat.status, lastError: err };
      });
      break;
    case 3:
      // New empty session in the same project + composer prefill.
      sessions.pendingComposerText = `基于会话「${s.title}」：`;
      void chat.newSession();
      break;
    case 4:
      // Child rows: 跳转父会话; top-level rows: 删除会话.
      if (s.parentId) void chat.openSession(s.parentId);
      else deleteTarget.value = s;
      break;
    case 5:
      deleteTarget.value = s;
      break;
  }
}

// ---- inline rename (Enter submits non-empty trim ≤48 chars; Esc cancels) ----
const renamingId = ref('');
const renameText = ref('');

async function commitRename(): Promise<void> {
  const id = renamingId.value;
  const title = renameText.value.trim().slice(0, 48);
  renamingId.value = '';
  if (!id || !title) return;
  try {
    await sessions.rename(id, title);
    await sessions.refresh(chat.projectDir);
    await chat.refreshMeta();
  } catch (e) {
    console.warn('[rail] rename failed', e);
  }
}

function cancelRename(): void {
  renamingId.value = '';
}

// ---- delete confirm ----
const deleteTarget = ref<RailSession | null>(null);
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
  await chat.deleteSession(s.sessionId);
}

// Esc while renaming belongs to the input, not the page-back shortcut.
defineExpose({ renaming: renamingId });
</script>

<template>
  <div class="rail">
    <div class="rail__title war-font-title war-outline-black" :style="{ fontSize: prefs.fs(15) + 'px' }">
      本项目会话
    </div>

    <div class="rail__new-row">
      <button class="rail__new" :style="{ fontSize: prefs.fs(12) + 'px' }" @click="chat.newSession()">
        ＋ 新会话
      </button>
      <button class="rail__new rail__new--group" :style="{ fontSize: prefs.fs(12) + 'px' }" @click="startNewGroup()">
        ＋ 组
      </button>
    </div>

    <input
      v-model="filter"
      class="war-input rail__search"
      placeholder="搜索会话…"
      :style="{ fontSize: prefs.fs(12) + 'px' }"
    />

    <div class="rail__body">
      <div ref="listEl" class="rail__list">
        <!-- new-group inline input -->
        <input
          v-if="newGroupOpen"
          v-model="newGroupName"
          class="rail__rename rail__gname-input"
          placeholder="组名…（Enter 确认）"
          :style="{ fontSize: prefs.fs(12) + 'px' }"
          maxlength="48"
          v-focus
          @keydown.enter.prevent="commitNewGroup"
          @keydown.esc.stop.prevent="cancelNewGroup"
          @blur="commitNewGroup"
        />

        <template v-for="g in groupHeaders" :key="g.id">
          <!-- group header -->
          <div
            class="rail__group"
            :class="{ hover: hoverGroupId === g.id }"
            :data-gid="g.id"
            @click="toggleGroup(g.id)"
            @contextmenu.prevent="onGroupContextMenu($event, g)"
          >
            <span class="rail__gtgl" :style="{ fontSize: prefs.fs(10) + 'px' }">
              {{ groupCollapsed(g.id) ? '▸' : '▾' }}
            </span>
            <template v-if="renamingGroupId === g.id">
              <input
                v-model="groupRenameText"
                class="rail__rename rail__gname-input"
                :style="{ fontSize: prefs.fs(12) + 'px' }"
                maxlength="48"
                v-focus
                @keydown.enter.prevent="commitGroupRename"
                @keydown.esc.stop.prevent="renamingGroupId = null"
                @click.stop
                @blur="commitGroupRename"
              />
            </template>
            <template v-else>
              <span class="rail__gname" :style="{ fontSize: prefs.fs(12) + 'px' }">{{ g.name }}</span>
              <span class="rail__gcnt" :style="{ fontSize: prefs.fs(10) + 'px' }">{{ groupSessionCount(g.id) }}</span>
            </template>
            <span
              class="rail__gnew"
              :style="{ fontSize: prefs.fs(12) + 'px' }"
              title="在此组新建会话"
              @click.stop="newInGroup(g.id)"
              >＋</span
            >
          </div>

          <!-- session rows of this group -->
          <div v-if="!groupCollapsed(g.id)" class="rail__grows">
            <div
              v-for="item in treeOfGroup(g.id)"
              :key="item.s.sessionId"
              class="rail__row"
              :class="{
                active: item.s.sessionId === chat.sessionId,
                child: !!item.s.parentId,
                drag: dragging && dragState?.s.sessionId === item.s.sessionId,
              }"
              :style="{
                paddingLeft: item.depth === 0 ? 6 + 'px' : 8 + 'px',
                maxWidth: item.depth > 0 ? Math.pow(0.9, item.depth) * 100 + '%' : '',
              }"
              @pointerdown="onRowDown($event, item.s)"
              @pointermove="onRowMove"
              @pointerup="onRowUp"
              @pointercancel="onRowUp"
              @contextmenu.prevent="onContextMenu($event, item.s)"
            >
              <span
                v-if="childCount(item.s.sessionId) > 0"
                class="rail__tgl"
                :style="{ fontSize: prefs.fs(10) + 'px' }"
                @click.stop="toggleExpanded(item.s.sessionId)"
                >{{ isExpanded(item.s.sessionId) ? '▾' : '▸' }}</span
              >
              <span v-else class="rail__tgl rail__tgl--none">·</span>
              <span
                class="rail__dot"
                :class="[sessions.dotState(item.s.sessionId), { breath: sessions.dotState(item.s.sessionId) !== 'idle' }]"
              ></span>
              <div class="rail__text">
                <div class="rail__name" :style="{ fontSize: prefs.fs(12) + 'px' }">
                  <template v-if="renamingId === item.s.sessionId">
                    <input
                      v-model="renameText"
                      class="rail__rename"
                      :style="{ fontSize: prefs.fs(12) + 'px' }"
                      maxlength="48"
                      v-focus
                      @keydown.enter.prevent="commitRename"
                      @keydown.esc.stop.prevent="cancelRename"
                      @click.stop
                      @pointerdown.stop
                      @blur="commitRename"
                    />
                  </template>
                  <template v-else>{{ item.s.title }}</template>
                </div>
                <div class="rail__sub" :style="{ fontSize: prefs.fs(10) + 'px' }">{{ subLine(item.s) }}</div>
              </div>
              <span v-if="item.s.pinned" class="rail__pin">📌</span>
              <span
                v-if="childCount(item.s.sessionId) > 0"
                class="rail__badge"
                :class="{ open: isExpanded(item.s.sessionId) }"
                :style="{ fontSize: prefs.fs(9) + 'px' }"
                @click.stop="toggleExpanded(item.s.sessionId)"
                >{{ childCount(item.s.sessionId) }}</span
              >
            </div>
          </div>
        </template>

        <div v-if="sessions.rail.length === 0" class="rail__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">
          （无会话）
        </div>
      </div>
      <div
        class="rail__resize"
        :class="{ active: railDrag }"
        title="拖动调整宽度（双击恢复默认 240）"
        @pointerdown="onResizeDown"
        @pointermove="onResizeMove"
        @pointerup="onResizeUp"
        @pointercancel="onResizeUp"
        @dblclick="onResizeReset"
      ></div>
      <WarScrollBar :target="listEl" :scale="0.8" />
    </div>

    <div class="rail__legend" :style="{ fontSize: prefs.fs(10) + 'px' }">
      <span class="lg running">●</span> 执行中
      <span class="lg waiting">●</span> 等待
      <span class="lg idle">●</span> 空闲
    </div>

    <!-- drag ghost -->
    <Teleport to="body">
      <div v-if="dragGhost" class="rail__ghost" :style="{ left: dragGhost.x + 'px', top: dragGhost.y + 'px' }">
        <span class="rail__ghost-name" :style="{ fontSize: prefs.fs(12) + 'px' }">{{ dragGhost.title }}</span>
        <span class="rail__ghost-hint" :style="{ fontSize: prefs.fs(10) + 'px' }">拖到组头上方移入该组</span>
      </div>
    </Teleport>

    <WarMenu v-model:visible="menuVisible" :x="menuX" :y="menuY" :items="menuItems" @select="onMenuSelect" />
    <WarMenu
      v-model:visible="groupMenuVisible"
      :x="groupMenuX"
      :y="groupMenuY"
      :items="groupMenuItems"
      @select="onGroupMenuSelect"
    />

    <WarDialog
      v-model:open="deleteOpen"
      title-text="删除会话"
      :message-text="'确定删除这条会话及其全部消息吗？\n它的全部子会话也将一并删除。\n该操作不可撤销。'"
    >
      <WarButton skin="dialog" :width="190" text="删除" @activated="confirmDelete" />
      <WarButton skin="dialog" :width="190" text="取消" @activated="deleteTarget = null" />
    </WarDialog>

    <WarDialog
      v-model:open="groupDeleteOpen"
      title-text="删除组"
      :message-text="
        groupDeleteTarget
          ? `确定删除组「${groupDeleteTarget.name}」吗？\n组内 ${groupDeleteTarget.count} 条会话（含子会话）将一并删除。\n该操作不可撤销。`
          : ''
      "
    >
      <WarButton skin="dialog" :width="190" text="删除" @activated="confirmGroupDelete" />
      <WarButton skin="dialog" :width="190" text="取消" @activated="groupDeleteTarget = null" />
    </WarDialog>
  </div>
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
.rail {
  display: flex;
  flex-direction: column;
  gap: 8px;
  height: 100%;
  min-height: 0;
  font-family: SimSun, serif;
  padding: 10px 6px; /* tighter than 8px so rows keep more width for the tree */
  box-sizing: border-box;
}

.rail__title {
  flex: none;
  color: var(--war-text-dim);
  text-align: center;
}

.rail__new-row {
  flex: none;
  display: flex;
  gap: 6px;
}

.rail__new {
  flex: 1;
  height: 28px;
  background: #10141f;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: var(--war-gold);
  font-family: SimSun, serif;
}

.rail__new:hover {
  border-color: var(--war-gold-input);
  color: var(--war-gold-bright);
}

.rail__search {
  flex: none;
  height: 28px;
}

.rail__body {
  flex: 1;
  min-height: 0;
  display: flex;
  gap: 4px;
}

.rail__list {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.rail__empty {
  color: var(--war-text-faint);
  text-align: center;
  padding: 12px 0;
}

/* ---- group header ---- */
.rail__group {
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 4px;
  padding: 4px 4px 4px 2px;
  border: 1px solid #3a4252;
  border-radius: 2px;
  background: #10141f88;
  user-select: none;
}

.rail__group:first-child {
  margin-top: 0;
}

.rail__group:hover {
  background: #32509633;
}

.rail__group.hover {
  border-color: #5cb380;
  background: #173a22;
  box-shadow: 0 0 6px #5cb38055;
}

.rail__gtgl {
  flex: none;
  width: 12px;
  text-align: center;
  color: var(--war-gold);
}

.rail__gname {
  flex: 1;
  min-width: 0;
  color: var(--war-gold);
  font-weight: bold;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.rail__gcnt {
  flex: none;
  color: var(--war-text-faint);
}

.rail__gnew {
  flex: none;
  width: 18px;
  text-align: center;
  color: var(--war-gold);
  border: 1px solid #2a3344;
  border-radius: 2px;
  line-height: 16px;
}

.rail__gnew:hover {
  color: var(--war-gold-bright);
  border-color: var(--war-gold-input);
}

.rail__gname-input {
  height: 22px;
}

/* ---- session rows ---- */
.rail__row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 6px;
  border: 1px solid #0a0c10;
  border-radius: 2px;
  user-select: none;
}

.rail__row:hover {
  background: #32509640;
  border-color: #4a3c14;
}

.rail__row.active {
  border-color: #8a6f24;
}

.rail__row.active .rail__name {
  color: var(--war-gold);
  /* 不用 bold：fit-content 块会随加粗变宽（选中时块变长） */
}

/* 组内行包装层：flex column，让行的 align-self 对齐生效 */
.rail__grows {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

/* 子会话块：内容宽度（上限随深度递减）、右端贴右缘，层级由块内 ↳ 表达 */
.rail__row.child {
  position: relative;
  align-self: flex-end;
  width: fit-content;
  max-width: 100%;
  border-color: #2a3344;
  /* 右侧固定预留徽标位：徽标出现/消失都不改变 fit-content 宽度，
     否则子块会随徽标增减而向左侧生长（右缘不动） */
  padding-right: 20px;
}

/* 子会话计数徽标：绝对定位进预留区，不参与块宽计算 */
.rail__row.child .rail__badge {
  position: absolute;
  right: 6px;
  top: 50%;
  transform: translateY(-50%);
}

.rail__row.drag {
  opacity: 0.35;
}

.rail__dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
}

.rail__dot.running {
  background: #57d977;
}

.rail__dot.waiting {
  background: #f2cf6b;
}

.rail__dot.idle {
  background: #4a5265;
}

.rail__dot.breath {
  animation: breath 550ms ease-in-out infinite alternate;
}

/* ---- sub-session tree: toggle arrow + derived marker + child-count badge ---- */
.rail__tgl {
  flex: none;
  width: 12px;
  text-align: center;
  color: var(--war-gold);
  user-select: none;
}

.rail__tgl--none {
  color: #3a4252;
}

.rail__badge {
  flex: none;
  min-width: 15px;
  padding: 0 3px;
  text-align: center;
  line-height: 14px;
  border: 1px solid var(--war-gold-dim);
  border-radius: 8px;
  color: var(--war-gold);
  background: #0d1116;
  user-select: none;
}

.rail__badge:hover {
  color: var(--war-gold-bright);
  border-color: var(--war-gold);
}

.rail__badge.open {
  background: #1a1f16;
}

/* ---- drag ghost ---- */
.rail__ghost {
  position: fixed;
  z-index: 200;
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-width: 220px;
  padding: 6px 10px;
  border: 1px solid #5cb380;
  border-radius: 3px;
  background: #0d1116f2;
  box-shadow: 0 6px 18px #000a;
  pointer-events: none;
}

.rail__ghost-name {
  color: var(--war-gold);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.rail__ghost-hint {
  color: var(--war-text-faint);
}

/* ---- rail width drag handle: sits between the list and the scrollbar,
   turns green when hovered / dragging ---- */
.rail__resize {
  flex: none;
  width: 6px;
  align-self: stretch;
  cursor: col-resize;
  border-radius: 3px;
  user-select: none;
  touch-action: none;
}

.rail__resize:hover,
.rail__resize.active {
  background: #5cb380;
  box-shadow: 0 0 4px #5cb38088;
}

@keyframes breath {
  from {
    opacity: 0.35;
  }
  to {
    opacity: 1;
  }
}

.rail__text {
  flex: 1;
  min-width: 0;
}

.rail__name {
  color: var(--war-text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.rail__rename {
  width: 100%;
  background: #10141f;
  border: 1px solid #8a6f24;
  border-radius: 2px;
  color: var(--war-text);
  font-family: SimSun, serif;
  padding: 1px 4px;
  outline: none;
}

.rail__sub {
  color: var(--war-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.rail__pin {
  flex: none;
  font-size: 10px;
}

.rail__legend {
  flex: none;
  color: var(--war-text-muted);
  text-align: center;
  user-select: none;
}

.rail__legend .lg {
  margin-left: 6px;
}

.rail__legend .lg:first-child {
  margin-left: 0;
}

.lg.running {
  color: #57d977;
}

.lg.waiting {
  color: #f2cf6b;
}

.lg.idle {
  color: #4a5265;
}
</style>

<style>
/* 拖拽中禁文本选择（body 级；teleport 的 ghost 不受 scoped 影响） */
body.rail-dragging {
  user-select: none;
}
</style>

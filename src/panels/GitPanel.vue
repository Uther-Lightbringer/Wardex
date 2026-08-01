<script setup lang="ts">
// Version-control panel (features/chat.md §6.3, panels.md §2): branch badge
// read from .git/HEAD (hidden entirely outside git repos), a working-tree
// 更改 list (git_status) and a read-only commit list (git_log, 200-entry
// cap). 更改 rows open an inline read-only diff (git_diff_file); commit rows
// open the GitLab-style GitCommitDialog (git_diff_commit — file list + per-
// file diff, R4 64KB-capped with a truncated marker). No stage/unstage.
// refreshOn: turnEnd | sessionSwitch | expand | manual — the agent may have
// committed or switched branches during the turn.
import { computed, onMounted, ref, watch } from 'vue';
import { cmd } from '../lib/tauri';
import { useChatStore } from '../stores/chat';
import { usePrefsStore } from '../stores/prefs';
import GitCommitDialog from './GitCommitDialog.vue';
import WarScrollBar from '../components/war/WarScrollBar.vue';
import type { GitCommit, GitDiff, GitStatusEntry } from './git-types';

/** What the open diff view shows; re-fetched on refresh (generation-guarded).
 * kind 'file' renders inline (更改 tab); kind 'commit' opens the
 * GitLab-style GitCommitDialog. */
type DiffSpec =
  | { kind: 'file'; path: string; mode: 'worktree' | 'staged' | 'untracked'; label: string }
  | { kind: 'commit'; hash: string; subject: string; meta: string };

const chat = useChatStore();
const prefs = usePrefsStore();

const branch = ref('');
const commits = ref<GitCommit[]>([]);
const changes = ref<GitStatusEntry[]>([]);
const tab = ref<'changes' | 'history'>('changes');
const loading = ref(false);
const error = ref('');

const diffSpec = ref<DiffSpec | null>(null);
const diff = ref<GitDiff | null>(null);
const diffLoading = ref(false);
const diffError = ref('');

// Generation guard: rapid refreshes never stack stale responses (panels.md
// §4 checklist: refreshOn triggers need generation invalidation).
let generation = 0;

// ---- scroll targets (WC3 WarScrollBar) ----
const listEl = ref<HTMLElement | null>(null);
const diffEl = ref<HTMLElement | null>(null);

const workDir = computed(() => chat.meta?.workDir || chat.meta?.projectDir || chat.projectDir);

// Empty-state copy: guide project-less sessions toward opening a project
// (docs/features/chat.md §6 decision: A — guide text instead of a bare label).
const noRepoText = computed(() =>
  chat.meta?.projectDir ? '（非 Git 目录）' : '（非 Git 目录——从主菜单打开项目后可用）',
);

/** Porcelain columns → which diff command mode shows this entry's changes. */
function diffMode(e: GitStatusEntry): 'worktree' | 'staged' | 'untracked' {
  if (e.index === '?') return 'untracked';
  return e.worktree !== ' ' ? 'worktree' : 'staged';
}

function statusBadge(e: GitStatusEntry): string {
  if (e.index === '?') return '?';
  const a = e.index === ' ' ? '' : e.index;
  const b = e.worktree === ' ' ? '' : e.worktree;
  return a + b;
}

async function openDiff(spec: DiffSpec): Promise<void> {
  diffSpec.value = spec;
  diff.value = null;
  diffError.value = '';
  const gen = ++generation;
  const dir = workDir.value;
  if (!dir) return;
  diffLoading.value = true;
  try {
    diff.value =
      spec.kind === 'file'
        ? await cmd<GitDiff>('git_diff_file', { dir, path: spec.path, mode: spec.mode })
        : await cmd<GitDiff>('git_diff_commit', { dir, hash: spec.hash });
    if (gen !== generation) return; // superseded by a newer fetch
  } catch (e) {
    if (gen === generation) diffError.value = String(e);
  } finally {
    if (gen === generation) diffLoading.value = false;
  }
}

function closeDiff(): void {
  generation++; // invalidate any in-flight diff fetch
  diffSpec.value = null;
  diff.value = null;
  diffError.value = '';
}

async function refresh(): Promise<void> {
  const dir = workDir.value;
  if (!dir) {
    branch.value = '';
    commits.value = [];
    changes.value = [];
    closeDiff();
    return;
  }
  try {
    branch.value = await cmd<string>('git_branch', { dir }, '');
  } catch {
    branch.value = '';
  }
  if (!branch.value) {
    commits.value = [];
    changes.value = [];
    error.value = '';
    closeDiff();
    return;
  }
  loading.value = true;
  error.value = '';
  try {
    const [log, status] = await Promise.all([
      cmd<GitCommit[]>('git_log', { dir }, []),
      cmd<GitStatusEntry[]>('git_status', { dir }, []),
    ]);
    commits.value = log;
    changes.value = status;
  } catch (e) {
    commits.value = [];
    changes.value = [];
    error.value = String(e);
  } finally {
    loading.value = false;
  }
  // An open diff view tracks the refresh (files may have changed mid-turn).
  if (diffSpec.value) void openDiff(diffSpec.value);
}

onMounted(refresh);
watch(() => chat.sessionId, refresh); // sessionSwitch
watch(() => chat.turnSeq, refresh); // turnEnd
// meta loads async after the panel mounts on first project open; without
// this the badge stays "（非 Git 目录）" until the first turn bumps turnSeq.
watch(workDir, refresh);
watch(() => chat.workspaceRefreshSeq, refresh); // 刷新工作区 button
</script>

<template>
  <div class="gitp">
    <template v-if="branch">
      <div class="gitp__badge-row">
        <span class="gitp__badge" :style="{ fontSize: prefs.fs(11) + 'px' }" :title="branch">
          ⎇ Git {{ branch }}
        </span>
        <span class="gitp__refresh" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="refresh">
          {{ loading ? '加载中…' : '刷新' }}
        </span>
      </div>

      <!-- diff view (replaces the lists while open; file diffs only — commits
           open the GitCommitDialog instead) -->
      <template v-if="diffSpec && diffSpec.kind === 'file'">
        <div class="gitp__diff-head">
          <span class="gitp__back" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="closeDiff">← 返回</span>
          <span class="gitp__diff-title" :style="{ fontSize: prefs.fs(11) + 'px' }" :title="diffSpec.label">
            {{ diffSpec.label }}
          </span>
        </div>
        <div class="gitp__diff-wrap">
          <div ref="diffEl" class="gitp__diff">
          <div v-if="diffError" class="gitp__error" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ diffError }}</div>
          <div v-else-if="diffLoading" class="gitp__empty" :style="{ fontSize: prefs.fs(11) + 'px' }">加载中…</div>
          <template v-else-if="diff">
            <div v-if="diff.files.length === 0" class="gitp__empty" :style="{ fontSize: prefs.fs(11) + 'px' }">
              （无差异）
            </div>
            <div v-for="f in diff.files" :key="f.path" class="gitp__file">
              <div class="gitp__file-name" :style="{ fontSize: prefs.fs(11) + 'px' }" :title="f.path">
                {{ f.path }}<template v-if="f.binary">（二进制）</template>
              </div>
              <div
                v-for="(l, i) in f.lines"
                :key="i"
                class="gitp__line"
                :class="`gitp__line--${l.kind}`"
                :style="{ fontSize: prefs.fs(11) + 'px' }"
              >
                <template v-if="l.kind === 'add' || l.kind === 'del' || l.kind === 'ctx'">
                  <span class="gitp__ln">{{ l.old_lineno ?? '' }}</span>
                  <span class="gitp__ln">{{ l.new_lineno ?? '' }}</span>
                  <span class="gitp__sign">{{ l.kind === 'add' ? '+' : l.kind === 'del' ? '-' : ' ' }}</span>
                  <span class="gitp__code">{{ l.text }}</span>
                </template>
                <template v-else>
                  <span class="gitp__code">{{ l.text }}</span>
                </template>
              </div>
            </div>
            <div v-if="diff.truncated" class="gitp__trunc" :style="{ fontSize: prefs.fs(11) + 'px' }">
              …（内容超过 64KB，已截断）
            </div>
          </template>
          </div>
          <WarScrollBar :target="diffEl" />
        </div>
      </template>

      <!-- changes / history lists -->
      <template v-else>
        <div class="gitp__tabs">
          <span
            class="gitp__tab"
            :class="{ active: tab === 'changes' }"
            :style="{ fontSize: prefs.fs(11) + 'px' }"
            @click="tab = 'changes'"
          >
            更改<template v-if="changes.length">（{{ changes.length }}）</template>
          </span>
          <span
            class="gitp__tab"
            :class="{ active: tab === 'history' }"
            :style="{ fontSize: prefs.fs(11) + 'px' }"
            @click="tab = 'history'"
          >
            历史
          </span>
        </div>
        <div class="gitp__list-wrap">
          <div ref="listEl" class="gitp__list">
            <div v-if="error" class="gitp__error" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ error }}</div>
          <template v-else-if="tab === 'changes'">
            <div v-if="!loading && changes.length === 0" class="gitp__empty" :style="{ fontSize: prefs.fs(11) + 'px' }">
              工作区干净
            </div>
            <div
              v-for="c in changes"
              :key="c.index + c.worktree + c.path"
              class="gitp__change-row"
              @click="openDiff({ kind: 'file', path: c.path, mode: diffMode(c), label: c.path })"
            >
              <span class="gitp__st" :class="{ 'gitp__st--new': c.index === '?' }" :style="{ fontSize: prefs.fs(10) + 'px' }">
                {{ statusBadge(c) }}
              </span>
              <span class="gitp__path" :style="{ fontSize: prefs.fs(12) + 'px' }" :title="c.orig_path ? `${c.orig_path} → ${c.path}` : c.path">
                {{ c.orig_path ? `${c.orig_path} → ${c.path}` : c.path }}
              </span>
              <span v-if="c.index !== ' ' && c.index !== '?'" class="gitp__staged" :style="{ fontSize: prefs.fs(10) + 'px' }">
                已暂存
              </span>
            </div>
          </template>
          <template v-else>
            <div v-if="!loading && commits.length === 0" class="gitp__empty" :style="{ fontSize: prefs.fs(11) + 'px' }">
              暂无提交
            </div>
            <div
              v-for="c in commits"
              :key="c.hash"
              class="gitp__row"
              @click="openDiff({ kind: 'commit', hash: c.hash, subject: c.subject, meta: `${c.short} · ${c.author} · ${c.date}` })"
            >
              <div class="gitp__subject" :style="{ fontSize: prefs.fs(12) + 'px' }" :title="c.subject">
                {{ c.subject }}
              </div>
              <div class="gitp__meta" :style="{ fontSize: prefs.fs(10) + 'px' }">
                {{ c.short }} · {{ c.author }} · {{ c.date }}
              </div>
            </div>
          </template>
          </div>
          <WarScrollBar :target="listEl" />
        </div>
      </template>
    </template>
    <div v-else class="gitp__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">{{ noRepoText }}</div>

    <!-- commit detail dialog (GitLab-style file list + per-file diff) -->
    <GitCommitDialog
      :open="diffSpec?.kind === 'commit'"
      :subject="diffSpec?.kind === 'commit' ? diffSpec.subject : ''"
      :meta="diffSpec?.kind === 'commit' ? diffSpec.meta : ''"
      :diff="diff"
      :loading="diffLoading"
      :error="diffError"
      @update:open="(v) => !v && closeDiff()"
    />
  </div>
</template>

<style scoped>
.gitp {
  display: flex;
  flex-direction: column;
  gap: 6px;
  height: 100%;
  min-height: 0;
  font-family: SimSun, serif;
}

.gitp__badge-row {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.gitp__badge {
  display: inline-block;
  max-width: 75%;
  padding: 2px 10px;
  color: var(--war-user-blue);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  /* GlueScreen-Profile-Stretch2 (ui-design.md §2.2: slice 8) */
  border: 8px solid transparent;
  border-image: url('/assets/wc3_extracted/ui/GlueScreen-Profile-Stretch2.png') 8 fill stretch;
  box-sizing: border-box;
}

.gitp__refresh {
  flex: none;
  color: var(--war-gold);
  user-select: none;
}

.gitp__refresh:hover {
  color: var(--war-gold-bright);
}

.gitp__tabs {
  flex: none;
  display: flex;
  gap: 12px;
}

.gitp__tab {
  color: var(--war-text-muted);
  user-select: none;
}

.gitp__tab:hover {
  color: var(--war-gold-bright);
}

.gitp__tab.active {
  color: var(--war-gold);
  text-decoration: underline;
  text-underline-offset: 3px;
}

.gitp__list-wrap {
  flex: 1;
  min-height: 0;
  max-height: 156px; /* ≈4 rows + scrollbar (features/chat.md §6.3) */
  display: flex;
}

.gitp__list {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none; /* native bar hidden — the WC3 WarScrollBar replaces it */
}

.gitp__row {
  padding: 2px 0;
}

.gitp__row:hover .gitp__subject,
.gitp__change-row:hover .gitp__path {
  color: var(--war-text);
}

.gitp__change-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 2px 0;
}

.gitp__st {
  flex: none;
  min-width: 18px;
  text-align: center;
  color: var(--war-gold-dim);
  font-family: Consolas, monospace;
}

.gitp__st--new {
  color: #7ec87e; /* diff-add green (local literal; theme has no green slot) */
}

.gitp__path {
  flex: 1;
  min-width: 0;
  color: var(--war-text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.gitp__staged {
  flex: none;
  color: var(--war-text-faint);
}

.gitp__subject {
  color: var(--war-text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.gitp__meta {
  color: var(--war-text-muted);
}

.gitp__error {
  color: var(--war-error);
  overflow-wrap: break-word;
}

.gitp__empty {
  color: var(--war-text-faint);
  text-align: center;
  padding: 8px 0;
}

/* ---- diff view ---- */

.gitp__diff-head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
}

.gitp__back {
  flex: none;
  color: var(--war-gold);
  user-select: none;
}

.gitp__back:hover {
  color: var(--war-gold-bright);
}

.gitp__diff-title {
  flex: 1;
  min-width: 0;
  color: var(--war-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.gitp__diff-wrap {
  flex: 1;
  min-height: 0;
  display: flex;
}

.gitp__diff {
  flex: 1;
  min-width: 0;
  overflow: auto;
  scrollbar-width: none; /* native bar hidden — the WC3 WarScrollBar replaces it */
}

.gitp__file {
  margin-bottom: 6px;
}

.gitp__file-name {
  color: var(--war-user-blue);
  padding: 2px 0;
  position: sticky;
  top: 0;
  background: var(--war-glass);
}

.gitp__line {
  display: flex;
  white-space: pre;
  font-family: Consolas, 'Cascadia Mono', monospace;
}

.gitp__ln {
  flex: none;
  width: 3ch;
  min-width: 3ch;
  margin-right: 4px;
  text-align: right;
  color: var(--war-text-faint);
  user-select: none;
}

.gitp__sign {
  flex: none;
  width: 1ch;
  user-select: none;
}

.gitp__code {
  overflow-wrap: normal;
}

.gitp__line--add {
  color: #7ec87e; /* diff-add green */
  background: #7ec87e14;
}

.gitp__line--del {
  color: var(--war-error);
  background: #d0807014;
}

.gitp__line--ctx {
  color: var(--war-text-muted);
}

.gitp__line--hunk {
  color: var(--war-user-blue);
}

.gitp__line--meta {
  color: var(--war-text-faint);
}

.gitp__line--eof {
  color: var(--war-text-faint);
}

.gitp__trunc {
  color: var(--war-gold);
  text-align: center;
  padding: 4px 0;
}
</style>

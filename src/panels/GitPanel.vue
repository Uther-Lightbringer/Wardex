<script setup lang="ts">
// Version-control panel (features/chat.md §6.3): branch badge read from
// .git/HEAD (hidden entirely outside git repos) + a read-only commit list
// (git_log command, 200-entry cap, ~4 rows visible with internal scroll).
// refreshOn: turnEnd | sessionSwitch | expand | manual — the agent may have
// committed or switched branches during the turn.
import { computed, onMounted, ref, watch } from 'vue';
import { cmd } from '../lib/tauri';
import { useChatStore } from '../stores/chat';
import { usePrefsStore } from '../stores/prefs';

interface GitCommit {
  hash: string;
  short: string;
  author: string;
  date: string;
  subject: string;
}

const chat = useChatStore();
const prefs = usePrefsStore();

const branch = ref('');
const commits = ref<GitCommit[]>([]);
const loading = ref(false);
const error = ref('');

const workDir = computed(() => chat.meta?.workDir || chat.meta?.projectDir || chat.projectDir);

async function refresh(): Promise<void> {
  const dir = workDir.value;
  if (!dir) {
    branch.value = '';
    commits.value = [];
    return;
  }
  try {
    branch.value = await cmd<string>('git_branch', { dir }, '');
  } catch {
    branch.value = '';
  }
  if (!branch.value) {
    commits.value = [];
    error.value = '';
    return;
  }
  loading.value = true;
  error.value = '';
  try {
    commits.value = await cmd<GitCommit[]>('git_log', { dir }, []);
  } catch (e) {
    commits.value = [];
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(refresh);
watch(() => chat.sessionId, refresh); // sessionSwitch
watch(() => chat.turnSeq, refresh); // turnEnd
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
      <div class="gitp__list">
        <div v-if="error" class="gitp__error" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ error }}</div>
        <div v-else-if="!loading && commits.length === 0" class="gitp__empty" :style="{ fontSize: prefs.fs(11) + 'px' }">
          暂无提交
        </div>
        <div v-for="c in commits" :key="c.hash" class="gitp__row">
          <div class="gitp__subject" :style="{ fontSize: prefs.fs(12) + 'px' }" :title="c.subject">
            {{ c.subject }}
          </div>
          <div class="gitp__meta" :style="{ fontSize: prefs.fs(10) + 'px' }">
            {{ c.short }} · {{ c.author }} · {{ c.date }}
          </div>
        </div>
      </div>
    </template>
    <div v-else class="gitp__empty" :style="{ fontSize: prefs.fs(12) + 'px' }">（非 Git 目录）</div>
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
  border-image: url('/assets/wc3_extracted/ui/GlueScreen-Profile-Stretch2.png') 8 stretch;
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

.gitp__list {
  flex: 1;
  min-height: 0;
  max-height: 132px; /* ≈4 rows (features/chat.md §6.3) */
  overflow-y: auto;
  scrollbar-width: none;
}

.gitp__row {
  padding: 2px 0;
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
</style>

<script setup lang="ts">
// Usage stats page: app-level token consumption report backed by the
// `usage_report` Rust command. Three sections 总计 / 按 Agent / 按会话;
// session titles are joined from the sessions index (sessions.all).
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import PageShell from '../components/PageShell.vue';
import WarFrame from '../components/war/WarFrame.vue';
import WarButton from '../components/war/WarButton.vue';
import { useNavStore } from '../stores/nav';
import { usePrefsStore } from '../stores/prefs';
import { useSessionsStore } from '../stores/sessions';
import { cmd } from '../lib/tauri';
import { formatTokens } from '../lib/format';

interface UsageSum {
  turns: number;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
}

interface UsageReport {
  grand: UsageSum;
  agents: Array<UsageSum & {
    agentId: string;
    agentName: string;
    models: Array<UsageSum & { model: string }>;
  }>;
  sessions: Array<UsageSum & { sessionId: string; agentName: string }>;
}

const nav = useNavStore();
const prefs = usePrefsStore();
const sessions = useSessionsStore();

const report = ref<UsageReport | null>(null);

onMounted(async () => {
  report.value = await cmd<UsageReport | null>('usage_report', undefined, null);
  // 会话标题 join 来源（list_sessions）；拿不到就回退到 sessionId 前 8 位。
  if (sessions.all.length === 0) await sessions.reloadAll();
});

// Esc returns to the config page (same as the 返回(B) button).
function onPageKey(e: KeyboardEvent): void {
  if (nav.page !== 'usage') return;
  if (e.key === 'Escape') void nav.goOverlay('config');
}
onMounted(() => window.addEventListener('keydown', onPageKey));
onBeforeUnmount(() => window.removeEventListener('keydown', onPageKey));

const titleById = computed(() => {
  const m = new Map<string, string>();
  for (const s of sessions.all) m.set(s.id, s.title);
  return m;
});

function sessionTitle(id: string): string {
  const t = titleById.value.get(id);
  return t && t.trim() ? t : id.slice(0, 8);
}
</script>

<template>
  <PageShell :embed="52">
    <div class="usage">
      <WarFrame
        class="usage__frame"
        src="/assets/ui/frames/frame_iron_panel.png"
        :slice="[96, 110, 69, 108]"
        :hole="[56, 25, 21, 24]"
        :content-left-extra="16"
      >
        <div class="usage__col">
          <div class="usage__title war-font-title war-outline-black" :style="{ fontSize: prefs.fs(18) + 'px' }">
            用量统计
          </div>

          <!-- 总计 -->
          <div class="usage__section" :style="{ fontSize: prefs.fs(14) + 'px' }">总计</div>
          <div class="usage__grand" :style="{ fontSize: prefs.fs(14) + 'px' }">
            <template v-if="report">
              <span class="usage__grand-item">回合数 {{ report.grand.turns }}</span>
              <span class="usage__grand-item">输入 {{ formatTokens(report.grand.inputTokens) }}</span>
              <span class="usage__grand-item">输出 {{ formatTokens(report.grand.outputTokens) }}</span>
              <span class="usage__grand-item">总 tokens {{ formatTokens(report.grand.totalTokens) }}</span>
            </template>
            <span v-else class="usage__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">（暂无数据）</span>
          </div>

          <!-- 按 Agent -->
          <div class="usage__section" :style="{ fontSize: prefs.fs(14) + 'px' }">按 Agent</div>
          <div class="usage__list usage__list--agents">
            <template v-for="(a, i) in report?.agents ?? []" :key="a.agentId">
              <div class="usage__row" :class="{ zebra: i % 2 === 1 }" :style="{ fontSize: prefs.fs(14) + 'px' }">
                <span class="usage__cell usage__cell--name">{{ a.agentName || a.agentId }}</span>
                <span class="usage__cell">{{ a.turns }} 回合</span>
                <span class="usage__cell usage__cell--num">↑{{ formatTokens(a.inputTokens) }}</span>
                <span class="usage__cell usage__cell--num">↓{{ formatTokens(a.outputTokens) }}</span>
                <span class="usage__cell usage__cell--total">{{ formatTokens(a.totalTokens) }}</span>
              </div>
              <div
                v-for="m in a.models"
                :key="a.agentId + '|' + m.model"
                class="usage__row usage__row--model"
                :style="{ fontSize: prefs.fs(12) + 'px' }"
              >
                <span class="usage__cell usage__cell--name">{{ m.model || '未记录' }}</span>
                <span class="usage__cell">{{ m.turns }} 回合</span>
                <span class="usage__cell usage__cell--num">↑{{ formatTokens(m.inputTokens) }}</span>
                <span class="usage__cell usage__cell--num">↓{{ formatTokens(m.outputTokens) }}</span>
                <span class="usage__cell usage__cell--total">{{ formatTokens(m.totalTokens) }}</span>
              </div>
            </template>
            <div v-if="(report?.agents.length ?? 0) === 0" class="usage__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">
              （暂无数据）
            </div>
          </div>

          <!-- 按会话 -->
          <div class="usage__section" :style="{ fontSize: prefs.fs(14) + 'px' }">按会话</div>
          <div class="usage__list usage__list--sessions">
            <div
              v-for="(s, i) in report?.sessions ?? []"
              :key="s.sessionId"
              class="usage__row"
              :class="{ zebra: i % 2 === 1 }"
              :style="{ fontSize: prefs.fs(14) + 'px' }"
            >
              <span class="usage__cell usage__cell--name">{{ sessionTitle(s.sessionId) }}</span>
              <span class="usage__cell usage__cell--agent">{{ s.agentName }}</span>
              <span class="usage__cell">{{ s.turns }} 回合</span>
              <span class="usage__cell usage__cell--num">↑{{ formatTokens(s.inputTokens) }}</span>
              <span class="usage__cell usage__cell--num">↓{{ formatTokens(s.outputTokens) }}</span>
              <span class="usage__cell usage__cell--total">{{ formatTokens(s.totalTokens) }}</span>
            </div>
            <div v-if="(report?.sessions.length ?? 0) === 0" class="usage__empty" :style="{ fontSize: prefs.fs(13) + 'px' }">
              （暂无数据）
            </div>
          </div>

          <!-- bottom actions -->
          <div class="usage__actions">
            <span class="usage__spring"></span>
            <WarButton skin="dialog" :width="150" :art-aspect="5.34" text="返回(B)" shortcut-key="B" :shortcut-active="nav.page === 'usage'" @activated="nav.goOverlay('config')" />
          </div>
        </div>
      </WarFrame>
    </div>
  </PageShell>
</template>

<style scoped>
.usage {
  height: 100%;
  padding-top: 4px;
  padding-bottom: 8px;
  box-sizing: border-box;
}

.usage__frame {
  width: calc(62% - 5px); /* same as TodoPage: leftW = (w-gap)*0.62 */
  height: 100%;
}

.usage__col {
  display: flex;
  flex-direction: column;
  gap: 8px;
  height: 100%;
}

.usage__title {
  color: var(--war-text-dim);
  flex: none;
}

.usage__section {
  color: var(--war-gold);
  font-family: SimSun, serif;
  flex: none;
}

.usage__grand {
  display: flex;
  gap: 18px;
  flex-wrap: wrap;
  flex: none;
  padding: 6px 10px;
  color: var(--war-text);
  font-family: SimSun, serif;
  border: 1px solid #1c2430;
  border-radius: 3px;
  background: #0e121899;
  box-sizing: border-box;
}

.usage__grand-item {
  white-space: nowrap;
}

.usage__list {
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.usage__list--agents {
  flex: 55;
}

.usage__list--sessions {
  flex: 45;
}

.usage__row {
  display: flex;
  align-items: center;
  gap: 10px;
  height: 34px;
  flex: none;
  padding: 4px 10px;
  color: var(--war-text);
  font-family: SimSun, serif;
  border: 1px solid #1c2430;
  border-radius: 3px;
  background: #0e121899;
  box-sizing: border-box;
}

.usage__row.zebra {
  background: #14182099;
}

.usage__row--model {
  color: var(--war-text-muted);
  background: #10141866;
  border-color: #141a24;
  margin-left: 24px; /* model 明细缩进在所属 agent 行之下 */
}

.usage__cell {
  flex: none;
  white-space: nowrap;
}

.usage__cell--name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

.usage__cell--agent {
  color: var(--war-text-muted);
  font-size: 0.85em;
  max-width: 120px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.usage__cell--num {
  color: var(--war-text-muted);
}

.usage__cell--total {
  color: var(--war-gold);
  min-width: 56px;
  text-align: right;
}

.usage__empty {
  color: var(--war-text-faint);
  font-family: SimSun, serif;
  text-align: center;
  padding: 12px 0;
}

.usage__actions {
  display: flex;
  align-items: center;
  gap: 12px;
  flex: none;
}

.usage__spring {
  flex: 1;
}
</style>

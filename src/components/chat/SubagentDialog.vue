<script setup lang="ts">
// Sub-agent detail dialog (features/chat.md §4.2): opened by clicking an
// entry in the SubagentPanel. Shows the task brief (input: prompt / swarm
// args JSON) and the final report (output: rawOutput) — ACP reports a
// sub-agent as one top-level tool call, so its INNER steps are not
// available; brief+report is all the protocol gives us.
// Liveness: elapsed time + "time since last update" (turns red past the
// stuck threshold). The stop button cancels the WHOLE turn (session/cancel)
// — ACP cannot kill a single sub-agent. Esc / mask click closes.
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import type { Subagent } from '../../stores/chat';
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';
import { cmd } from '../../lib/tauri';
import WarButton from '../war/WarButton.vue';
import WarScrollBar from '../war/WarScrollBar.vue';

const props = defineProps<{
  open: boolean;
  entry: Subagent | null;
}>();

const emit = defineEmits<{ (e: 'update:open', v: boolean): void }>();

const chat = useChatStore();
const prefs = usePrefsStore();

const STATUS_CN: Record<string, string> = {
  in_progress: '执行中',
  pending: '等待',
  completed: '完成',
  failed: '失败',
  interrupted: '中断',
};

const STUCK_MS = 120_000; // no tool_call updates for 2min → "可能卡住"

const live = computed(
  () => props.entry?.status === 'pending' || props.entry?.status === 'in_progress',
);

// 1s heartbeat while open AND live — drives elapsed/stale labels.
const now = ref(Date.now());
let timer: ReturnType<typeof setInterval> | null = null;
function syncTimer(): void {
  const need = props.open && live.value;
  if (need && !timer) {
    timer = setInterval(() => (now.value = Date.now()), 1000);
  } else if (!need && timer) {
    clearInterval(timer);
    timer = null;
  }
}
watch(() => [props.open, live.value], syncTimer, { immediate: true });
onBeforeUnmount(() => {
  if (timer) clearInterval(timer);
});

function fmtDur(ms: number): string {
  const sec = Math.max(0, Math.round(ms / 1000));
  if (sec < 60) return `${sec}s`;
  return `${Math.floor(sec / 60)}m${sec % 60}s`;
}

const elapsed = computed(() => {
  const e = props.entry;
  if (!e || e.startedAt <= 0) return '';
  void now.value;
  const end = e.finishedAt > 0 ? e.finishedAt : Date.now();
  return fmtDur(end - e.startedAt);
});

const staleMs = computed(() => {
  const e = props.entry;
  if (!e || !live.value || e.lastUpdate <= 0) return 0;
  void now.value;
  return Math.max(0, Date.now() - e.lastUpdate);
});
const stuck = computed(() => staleMs.value >= STUCK_MS);

const metaLine = computed(() => {
  const e = props.entry;
  if (!e) return '';
  const parts = [e.kind, STATUS_CN[e.status] ?? e.status];
  if (elapsed.value) parts.push(`已用时 ${elapsed.value}`);
  if (live.value && staleMs.value > 0) {
    parts.push(stuck.value ? `可能卡住 · 无更新 ${fmtDur(staleMs.value)}` : `距上次更新 ${fmtDur(staleMs.value)}`);
  }
  return parts.join(' · ');
});

const canStop = computed(() => live.value && chat.status.busy);

function stop(): void {
  void chat.cancel();
}

function close(): void {
  emit('update:open', false);
}

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.stopPropagation();
    close();
  }
}

watch(
  () => props.open,
  (v) => {
    if (v) window.addEventListener('keydown', onKey, true);
    else window.removeEventListener('keydown', onKey, true);
  },
);
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true));

// WC3 scrollbar targets
const briefEl = ref<HTMLElement | null>(null);
const reportEl = ref<HTMLElement | null>(null);
const procEl = ref<HTMLElement | null>(null);

// ---- 执行过程 (on-disk wire.jsonl via subagent_process) ----
// kimi CLI 专属优化：只有 provider === 'kimi' 才展示此区；其他 ACP CLI
// 没有这套磁盘格式，保持任务书+报告的通用视图（各自的原生行为）。
const isKimi = computed(() => chat.meta?.provider === 'kimi');

interface ProcStep {
  kind: string; // tool | result | think | text
  name: string;
  detail: string;
}

const procSteps = ref<ProcStep[]>([]);
const procError = ref('');
const procLoading = ref(false);
const procTruncated = ref(false);
const procOpenIdx = ref<Record<number, boolean>>({});
const activeAgentId = ref('');

function toggleProc(i: number): void {
  procOpenIdx.value = { ...procOpenIdx.value, [i]: !procOpenIdx.value[i] };
}

function previewOf(s: ProcStep): string {
  const first = (s.detail.split('\n').find((l) => l.trim()) ?? '').trim();
  return first.length > 56 ? first.slice(0, 56) + '…' : first;
}

async function loadProcess(agentId?: string): Promise<void> {
  const id = agentId ?? activeAgentId.value;
  if (!id || !chat.sessionId) return;
  activeAgentId.value = id;
  procLoading.value = true;
  procError.value = '';
  try {
    const r = await cmd<{ steps: ProcStep[]; truncated: boolean }>('subagent_process', {
      sessionId: chat.sessionId,
      agentId: id,
    });
    procSteps.value = r.steps ?? [];
    procTruncated.value = !!r.truncated;
    procOpenIdx.value = {};
  } catch (e) {
    procSteps.value = [];
    procError.value = String(e);
  } finally {
    procLoading.value = false;
  }
}

// Auto-load when the dialog opens with known agent ids; reset on close.
watch(
  () => [props.open, props.entry?.agentIds?.join(',')] as const,
  ([open]) => {
    if (!open) {
      procSteps.value = [];
      procError.value = '';
      activeAgentId.value = '';
      return;
    }
    const ids = props.entry?.agentIds ?? [];
    if (isKimi.value && ids.length > 0) void loadProcess(ids[0]);
    else {
      procSteps.value = [];
      procError.value = '';
      activeAgentId.value = '';
    }
  },
  { immediate: true },
);
</script>

<template>
  <Teleport to="body">
    <div v-if="open && entry" class="sad-mask" @mousedown.self="close">
      <div class="sad">
        <div class="sad__frame"></div>

        <div class="sad__inner">
          <!-- header -->
          <div class="sad__head">
            <div class="sad__head-text">
              <div class="sad__title" :style="{ fontSize: prefs.fs(14) + 'px' }" :title="entry.title">
                {{ entry.title }}
              </div>
              <div class="sad__meta" :class="{ stuck }" :style="{ fontSize: prefs.fs(11) + 'px' }">
                {{ metaLine }}
              </div>
            </div>
            <span class="sad__close" title="关闭" @click="close">✕</span>
          </div>

          <!-- swarm children -->
          <div v-if="entry.childNames.length > 0" class="sad__children">
            <span
              v-for="(n, i) in entry.childNames"
              :key="i"
              class="sad__child"
              :style="{ fontSize: prefs.fs(11) + 'px' }"
              :title="n"
              >{{ n }}</span
            >
          </div>

          <!-- task brief -->
          <div class="sad__label" :style="{ fontSize: prefs.fs(12) + 'px' }">任务书</div>
          <div class="sad__pane sad__pane--brief">
            <pre ref="briefEl" class="sad__pre" :style="{ fontSize: prefs.fs(11) + 'px' }">{{
              entry.input || '（无任务书数据）'
            }}</pre>
            <WarScrollBar :target="briefEl" />
          </div>

          <!-- report -->
          <div class="sad__label" :style="{ fontSize: prefs.fs(12) + 'px' }">最终报告</div>
          <div class="sad__pane sad__pane--report">
            <pre ref="reportEl" class="sad__pre" :style="{ fontSize: prefs.fs(11) + 'px' }">{{
              entry.output || (live ? '（执行中，暂无报告）' : '（无报告数据）')
            }}</pre>
            <WarScrollBar :target="reportEl" />
          </div>

          <!-- process (kimi CLI 专属：读 on-disk wire；其他 ACP CLI 不显示此区) -->
          <template v-if="isKimi">
          <div class="sad__label sad__label--row" :style="{ fontSize: prefs.fs(12) + 'px' }">
            <span>执行过程</span>
            <template v-if="(entry.agentIds?.length ?? 0) > 1">
              <span
                v-for="id in entry.agentIds"
                :key="id"
                class="sad__aid"
                :class="{ active: id === activeAgentId }"
                :style="{ fontSize: prefs.fs(10) + 'px' }"
                @click="loadProcess(id)"
                >{{ id }}</span
              >
            </template>
            <span
              v-if="activeAgentId"
              class="sad__refresh"
              :style="{ fontSize: prefs.fs(10) + 'px' }"
              @click="loadProcess()"
              >刷新</span
            >
          </div>
          <div class="sad__pane sad__pane--proc">
            <div ref="procEl" class="sad__proc">
              <div v-if="procLoading" class="sad__proc-note" :style="{ fontSize: prefs.fs(11) + 'px' }">加载中…</div>
              <div v-else-if="procError" class="sad__proc-note" :style="{ fontSize: prefs.fs(11) + 'px' }">
                {{ procError }}
              </div>
              <div
                v-else-if="!entry.agentIds || entry.agentIds.length === 0"
                class="sad__proc-note"
                :style="{ fontSize: prefs.fs(11) + 'px' }"
              >
                （子 Agent 完成后才可查看执行过程）
              </div>
              <template v-else>
                <div v-for="(st, i) in procSteps" :key="i" class="sad__step" :class="`sad__step--${st.kind}`">
                  <div class="sad__step-head" :style="{ fontSize: prefs.fs(11) + 'px' }" @click="toggleProc(i)">
                    {{ procOpenIdx[i] ? '▼' : '▶' }} {{ st.name }}
                    <span v-if="!procOpenIdx[i]" class="sad__step-preview">{{ previewOf(st) }}</span>
                  </div>
                  <pre v-if="procOpenIdx[i]" class="sad__step-detail" :style="{ fontSize: prefs.fs(11) + 'px' }">{{
                    st.detail
                  }}</pre>
                </div>
                <div v-if="procTruncated" class="sad__proc-note" :style="{ fontSize: prefs.fs(10) + 'px' }">
                  …（步骤过多，仅显示最后 400 条）
                </div>
              </template>
            </div>
            <WarScrollBar :target="procEl" />
          </div>
          </template>

          <!-- footer -->
          <div class="sad__footer">
            <WarButton v-if="canStop" skin="dialog" :width="180" text="停止回合" @activated="stop" />
            <WarButton skin="dialog" :width="150" text="关闭" @activated="close" />
          </div>
          <div v-if="canStop" class="sad__stop-hint" :style="{ fontSize: prefs.fs(10) + 'px' }">
            「停止回合」会中断当前整个回合（含其全部子 Agent）——ACP 不支持单独停止某一个子 Agent
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.sad-mask {
  position: fixed;
  inset: 0;
  z-index: 120;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.sad {
  position: relative;
  width: min(760px, 92vw);
  height: min(680px, 92vh);
}

/* frame_popup.png nine-slice (slice 88/100/90/100, center painted → fill) */
.sad__frame {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 88px 100px 90px 100px;
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.sad__inner {
  position: absolute;
  inset: 60px 64px 56px 62px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-height: 0;
  font-family: SimSun, serif;
}

.sad__head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 12px;
}

.sad__head-text {
  flex: 1;
  min-width: 0;
}

.sad__title {
  color: var(--war-gold);
  font-weight: bold;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sad__meta {
  color: var(--war-text-muted);
}

.sad__meta.stuck {
  color: var(--war-error);
}

.sad__close {
  flex: none;
  color: var(--war-text-dim);
  padding: 2px 6px;
  user-select: none;
}

.sad__close:hover {
  color: var(--war-gold-bright);
}

.sad__children {
  flex: none;
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.sad__child {
  max-width: 220px;
  padding: 1px 8px;
  background: #1a2334;
  border: 1px solid #2c4a7a;
  border-radius: 2px;
  color: #c0d0ec;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sad__label {
  flex: none;
  color: var(--war-gold-dim);
}

.sad__pane {
  min-height: 0;
  display: flex;
  border: 1px solid #1a2230;
  background: #10141dcc;
}

.sad__pane--brief {
  flex: 2;
}

.sad__pane--report {
  flex: 2;
}

.sad__pane--proc {
  flex: 3;
}

.sad__label--row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.sad__aid {
  padding: 0 6px;
  background: #1a2334;
  border: 1px solid #2c4a7a;
  border-radius: 2px;
  color: #c0d0ec;
  user-select: none;
}

.sad__aid:hover,
.sad__aid.active {
  color: var(--war-gold);
  border-color: var(--war-gold-dim);
}

.sad__refresh {
  margin-left: auto;
  color: #a0a8b8;
  user-select: none;
}

.sad__refresh:hover {
  color: var(--war-gold-bright);
}

.sad__proc {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none;
  padding: 4px;
}

.sad__proc-note {
  color: var(--war-text-faint);
  padding: 6px 8px;
}

.sad__step {
  margin: 3px 4px;
  padding: 3px 8px;
  border-radius: 2px;
  background: #12151c44;
  border: 1px solid #3a4a40;
}

.sad__step--think {
  background: #19151044;
  border-color: #4a4232;
}

.sad__step--result {
  border-color: #2a3344;
}

.sad__step-head {
  color: #d0d6e0;
  user-select: none;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sad__step--think .sad__step-head {
  color: #c8b890;
}

.sad__step-head:hover {
  color: var(--war-gold);
}

.sad__step-preview {
  color: var(--war-text-faint);
  margin-left: 8px;
}

.sad__step-detail {
  margin: 4px 0 2px;
  color: var(--war-text-muted);
  white-space: pre-wrap;
  overflow-wrap: break-word;
  font-family: Consolas, monospace;
  user-select: text;
}

.sad__pre {
  flex: 1;
  min-width: 0;
  margin: 0;
  padding: 6px 8px;
  overflow-y: auto;
  scrollbar-width: none;
  white-space: pre-wrap;
  overflow-wrap: break-word;
  color: var(--war-text-muted);
  font-family: Consolas, 'Cascadia Mono', monospace;
  user-select: text;
}

.sad__footer {
  flex: none;
  display: flex;
  justify-content: center;
  gap: 16px;
  padding-top: 4px;
}

.sad__stop-hint {
  flex: none;
  text-align: center;
  color: var(--war-text-faint);
}
</style>

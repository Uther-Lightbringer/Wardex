<script setup lang="ts">
// 监控页权限审批弹窗（原型 #perm-dialog）：不跳页直接答复。
// 渲染 PermissionRequest.params.options（optionId/name/kind）为按钮 +
// toolCall 标题/内容摘要；options 为空 → 允许/拒绝兜底（照 PermissionDialog）。
// questions 非空（AskUserQuestion 型）不处理：提示并跳 ChatPage。
// Esc 由 MonitorPage 页级处理（只关弹窗不答复，请求保持 pending）。
import { computed } from 'vue';
import type { PermissionRequest } from '../../stores/chat';
import { useMonitorStore } from '../../stores/monitor';
import { useChatStore } from '../../stores/chat';
import { useNavStore } from '../../stores/nav';
import { useUiStore } from '../../stores/ui';
import { usePrefsStore } from '../../stores/prefs';

const props = defineProps<{ sessionId: string; request: PermissionRequest | null }>();

const monitor = useMonitorStore();
const chat = useChatStore();
const nav = useNavStore();
const ui = useUiStore();
const prefs = usePrefsStore();

const questionMode = computed(() => (props.request?.questions?.length ?? 0) > 0);

const title = computed(() => {
  const tc = props.request?.params.toolCall;
  return (tc?.title || tc?.kind || 'Agent 请求执行工具') as string;
});

/** Whitespace-collapse + 160-char middle elide（照 PermissionDialog）。 */
function condense(s: string): string {
  const flat = s.replace(/\s+/g, ' ').trim();
  if (flat.length <= 160) return flat;
  return flat.slice(0, 78) + ' … ' + flat.slice(flat.length - 78);
}

const detail = computed(() => {
  const tc = props.request?.params.toolCall;
  if (!tc) return '';
  let text = '';
  if (Array.isArray(tc.content)) {
    text = tc.content.map((b) => b?.content?.text ?? '').join('\n');
  }
  if (!text.trim() && tc.rawInput) {
    const ri = tc.rawInput as Record<string, unknown>;
    text = String(ri.path ?? ri.file_path ?? ri.abs_path ?? '');
  }
  const out = condense(text);
  return out && out !== title.value ? out : '';
});

interface OptionView {
  id: string;
  label: string;
  kind: string;
  cancel?: boolean;
}

/** Generic approve/reject names → the four Chinese states（照 PermissionDialog）。 */
function optionLabel(name: string, kind: string): string {
  const n = name.trim().toLowerCase();
  if (/^(approve|allow)( once)?$/.test(n) || n === 'allow' || n === 'approve') return '允许一次';
  if (n.startsWith('approve for this') || n === 'allow always' || n === 'always allow')
    return '总是允许';
  if (n === 'reject' || n === 'reject once' || n === 'deny') return '拒绝';
  if (n === 'reject always' || n === 'always reject') return '总是拒绝';
  if (!n) {
    if (kind === 'allow_once') return '允许一次';
    if (kind === 'allow_always') return '总是允许';
    if (kind === 'reject_once') return '拒绝';
    if (kind === 'reject_always') return '总是拒绝';
    return '选项';
  }
  return name;
}

const options = computed<OptionView[]>(() => {
  const opts = props.request?.params.options;
  if (!opts || opts.length === 0) {
    return [
      { id: 'allow', label: '允许', kind: 'allow_once' },
      { id: '', label: '拒绝', kind: 'reject_once', cancel: true },
    ];
  }
  return opts.map((o) => ({
    id: String(o.optionId ?? o.id ?? ''),
    label: optionLabel(String(o.name ?? ''), String(o.kind ?? '')),
    kind: String(o.kind ?? ''),
  }));
});

function btnClass(o: OptionView): string {
  if (o.kind.startsWith('reject') || o.cancel) return 'no';
  if (o.kind.startsWith('allow')) return 'ok';
  return '';
}

function answer(o: OptionView): void {
  void monitor.answerPermission(props.sessionId, o.id, o.cancel === true);
}

/** AskUserQuestion 型：监控页不处理，跳 ChatPage 的完整 PermissionDialog。 */
async function gotoChat(): Promise<void> {
  monitor.closePermDialog();
  const ok = await chat.openSession(props.sessionId);
  if (!ok) {
    ui.showBanner('无法打开会话');
    return;
  }
  await nav.goOverlay('chat');
}
</script>

<template>
  <div class="pd">
    <h3 class="pd__head" :style="{ fontSize: prefs.fs(16) + 'px' }">⚠ 权限请求</h3>

    <!-- payload 补拉中（后台会话 ensure_runtime → pending_permission） -->
    <div v-if="!request" class="pd__body" :style="{ fontSize: prefs.fs(13) + 'px' }">加载权限请求中…</div>

    <!-- AskUserQuestion 型：不处理，引导去完整页面 -->
    <template v-else-if="questionMode">
      <div class="pd__body" :style="{ fontSize: prefs.fs(13) + 'px' }">
        这是一条 <span class="pd__tool">Agent 询问</span>（需要看完整问题选项），
        监控页不处理，请前往完整会话页面答复。
      </div>
      <div class="pd__btns">
        <div class="pd__btn ok" :style="{ fontSize: prefs.fs(13) + 'px' }" @click="gotoChat">
          前往完整页面处理 →
        </div>
      </div>
    </template>

    <template v-else>
      <div class="pd__body" :style="{ fontSize: prefs.fs(13) + 'px' }">
        会话请求执行：<br />
        <span class="pd__tool">{{ title }}</span>
        <template v-if="detail"><br /><br />{{ detail }}</template>
      </div>
      <div class="pd__btns">
        <div
          v-for="(o, i) in options"
          :key="i"
          class="pd__btn"
          :class="btnClass(o)"
          :style="{ fontSize: prefs.fs(13) + 'px' }"
          @click="answer(o)"
        >
          {{ o.label }}
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.pd {
  position: absolute;
  z-index: 75;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  width: 380px;
  max-width: 90%;
  background: #10141f;
  border: 2px solid var(--war-gold-dim);
  border-radius: 4px;
  box-shadow:
    0 0 60px #000,
    0 0 24px #c9a22744;
  padding: 16px;
  font-family: SimSun, serif;
}

.pd__head {
  color: var(--war-gold);
  text-shadow: 1px 1px 0 #000;
  margin: 0 0 10px;
}

.pd__body {
  color: #d8cba0;
  line-height: 1.6;
  background: #0a0d14;
  border: 1px solid #2a3344;
  padding: 10px;
  margin-bottom: 14px;
  overflow-wrap: break-word;
  user-select: text;
}

.pd__tool {
  color: #7ec9e0;
}

.pd__btns {
  display: flex;
  gap: 10px;
  justify-content: center;
  flex-wrap: wrap;
}

.pd__btn {
  min-width: 110px;
  height: 34px;
  line-height: 34px;
  text-align: center;
  color: #e8d9a0;
  background: linear-gradient(#2b3a50, #1a2233);
  border: 1px solid #4a5b75;
  border-radius: 3px;
  text-shadow: 1px 1px 0 #000;
  padding: 0 10px;
  box-sizing: border-box;
}

.pd__btn:hover {
  border-color: var(--war-gold);
  color: var(--war-gold);
}

.pd__btn.ok {
  border-color: #7ec97a;
  color: #a8e6a0;
}

.pd__btn.ok:hover {
  border-color: #a8e6a0;
  color: #d0f5c8;
}

.pd__btn.no {
  border-color: #b0552f;
  color: #ff9b8a;
}
</style>

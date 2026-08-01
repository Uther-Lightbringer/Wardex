<script setup lang="ts">
// Permission request dialog (features/chat.md §4.5). Modal, NoAutoClose.
// Title line = toolCall.title || kind || fallback; detail = content text (or
// rawInput path) with whitespace collapsed and a 160-char middle elide.
// Buttons come from the ACP options[] (generic approve/reject names are
// localized; AskUserQuestion texts stay verbatim); >3 options lay out in a
// two-column grid. No options → 允许 / 拒绝 fallback.
//
// AskUserQuestion mode (kimi q{n}_* wire format, parsed into
// chat.permission.questions by acp/types.rs): EVERY question group the wire
// carries renders with its own option buttons, so nothing is dropped
// client-side. Two protocol-level narrowings apply (both mirror kimi's own
// adapter): the ACP response carries exactly one optionId, so answering any
// group resolves the request with that single selection — even for
// multi_select questions; and kimi 0.29.x degrades multi-question calls to
// the first question agent-side, so today only one group ever arrives.
import { computed } from 'vue';
import { useChatStore } from '../../stores/chat';
import type { QuestionGroup } from '../../stores/chat';
import WarDialog from '../war/WarDialog.vue';
import WarButton from '../war/WarButton.vue';

const chat = useChatStore();

const open = computed({
  get: () => chat.permission !== null,
  set: () => {
    /* NoAutoClose: only answering clears the request */
  },
});

const questions = computed<QuestionGroup[]>(() => chat.permission?.questions ?? []);
const questionMode = computed(() => questions.value.length > 0);

const title = computed(() => {
  const tc = chat.permission?.params.toolCall;
  const t = (tc?.title || tc?.kind || 'Agent 请求执行工具') as string;
  // The kimi adapter names the toolCall "AskUserQuestion" — localize.
  return questionMode.value && t === 'AskUserQuestion' ? 'Agent 询问' : t;
});

/** Whitespace-collapse + 160-char middle elide (head 78 … tail 78). */
function condense(s: string): string {
  const flat = s.replace(/\s+/g, ' ').trim();
  if (flat.length <= 160) return flat;
  return flat.slice(0, 78) + ' … ' + flat.slice(flat.length - 78);
}

const detail = computed(() => {
  const tc = chat.permission?.params.toolCall;
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
  cancel?: boolean;
}

/** Generic approve/reject names → the four Chinese states (§4.5). */
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
  return name; // AskUserQuestion real option text stays verbatim
}

const options = computed<OptionView[]>(() => {
  const opts = chat.permission?.params.options;
  if (!opts || opts.length === 0) {
    return [
      { id: 'allow', label: '允许' },
      { id: '', label: '拒绝', cancel: true },
    ];
  }
  return opts.map((o) => ({
    id: String(o.optionId ?? o.id ?? ''),
    label: optionLabel(String(o.name ?? ''), String(o.kind ?? '')),
  }));
});

const grid = computed(() => options.value.length > 3);

/** Question mode always uses the wide dialog (stacked groups + scroll). */
const dialogWidth = computed(() => (questionMode.value || grid.value ? 640 : 560));

function answer(o: OptionView): void {
  void chat.answerPermission(o.id, o.cancel === true);
}

/** Question option / Skip click: the optionId round-trips verbatim. */
function answerQuestion(optionId: string): void {
  void chat.answerPermission(optionId, false);
}
</script>

<template>
  <WarDialog v-model:open="open" title-text="工具权限请求" no-auto-close :dialog-width="dialogWidth">
    <template #plate>
      <div class="perm__title">{{ title }}</div>
      <!-- question mode: the question text is the headline content — show it
           big and centered inside the plate instead of cramped below -->
      <div v-if="questionMode" class="perm__plate-questions">
        <div v-for="(q, qi) in questions" :key="q.index" class="perm__plate-q">
          <div v-if="questions.length > 1" class="perm__qhead">
            问题 {{ qi + 1 }} / {{ questions.length }}
          </div>
          <div class="perm__qtext">{{ q.text || '（无问题文本）' }}</div>
          <div v-if="q.multi_select" class="perm__qhint">
            （该问题允许多选；ACP 通道每次仅回传一个选项）
          </div>
        </div>
      </div>
      <div v-else-if="detail" class="perm__detail">{{ detail }}</div>
    </template>
    <div v-if="questionMode" class="perm__questions">
      <div v-for="q in questions" :key="q.index" class="perm__question">
        <div class="perm__buttons" :class="{ grid: q.options.length > 3 }">
          <WarButton
            v-for="(o, i) in q.options"
            :key="i"
            skin="dialog"
            :width="q.options.length > 3 ? 168 : 190"
            :text="o.label"
            @activated="answerQuestion(o.option_id)"
          />
          <WarButton
            v-if="q.skip_id"
            skin="dialog"
            :width="q.options.length > 3 ? 168 : 190"
            text="跳过"
            @activated="answerQuestion(q.skip_id)"
          />
        </div>
      </div>
    </div>
    <div v-else class="perm__buttons" :class="{ grid }">
      <WarButton
        v-for="(o, i) in options"
        :key="i"
        skin="dialog"
        :width="grid ? 168 : 190"
        :text="o.label"
        @activated="answer(o)"
      />
    </div>
  </WarDialog>
</template>

<style scoped>
.perm__title {
  color: var(--war-text);
  font-family: SimSun, serif;
  font-size: 14px;
  font-weight: bold;
  text-align: center;
  overflow-wrap: break-word;
}

.perm__detail {
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  font-size: 12px;
  text-align: center;
  overflow-wrap: break-word;
  max-height: 56px;
  overflow: hidden;
}

.perm__buttons {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: center;
  gap: 8px 12px;
  max-width: 100%;
}

.perm__buttons.grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  justify-items: center;
}

/* question text lives in the plate (the frame art's black gold-rim area) */
.perm__plate-questions {
  width: 100%;
  max-height: 120px; /* plate is ~35% of the dialog; scroll long texts */
  overflow-y: auto;
  scrollbar-width: none;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}

.perm__plate-q {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
}

.perm__questions {
  width: 100%;
  max-height: 320px;
  overflow-y: auto;
  scrollbar-width: none;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.perm__question {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
}

.perm__qhead {
  color: var(--war-text-faint);
  font-family: SimSun, serif;
  font-size: 11px;
}

.perm__qtext {
  color: var(--war-gold-bright);
  font-family: SimSun, serif;
  font-size: 15px;
  font-weight: bold;
  text-align: center;
  line-height: 1.5;
  text-shadow:
    -1px 0 var(--war-outline-dark), 1px 0 var(--war-outline-dark),
    0 -1px var(--war-outline-dark), 0 1px var(--war-outline-dark);
  overflow-wrap: break-word;
}

.perm__qhint {
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  font-size: 11px;
  text-align: center;
}
</style>

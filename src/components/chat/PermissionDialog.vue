<script setup lang="ts">
// Permission request dialog (features/chat.md §4.5). Modal, NoAutoClose.
// Title line = toolCall.title || kind || fallback; detail = content text (or
// rawInput path) with whitespace collapsed and a 160-char middle elide.
// Buttons come from the ACP options[] (generic approve/reject names are
// localized; AskUserQuestion texts stay verbatim); >3 options lay out in a
// two-column grid. No options → 允许 / 拒绝 fallback.
import { computed } from 'vue';
import { useChatStore } from '../../stores/chat';
import WarDialog from '../war/WarDialog.vue';
import WarButton from '../war/WarButton.vue';

const chat = useChatStore();

const open = computed({
  get: () => chat.permission !== null,
  set: () => {
    /* NoAutoClose: only answering clears the request */
  },
});

const title = computed(() => {
  const tc = chat.permission?.params.toolCall;
  return (tc?.title || tc?.kind || 'Agent 请求执行工具') as string;
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

function answer(o: OptionView): void {
  void chat.answerPermission(o.id, o.cancel === true);
}
</script>

<template>
  <WarDialog v-model:open="open" title-text="工具权限请求" no-auto-close :dialog-width="grid ? 640 : 560">
    <template #plate>
      <div class="perm__title">{{ title }}</div>
      <div v-if="detail" class="perm__detail">{{ detail }}</div>
    </template>
    <div class="perm__buttons" :class="{ grid }">
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
</style>

<script setup lang="ts">
// Composer (features/chat.md §3): 64K cap with the three truncation paths,
// Enter/Shift+Enter with IME guard, @ file-reference picker with :from-to
// line suffix, Ctrl+V image paste → media cache + attachment bar (≤6),
// prompt template menu, permission-mode dropdown, send/enqueue button.
//
// Send path: the draft keeps short @tokens; only at send time each token is
// expanded through read_file_range into a 【引用文件：…】 block (§3.3) and the
// expanded text + attachment paths go to send_prompt. The draft and
// attachments are cleared ONLY when the backend accepts (§3.2).
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { cmd, isTauri } from '../../lib/tauri';
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';
import WarDropdown from '../war/WarDropdown.vue';
import WarButton from '../war/WarButton.vue';

const MAX_LEN = 64000;
const REF_INJECT_NOTE = '…（文件超过 200KB，已截断）';

const chat = useChatStore();
const prefs = usePrefsStore();

const text = ref('');
const inputEl = ref<HTMLTextAreaElement | null>(null);
const composing = ref(false);

// ---- 64K cap ----
const notice = ref('');
let noticeTimer: ReturnType<typeof setTimeout> | null = null;

function showNotice(): void {
  notice.value = `已达输入上限（${MAX_LEN} 字），超出部分已截断；大段内容请作为附件发送`;
  if (noticeTimer) clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => (notice.value = ''), 6000);
}
onBeforeUnmount(() => {
  if (noticeTimer) clearTimeout(noticeTimer);
});

const showCounter = computed(() => text.value.length > MAX_LEN * 0.75);

/** Unified truncation entry (typing fallback + template insert). */
function truncateInput(): void {
  if (text.value.length > MAX_LEN) {
    text.value = text.value.slice(0, MAX_LEN);
    showNotice();
  }
}

function onInput(): void {
  if (composing.value) return; // never touch the document mid-IME (§3.1 path ①)
  truncateInput();
  updatePicker();
}

function onKeydown(e: KeyboardEvent): void {
  // Path ②: at the cap, reject printable keys without a selection (a
  // selection replaces, not grows — let those through).
  if (
    text.value.length >= MAX_LEN &&
    e.key.length === 1 &&
    !e.ctrlKey &&
    !e.altKey &&
    !e.metaKey &&
    inputEl.value &&
    inputEl.value.selectionStart === inputEl.value.selectionEnd
  ) {
    e.preventDefault();
    showNotice();
    return;
  }
  if (e.key === 'Enter' && !e.shiftKey && !composing.value && !e.isComposing) {
    if (pickerOpen.value) {
      e.preventDefault();
      pickPicker(pickerIndex.value);
      return;
    }
    e.preventDefault();
    void send();
    return;
  }
  if (pickerOpen.value) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      pickerIndex.value = Math.min(pickerIndex.value + 1, pickerItems.value.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      pickerIndex.value = Math.max(pickerIndex.value - 1, 0);
    } else if (e.key === 'Tab') {
      e.preventDefault();
      pickPicker(pickerIndex.value);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      pickerOpen.value = false;
    }
  }
}

// ---- paste: image → media cache; text → capped head insert (§3.1 path ③) ----
async function onPaste(e: ClipboardEvent): Promise<void> {
  const items = e.clipboardData?.items;
  if (!items) return;
  for (const item of items) {
    if (item.kind === 'file' && item.type.startsWith('image/')) {
      e.preventDefault();
      const file = item.getAsFile();
      if (!file) return;
      const buf = await file.arrayBuffer();
      try {
        const path = await cmd<string>('save_clipboard_image', {
          sessionId: chat.sessionId,
          bytes: Array.from(new Uint8Array(buf)),
        });
        if (path) chat.addAttachments([path]);
      } catch (err) {
        console.warn('[composer] save_clipboard_image failed', err);
      }
      return;
    }
  }
  // Plain text: measure first, insert only the head that fits.
  const pasted = e.clipboardData?.getData('text/plain');
  if (pasted === undefined || pasted === '') return;
  e.preventDefault();
  const el = inputEl.value;
  const s = el ? el.selectionStart : text.value.length;
  const t = el ? el.selectionEnd : text.value.length;
  const room = MAX_LEN - (text.value.length - (t - s));
  if (pasted.length > room) showNotice();
  const insert = pasted.slice(0, Math.max(0, room));
  text.value = text.value.slice(0, s) + insert + text.value.slice(t);
  void Promise.resolve().then(() => {
    if (el) {
      const pos = s + insert.length;
      el.selectionStart = el.selectionEnd = pos;
      el.focus();
    }
  });
}

// ---- attachment picker (the bar itself renders in ChatPage) ----
const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'];

async function pickFiles(): Promise<void> {
  if (!isTauri) return;
  try {
    const picked = await openFileDialog({
      multiple: true,
      filters: [
        { name: '图片', extensions: IMAGE_EXTS },
        { name: '所有文件', extensions: ['*'] },
      ],
    });
    if (!picked) return;
    chat.addAttachments(Array.isArray(picked) ? picked : [picked]);
  } catch (e) {
    console.warn('[composer] open dialog failed', e);
  }
}

// ---- @ file-reference picker (§3.3) ----
interface RefToken {
  start: number; // index of '@' in the draft
  end: number; // cursor
  segmentStart: number; // start of the current comma-segment
  filter: string; // current segment minus the :from-to suffix
  suffix: string; // typed ":from" / ":from-to" to preserve on select
  exact: boolean; // filter already equals a listed path (picker closes)
}

const pickerOpen = ref(false);
const pickerItems = ref<string[]>([]);
const pickerIndex = ref(0);
const activeToken = ref<RefToken | null>(null);
let pickerSeq = 0;

const TOKEN_RE = /^[^\s@]*(?:, ?[^\s@]*)*$/;
const SUFFIX_RE = /:(\d+(?:-\d+)?)$/;

function currentToken(): RefToken | null {
  const el = inputEl.value;
  if (!el) return null;
  const pos = el.selectionStart;
  const before = text.value.slice(0, pos);
  const at = before.lastIndexOf('@');
  if (at < 0) return null;
  const frag = before.slice(at + 1);
  if (!TOKEN_RE.test(frag)) return null;
  const segStart = Math.max(frag.lastIndexOf(',') + 1, 0);
  const seg = frag.slice(segStart).trimStart();
  const m = seg.match(SUFFIX_RE);
  const filter = m ? seg.slice(0, seg.length - m[0].length) : seg;
  return {
    start: at,
    end: pos,
    segmentStart: at + 1 + segStart + (frag.slice(segStart).length - frag.slice(segStart).trimStart().length),
    filter,
    suffix: m ? m[0] : '',
    exact: false,
  };
}

let pickerDebounce: ReturnType<typeof setTimeout> | null = null;
function updatePicker(): void {
  const token = currentToken();
  if (!token || !chat.projectDir || !isTauri) {
    pickerOpen.value = false;
    activeToken.value = null;
    return;
  }
  activeToken.value = token;
  if (pickerDebounce) clearTimeout(pickerDebounce);
  pickerDebounce = setTimeout(() => void fetchPickerItems(token), 120);
}
onBeforeUnmount(() => {
  if (pickerDebounce) clearTimeout(pickerDebounce);
});

async function fetchPickerItems(token: RefToken): Promise<void> {
  const seq = ++pickerSeq;
  try {
    const items = await cmd<string[]>(
      'workspace_files',
      { root: chat.projectDir, filter: token.filter },
      [],
    );
    if (seq !== pickerSeq) return; // superseded by a newer keystroke
    // Exact (case-insensitive) match = reference complete → close (§3.3).
    if (items.some((p) => p.toLowerCase() === token.filter.toLowerCase() && token.filter.length > 0)) {
      pickerOpen.value = false;
      return;
    }
    if (items.length === 0) {
      pickerOpen.value = false;
      return;
    }
    pickerItems.value = items;
    pickerIndex.value = 0;
    pickerOpen.value = true;
  } catch {
    pickerOpen.value = false;
  }
}

/** Replace only the current path segment; keep comma-prefix and :from-to. */
function pickPicker(i: number): void {
  const token = activeToken.value;
  const path = pickerItems.value[i];
  if (!token || !path) {
    pickerOpen.value = false;
    return;
  }
  const before = text.value.slice(0, token.segmentStart);
  const after = text.value.slice(token.end);
  text.value = before + path + token.suffix + after;
  pickerOpen.value = false;
  const pos = before.length + path.length + token.suffix.length;
  void Promise.resolve().then(() => {
    if (inputEl.value) {
      inputEl.value.selectionStart = inputEl.value.selectionEnd = pos;
      inputEl.value.focus();
    }
  });
}

watch(text, () => {
  if (!composing.value) updatePicker();
});

// ---- @ expansion at send time (§3.3 refBlock) ----
const EXPAND_RE = /@([^\s@]+(?:, ?[^\s@,]+)*)/g;

interface RangeOk {
  ok: boolean;
  lines: { n: number; text: string }[];
  totalLines: number;
  truncated: boolean;
}
interface RangeErr {
  ok: boolean;
  error: string;
  totalLines?: number;
}

function parseRefPart(part: string): { path: string; from: number; to: number } {
  const m = part.match(SUFFIX_RE);
  if (!m) return { path: part, from: 0, to: 0 };
  const [a, b] = m[1].split('-');
  return {
    path: part.slice(0, part.length - m[0].length),
    from: Number(a),
    to: b === undefined ? 0 : Number(b),
  };
}

async function expandOne(part: string): Promise<string> {
  const { path, from, to } = parseRefPart(part.trim());
  if (!path) return '';
  const label =
    from <= 0 ? '全文' : to <= 0 ? `第 ${from} 行` : `第 ${from}-${to} 行`;
  let res: RangeOk | RangeErr;
  try {
    res = await cmd<RangeOk | RangeErr>('read_file_range', {
      root: chat.projectDir,
      relPath: path,
      from,
      to,
    });
  } catch {
    res = { ok: false, error: 'unreadable' };
  }
  if (!res.ok) {
    const err = res as RangeErr;
    const why =
      err.error === 'escape'
        ? '路径超出工作区，已拒绝'
        : err.error === 'binary'
          ? '二进制文件，已跳过'
          : err.error === 'range'
            ? `行范围超出文件（共 ${err.totalLines ?? 0} 行）`
            : '文件不存在或不可读';
    return `【引用文件：${path}：${why}】`;
  }
  const ok = res as RangeOk;
  const lines = ok.lines.map((l) => `  ${l.n}  ${l.text}`).join('\n');
  const trunc = ok.truncated ? `\n  ${REF_INJECT_NOTE}` : '';
  return `【引用文件：${path}，${label}】\n${lines}${trunc}\n【引用结束】`;
}

async function expandReferences(input: string): Promise<string> {
  const matches = [...input.matchAll(EXPAND_RE)];
  if (matches.length === 0) return input;
  let out = '';
  let last = 0;
  for (const m of matches) {
    const idx = m.index ?? 0;
    out += input.slice(last, idx);
    const parts = m[1].split(/, ?/);
    const blocks: string[] = [];
    for (const p of parts) blocks.push(await expandOne(p));
    out += blocks.filter(Boolean).join('\n');
    last = idx + m[0].length;
  }
  out += input.slice(last);
  return out;
}

// ---- prompt templates (§3.6) ----
interface PromptRow {
  id: string;
  name: string;
  text: string;
}
const templates = ref<PromptRow[]>([]);
const templateOpen = ref(false);

async function loadTemplates(): Promise<void> {
  if (!isTauri) return;
  try {
    templates.value = await cmd<PromptRow[]>('prompts_list', undefined, []);
  } catch (e) {
    console.warn('[composer] prompts_list failed', e);
  }
}

function toggleTemplates(): void {
  templateOpen.value = !templateOpen.value;
  if (templateOpen.value) void loadTemplates();
}

function insertTemplate(body: string): void {
  templateOpen.value = false;
  const el = inputEl.value;
  const s = el ? el.selectionStart : text.value.length;
  const t = el ? el.selectionEnd : text.value.length;
  // Newline guard: prepend \n when the preceding text doesn't end with one.
  const prefix = s > 0 && !text.value.slice(0, s).endsWith('\n') ? '\n' : '';
  text.value = text.value.slice(0, s) + prefix + body + text.value.slice(t);
  truncateInput(); // unified 64K entry
  void Promise.resolve().then(() => {
    if (el) {
      const pos = s + prefix.length + body.length;
      el.selectionStart = el.selectionEnd = pos;
      el.focus();
    }
  });
}

async function saveAsTemplate(): Promise<void> {
  const body = text.value.trim();
  if (!body) return;
  templateOpen.value = false;
  const name = body.split('\n')[0].slice(0, 20);
  try {
    await cmd('prompt_add', { name, text: body });
  } catch (e) {
    console.warn('[composer] prompt_add failed', e);
  }
}

// ---- permission mode (§3.7) ----
const MODE_IDS = ['default', 'plan', 'auto', 'yolo'];
const MODE_LABELS = ['需批准', '计划', '自动', 'YOLO'];
const modeIndex = computed(() => Math.max(0, MODE_IDS.indexOf(prefs.permissionMode)));

function onModeChange(i: number): void {
  void prefs.setPermissionMode(MODE_IDS[i]);
}

// ---- send (§3.2) ----
const sendEnabled = computed(() => {
  if (!chat.sessionId) return false;
  if (chat.status.busy && chat.status.queueLength >= 10) return false;
  return text.value.trim().length > 0 || chat.attachments.length > 0;
});

async function send(): Promise<void> {
  const draft = text.value.trim();
  if (!draft && chat.attachments.length === 0) return;
  if (!chat.sessionId) return;
  const expanded = draft ? await expandReferences(draft) : draft;
  const ok = await chat.send(expanded, [...chat.attachments]);
  if (ok) {
    text.value = '';
    chat.clearAttachments();
    pickerOpen.value = false;
  }
  inputEl.value?.focus();
}

// ---- "基于此提问" one-shot prefill ----
import { useSessionsStore } from '../../stores/sessions';
const sessionsStore = useSessionsStore();
watch(
  () => sessionsStore.pendingComposerText,
  (v) => {
    if (!v) return;
    text.value = v;
    sessionsStore.pendingComposerText = '';
    void Promise.resolve().then(() => inputEl.value?.focus());
  },
);

onMounted(() => inputEl.value?.focus());
</script>

<template>
  <div class="composer">
    <!-- truncation notice -->
    <div v-if="notice" class="composer__notice" :style="{ fontSize: prefs.fs(11) + 'px' }">
      {{ notice }}
    </div>
    <!-- char counter (top of the field — invisible at the bottom of small windows) -->
    <div
      v-if="showCounter"
      class="composer__counter"
      :class="{ full: text.length >= MAX_LEN }"
      :style="{ fontSize: prefs.fs(10) + 'px' }"
    >
      {{ text.length }} / {{ MAX_LEN }}
    </div>

    <!-- @ picker popup (non-modal, focus stays in the field) -->
    <div v-if="pickerOpen" class="composer__picker">
      <div
        v-for="(item, i) in pickerItems.slice(0, 12)"
        :key="item"
        class="composer__picker-row"
        :class="{ active: i === pickerIndex }"
        :style="{ fontSize: prefs.fs(12) + 'px' }"
        @mousedown.prevent="pickPicker(i)"
        @mouseenter="pickerIndex = i"
      >
        {{ item }}
      </div>
      <div class="composer__picker-hint" :style="{ fontSize: prefs.fs(10) + 'px' }">
        ↑↓ 选择 · Enter 确认 · Esc 关闭 · 选中后可直接补 :起-止 行号，逗号可连引多个
      </div>
    </div>

    <!-- template menu (anchored above the button, right edges aligned) -->
    <div v-if="templateOpen" class="composer__templates">
      <div class="composer__templates-list">
        <div
          v-for="t in templates.slice(0, 8)"
          :key="t.id"
          class="composer__templates-row"
          :style="{ fontSize: prefs.fs(12) + 'px' }"
          @click="insertTemplate(t.text)"
        >
          {{ t.name }}
        </div>
        <div v-if="templates.length === 0" class="composer__templates-row dim" :style="{ fontSize: prefs.fs(12) + 'px' }">
          （暂无模板）
        </div>
      </div>
      <div
        class="composer__templates-row composer__templates-save"
        :class="{ dim: !text.trim() }"
        :style="{ fontSize: prefs.fs(12) + 'px' }"
        @click="text.trim() && saveAsTemplate()"
      >
        保存当前输入为模板
      </div>
    </div>

    <textarea
      ref="inputEl"
      v-model="text"
      class="composer__field"
      placeholder="输入消息…（@ 引用文件，Ctrl+V 粘贴图片）"
      :style="{ fontSize: prefs.fs(14) + 'px' }"
      @input="onInput"
      @keydown="onKeydown"
      @paste="onPaste"
      @compositionstart="composing = true"
      @compositionend="composing = false; onInput()"
      @click="updatePicker"
      @keyup="updatePicker"
    ></textarea>

    <div class="composer__side">
      <div class="composer__tools">
        <button class="composer__tool" title="添加附件" @click="pickFiles">📎</button>
        <button class="composer__tool" title="提示词模板" @click="toggleTemplates">模板</button>
        <WarDropdown
          class="composer__mode"
          :options="MODE_LABELS"
          :model-value="modeIndex"
          drop-up
          :text-size="prefs.fs(12)"
          @update:model-value="onModeChange"
        />
      </div>
      <WarButton
        :width="120"
        :text="chat.sendLabel"
        :enabled="sendEnabled"
        @activated="send"
      />
    </div>
  </div>
</template>

<style scoped>
.composer {
  position: relative;
  display: flex;
  gap: 8px;
  height: 100%;
  min-height: 0;
}

.composer__notice {
  position: absolute;
  left: 4px;
  right: 4px;
  top: -22px;
  z-index: 40;
  color: var(--war-gold);
  background: #0d1116f0;
  border: 1px solid #6a5a3f;
  border-radius: 3px;
  padding: 2px 8px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.composer__counter {
  position: absolute;
  right: 148px;
  top: 2px;
  z-index: 5;
  color: var(--war-text-muted);
  user-select: none;
}

.composer__counter.full {
  color: #ff8a70;
}

.composer__picker {
  position: absolute;
  left: 0;
  bottom: calc(100% + 4px);
  z-index: 46;
  width: min(520px, 90%);
  max-height: 320px;
  overflow-y: auto;
  border: 14px 16px 13px 20px solid transparent;
  border-image: url('/assets/ui/dropdown/dropdown_panel.png') 14 16 13 20 stretch;
  box-sizing: border-box;
  background: #0d1116f0;
  padding: 8px;
}

.composer__picker-row {
  padding: 4px 8px;
  color: var(--war-text-dim);
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  user-select: none;
}

.composer__picker-row.active {
  color: var(--war-gold);
  background: #32509633;
}

.composer__picker-hint {
  color: var(--war-text-faint);
  padding: 6px 8px 2px;
  border-top: 1px solid #2a3344;
  margin-top: 4px;
  font-family: SimSun, serif;
}

.composer__templates {
  position: absolute;
  right: 0;
  bottom: calc(100% + 4px);
  z-index: 46;
  width: 260px;
  border: 14px 16px 13px 20px solid transparent;
  border-image: url('/assets/ui/dropdown/dropdown_panel.png') 14 16 13 20 stretch;
  box-sizing: border-box;
  background: #0d1116f0;
  padding: 8px;
}

.composer__templates-list {
  max-height: calc(8 * 26px);
  overflow-y: auto;
}

.composer__templates-row {
  padding: 4px 8px;
  color: var(--war-text-dim);
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  user-select: none;
}

.composer__templates-row:hover:not(.dim) {
  color: var(--war-gold);
  background: #32509633;
}

.composer__templates-row.dim {
  color: var(--war-text-faint);
}

.composer__templates-save {
  border-top: 1px solid #2a3344;
  margin-top: 4px;
  padding-top: 6px;
  color: var(--war-gold);
}

.composer__field {
  flex: 1;
  min-width: 0;
  resize: none;
  background: #10141f;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: var(--war-text);
  font-family: SimSun, serif;
  padding: 6px 8px;
  outline: none;
}

.composer__field:focus {
  border-color: var(--war-gold-input);
}

.composer__field::placeholder {
  color: var(--war-text-faint);
}

.composer__side {
  flex: none;
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  align-items: flex-end;
  gap: 6px;
}

.composer__tools {
  display: flex;
  align-items: center;
  gap: 6px;
}

.composer__tool {
  height: 26px;
  min-width: 40px;
  padding: 0 8px;
  background: #10141f;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-size: 12px;
}

.composer__tool:hover {
  border-color: var(--war-gold-input);
  color: var(--war-gold-bright);
}

.composer__mode {
  width: 96px;
}
</style>

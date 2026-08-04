<script setup lang="ts">
// Composer (features/chat.md §3): 64K cap with the three truncation paths,
// Enter/Shift+Enter with IME guard, @ file-reference picker with :from-to
// line suffix, Ctrl+V image paste → media cache + attachment bar (≤6) plus a
// ![](<path>) embed in the draft, OS file drag-drop (image → embed +
// attachment, project file → @reference), permission-mode dropdown,
// send/enqueue button.
//
// <selection>…</selection> quote blocks live in the QuoteBar ABOVE the input
// (chat.composerQuotes, full bodies) — the textarea here holds plain text
// only; on send the quotes are re-wrapped into tags and prepended.
//
// Send path: the draft keeps short @tokens; only at send time each token is
// expanded through read_file_range into a 【引用文件：…】 block (§3.3) and the
// expanded text + attachment paths go to send_prompt. The draft and
// attachments are cleared ONLY when the backend accepts (§3.2).
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { cmd, isTauri } from '../../lib/tauri';
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';
import WarDropdown from '../war/WarDropdown.vue';
import WarButton from '../war/WarButton.vue';
import WarScrollBar from '../war/WarScrollBar.vue';
import ComposerExpandDialog from './ComposerExpandDialog.vue';

const MAX_LEN = 64000;
const REF_INJECT_NOTE = '…（文件超过 200KB，已截断）';

const chat = useChatStore();
const prefs = usePrefsStore();

const text = ref('');
const inputEl = ref<HTMLTextAreaElement | null>(null);
const rootEl = ref<HTMLElement | null>(null);
const composing = ref(false);

// ---- <selection>…</selection> quote blocks (quote bar above the input) ----
// Quote blocks are NOT embedded in the textarea anymore: the store keeps the
// full bodies in chat.composerQuotes and QuoteBar renders one chip each above
// the composer. The textarea holds plain text only. Send re-wraps the quotes
// into <selection>…</selection> tags; prefills that arrive as tagged strings
// (基于选中文本提问 / 分支会话预填) are split back into quotes + text here.

/** 带 <selection>…</selection> 标签的预填文本 → 引用块数组 + 纯正文。 */
function parseQuoted(src: string): { quotes: string[]; text: string } {
  const quotes: string[] = [];
  const text = src
    .split(/(<selection>[\s\S]*?<\/selection>)/g)
    .map((p) => {
      const m = p.match(/^<selection>([\s\S]*?)<\/selection>$/);
      if (!m) return p;
      const body = m[1].replace(/^\r?\n/, '').replace(/\r?\n$/, '');
      if (body) quotes.push(body);
      return '';
    })
    .join('')
    .replace(/^\r?\n+/, '');
  return { quotes, text };
}

// ---- terminal command mode (`!` prefix, cmd.rs) ----
// Leading `!` (or fullwidth `！` — IME users type it from the Chinese
// keyboard layout) routes the input to LOCAL execution instead of the
// agent: Enter runs the command, output streams back as a kind=="command"
// chat row (term://output), never entering the agent context.
// `![` is excluded: a pasted/dropped image embeds as `![name](<path>)` at
// the cursor and must stay a message, not a command. A real command
// starting with `[` (test shorthand) can still be run as `! [ … ]`.
const TERM_PREFIX_RE = /^[!！](?!\[)/;

const termMode = computed(() => {
  const t = text.value.trimStart();
  return TERM_PREFIX_RE.test(t) && t.length > 1;
});
const termCmd = computed(() => text.value.trimStart().replace(TERM_PREFIX_RE, '').trim());

function maybeTermHint(): void {
  try {
    if (localStorage.getItem('wardex-term-hint')) return;
    localStorage.setItem('wardex-term-hint', '1');
    showNotice('终端命令模式：Enter 执行 · 删除 ! 或 ！ 恢复消息输入');
  } catch {
    /* localStorage unavailable — hint skipped */
  }
}

watch(termMode, (v, p) => {
  if (v && !p) maybeTermHint();
});

// ---- per-session draft (text + quotes + attachments), in-memory ----
// Switching sessions saves the outgoing draft under the old sessionId and
// restores the target's; deleted/cleared sessions simply have no entry.
watch(
  () => chat.sessionId,
  (id, prev) => {
    if (prev && prev !== id) {
      chat.saveDraft(prev, text.value, chat.attachments, chat.composerQuotes);
    }
    const d = id ? chat.drafts[id] : undefined;
    text.value = d?.text ?? '';
    chat.composerQuotes = [...(d?.quotes ?? [])];
    chat.attachments = [...(d?.attachments ?? [])];
    pickerOpen.value = false;
    slashOpen.value = false;
  },
);

// ---- external ref insert (preview context menu → @token) ----
// The preview dialog queues a token through chat.pendingRefInsert; append it
// to the draft at the end and park the cursor after it. The existing @-picker
// machinery then resolves the path normally (exact match closes the picker).
watch(
  () => [chat.pendingRefInsert.seq, chat.pendingRefInsert.token] as const,
  ([seq, token]) => {
    if (!seq || !token) return;
    const sep = text.value && !text.value.endsWith('\n') ? ' ' : '';
    text.value += sep + token;
    void Promise.resolve().then(() => {
      if (inputEl.value) {
        inputEl.value.selectionStart = inputEl.value.selectionEnd = text.value.length;
        inputEl.value.focus();
      }
    });
  },
);

// ---- popup anchoring ----
// Popups (slash / @ pickers) float ABOVE the composer, but the composer lives
// inside WarFrame's overflow:hidden content layer — position:absolute there
// is clipped. position:fixed escapes it (no transformed ancestor), anchored
// to the composer rect, recomputed on every open.
const popupPos = ref({ left: 0, bottom: 0 });

function anchorPopup(): void {
  const r = rootEl.value?.getBoundingClientRect();
  if (!r) return;
  popupPos.value = { left: r.left, bottom: window.innerHeight - r.top + 4 };
}

const popupStyle = computed(() => ({
  position: 'fixed' as const,
  left: popupPos.value.left + 'px',
  bottom: popupPos.value.bottom + 'px',
}));

// ---- 64K cap ----
const notice = ref('');
let noticeTimer: ReturnType<typeof setTimeout> | null = null;

function showNotice(msg?: string): void {
  notice.value = msg ?? `已达输入上限（${MAX_LEN} 字），超出部分已截断；大段内容请作为附件发送`;
  if (noticeTimer) clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => (notice.value = ''), 6000);
}
onBeforeUnmount(() => {
  if (noticeTimer) clearTimeout(noticeTimer);
});

const showCounter = computed(() => text.value.length > MAX_LEN * 0.75);

/** Unified truncation entry (typing fallback + paste). */
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
    if (termMode.value) {
      e.preventDefault();
      void runTermCommand();
      return;
    }
    if (slashOpen.value) {
      e.preventDefault();
      pickSlash(slashIndex.value);
      return;
    }
    if (pickerOpen.value) {
      e.preventDefault();
      pickPicker(pickerIndex.value);
      return;
    }
    e.preventDefault();
    void send();
    return;
  }
  if (slashOpen.value) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      slashIndex.value = Math.min(slashIndex.value + 1, slashItems.value.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      slashIndex.value = Math.max(slashIndex.value - 1, 0);
    } else if (e.key === 'Tab') {
      e.preventDefault();
      pickSlash(slashIndex.value);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      slashOpen.value = false;
    }
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

// ---- markdown image embed + file drag-drop ----
// Pasted/dropped images go to the attachment bar (the model sees them) AND
// leave a ![name](<path>) embed in the draft so the sent bubble renders the
// image inline (ChatBubble skips re-thumbnailing embedded paths). Dropped
// non-image files inside the project insert an @reference (§3.3); outside
// files are rejected with a notice.
const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'];

function isImagePath(p: string): boolean {
  const ext = p.split('.').pop()?.toLowerCase() ?? '';
  return IMAGE_EXTS.includes(ext);
}

function fileNameOf(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

function mdImageFor(p: string): string {
  // markdown-it strips '\' as escape chars inside the destination — store
  // Windows paths with forward slashes (the asset protocol accepts both).
  return `![${fileNameOf(p)}](<${p.replace(/\\/g, '/')}>)`;
}

/** Project-relative path for @ references; '' when the file is outside. */
function relUnderProject(p: string): string {
  const root = (chat.projectDir || '').replace(/[\\/]+$/, '');
  if (!root) return '';
  const norm = p.replace(/\//g, '\\');
  const r = root.replace(/\//g, '\\');
  if (!norm.toLowerCase().startsWith(r.toLowerCase() + '\\')) return '';
  return norm.slice(r.length + 1).replace(/\\/g, '/');
}

/** Insert at the cursor (replacing any selection), keeping the caret after it. */
function insertAtCursor(snippet: string): void {
  const el = inputEl.value;
  const s = el ? el.selectionStart : text.value.length;
  const t = el ? el.selectionEnd : text.value.length;
  text.value = text.value.slice(0, s) + snippet + text.value.slice(t);
  truncateInput(); // an embed past the cap is clipped like typed input
  void Promise.resolve().then(() => {
    if (el) {
      el.selectionStart = el.selectionEnd = s + snippet.length;
      el.focus();
    }
  });
}

/** OS file drop (Tauri window drag-drop event, scoped to the composer rect). */
function onDropPaths(paths: string[]): void {
  let snippet = '';
  const images: string[] = [];
  for (const p of paths) {
    if (isImagePath(p)) {
      images.push(p);
      snippet += (snippet ? '\n' : '') + mdImageFor(p);
    } else {
      const rel = relUnderProject(p);
      if (rel) snippet += (snippet ? '\n' : '') + '@' + rel;
      else showNotice(`「${fileNameOf(p)}」不在项目目录内，无法 @ 引用（图片可直接拖入）`);
    }
  }
  if (images.length) chat.addAttachments(images);
  if (snippet) insertAtCursor(snippet);
}

let unlistenDrag: (() => void) | null = null;
onMounted(async () => {
  if (!isTauri) return;
  try {
    unlistenDrag = await getCurrentWebviewWindow().onDragDropEvent((ev) => {
      if (ev.payload.type !== 'drop') return;
      const r = rootEl.value?.getBoundingClientRect();
      if (!r) return;
      const scale = window.devicePixelRatio || 1; // payload position is physical px
      const x = ev.payload.position.x / scale;
      const y = ev.payload.position.y / scale;
      if (x < r.left || x > r.right || y < r.top || y > r.bottom) return;
      onDropPaths(ev.payload.paths);
    });
  } catch (e) {
    console.warn('[composer] drag-drop listener failed', e);
  }
});
onBeforeUnmount(() => {
  unlistenDrag?.();
});

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
        if (path) {
          chat.addAttachments([path]);
          insertAtCursor(mdImageFor(path)); // inline render in the sent bubble
        }
      } catch (err) {
        console.warn('[composer] save_clipboard_image failed', err);
      }
      return;
    }
  }
  // Plain text: measure first, insert only the head that fits.
  const pasted = e.clipboardData?.getData('text/plain') ?? '';
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
/** keyup：刷新 @picker。 */
function onKeyup(): void {
  updatePicker();
}
function updatePicker(): void {  // Terminal mode: `@` references and `/` commands mean nothing to a local
  // shell — disable both pickers.
  if (termMode.value) {
    pickerOpen.value = false;
    activeToken.value = null;
    return;
  }
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
    anchorPopup();
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

// ---- slash command completion (acp://commands, available_commands_update) ----
// The agent owns execution: picking a command only completes the draft text
// (`/name `), and it is sent as a normal prompt.
const slashOpen = ref(false);
const slashIndex = ref(0);

/** Commands matching the draft when it is exactly "/<filter>" (first token,
 * no whitespace yet). Empty unless the agent advertised commands. In
 * terminal mode the `/` picker is disabled (local shell has no agent
 * commands). */
const slashItems = computed(() => {
  if (termMode.value) return [];
  const m = text.value.match(/^\/(\S*)$/);
  if (!m) return [];
  const f = m[1].toLowerCase();
  return chat.commands.filter((c) => c.name.toLowerCase().includes(f));
});

watch(slashItems, (items) => {
  if (items.length > 0 && !slashOpen.value) anchorPopup();
  slashOpen.value = items.length > 0;
  slashIndex.value = 0;
});

function pickSlash(i: number): void {
  const cmd = slashItems.value[i];
  slashOpen.value = false;
  if (!cmd) return;
  text.value = `/${cmd.name} `;
  void Promise.resolve().then(() => {
    if (inputEl.value) {
      inputEl.value.selectionStart = inputEl.value.selectionEnd = text.value.length;
      inputEl.value.focus();
    }
  });
}

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

// ---- permission mode (§3.7) ----
// The dropdown prefers the agent's OWN mode picker (configOptions `id=="mode"`,
// e.g. opencode exposes its agents build/plan/general…, kimi its own ids):
// picking sends setConfigOption("mode", value) verbatim. Without an advertised
// picker it falls back to the global default/plan/auto/yolo list, which the
// backend maps per provider (provider.rs mapMode) before sending.
const FALLBACK_MODE_IDS = ['default', 'plan', 'auto', 'yolo'];
const FALLBACK_MODE_LABELS = ['需批准', '计划', '自动', 'YOLO'];

const modeOpt = computed(() => chat.configOptions.find((o) => o.id === 'mode'));
const modeChoices = computed(() =>
  (modeOpt.value?.options ?? []).map((o) => ({ value: o.value, name: o.name })),
);
const modeIsDynamic = computed(() => modeChoices.value.length > 0);

const modeLabels = computed(() =>
  modeIsDynamic.value ? modeChoices.value.map((o) => o.name) : FALLBACK_MODE_LABELS,
);
const modeIndex = computed(() => {
  if (modeIsDynamic.value) {
    const cur = modeOpt.value?.currentValue ?? '';
    return Math.max(0, modeChoices.value.findIndex((o) => o.value === cur));
  }
  return Math.max(0, FALLBACK_MODE_IDS.indexOf(prefs.permissionMode));
});

function onModeChange(i: number): void {
  if (modeIsDynamic.value) {
    const o = modeChoices.value[i];
    if (o && o.value !== modeOpt.value?.currentValue) void chat.setConfigOption('mode', o.value);
  } else {
    void prefs.setPermissionMode(FALLBACK_MODE_IDS[i]);
  }
}

// ---- send (§3.2) / terminal run (`!`) ----
const sendEnabled = computed(() => {
  if (!chat.sessionId) return false;
  if (termMode.value) return termCmd.value.length > 0;
  if (chat.status.busy && chat.status.queueLength >= 10) return false;
  return (
    text.value.trim().length > 0 ||
    chat.composerQuotes.length > 0 ||
    chat.attachments.length > 0
  );
});

/** Enter path: terminal mode runs locally (cmd.rs), otherwise the normal
 * agent send. */
async function submit(): Promise<void> {
  if (termMode.value) await runTermCommand();
  else await send();
}

async function runTermCommand(): Promise<void> {
  if (!chat.sessionId) {
    showNotice('没有打开的会话');
    return;
  }
  if (!chat.projectDir) {
    showNotice('请先绑定项目目录');
    return;
  }
  if (!termCmd.value) {
    showNotice('请输入要执行的命令（! 后跟命令）');
    return;
  }
  const ok = await chat.runCommand(termCmd.value);
  if (ok) {
    text.value = '';
    chat.saveDraft(chat.sessionId, '', []); // executed → drop any stored draft
    pickerOpen.value = false;
  }
  inputEl.value?.focus();
}

/** 引用块数组 → <selection>…</selection> 标签文本（发往会话）。 */
function quotesToTags(quotes: string[]): string {
  return quotes.map((q) => `<selection>\n${q}\n</selection>`).join('\n');
}

async function send(): Promise<void> {
  if (!chat.sessionId) return;
  const quoted = quotesToTags(chat.composerQuotes);
  const draft = text.value.trim();
  if (!quoted && !draft && chat.attachments.length === 0) return;
  // Only the typed text undergoes @reference expansion — quote bodies are
  // user-selected content and must not be treated as tokens.
  const expanded = draft ? await expandReferences(draft) : draft;
  const full = [quoted, expanded].filter(Boolean).join('\n');
  const ok = await chat.send(full, [...chat.attachments]);
  if (ok) {
    text.value = '';
    chat.clearComposerQuotes();
    chat.clearAttachments();
    chat.saveDraft(chat.sessionId, '', []); // sent → drop any stored draft
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
    const { quotes, text: t } = parseQuoted(v);
    chat.composerQuotes = quotes;
    text.value = t;
    sessionsStore.pendingComposerText = '';
    void Promise.resolve().then(() => inputEl.value?.focus());
  },
);

onMounted(() => inputEl.value?.focus());

// ---- expand-to-dialog (⛶): edit the draft in a roomy popup ----
const expandOpen = ref(false);

function onExpandConfirm(v: string): void {
  text.value = v.slice(0, MAX_LEN);
  void Promise.resolve().then(() => inputEl.value?.focus());
}
</script>

<template>
  <div ref="rootEl" class="composer">
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

    <!-- slash command popup (non-modal, same style as the @ picker) -->
    <div v-if="slashOpen" class="composer__picker" :style="popupStyle">
      <div
        v-for="(item, i) in slashItems.slice(0, 12)"
        :key="item.name"
        class="composer__picker-row"
        :class="{ active: i === slashIndex }"
        :style="{ fontSize: prefs.fs(12) + 'px' }"
        @mousedown.prevent="pickSlash(i)"
        @mouseenter="slashIndex = i"
      >
        <span class="composer__slash-name">/{{ item.name }}</span>
        <span v-if="item.description" class="composer__slash-desc">{{ item.description }}</span>
      </div>
      <div class="composer__picker-hint" :style="{ fontSize: prefs.fs(10) + 'px' }">
        ↑↓ 选择 · Enter 确认 · Esc 关闭 · 命令由 agent 执行，选中后继续输入参数
      </div>
    </div>

    <!-- @ picker popup (non-modal, focus stays in the field) -->
    <div v-if="pickerOpen" class="composer__picker" :style="popupStyle">
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

    <div class="composer__field-wrap">
      <textarea
        ref="inputEl"
        v-model="text"
        class="composer__field"
        :class="{ term: termMode }"
        :placeholder="
          termMode ? '执行终端命令…（Enter 执行，不发给 AI）' : '输入消息…（@ 引用文件，Ctrl+V 粘贴图片）'
        "
        :style="{ fontSize: prefs.fs(14) + 'px' }"
        @input="onInput"
        @keydown="onKeydown"
        @paste="onPaste"
        @compositionstart="composing = true"
        @compositionend="composing = false; onInput()"
        @click="updatePicker"
        @keyup="onKeyup"
      ></textarea>
      <span
        class="composer__expand"
        :style="{ fontSize: prefs.fs(16) + 'px' }"
        title="放大输入框，在弹框中编辑"
        @click="expandOpen = true"
        >⛶</span
      >
      <WarScrollBar :target="inputEl" :scale="0.7" />
    </div>

    <div class="composer__side">
      <div class="composer__tools">
        <WarDropdown
          class="composer__mode"
          :options="modeLabels"
          :model-value="modeIndex"
          drop-up
          :text-size="prefs.fs(12)"
          @update:model-value="onModeChange"
        />
      </div>
      <WarButton
        :width="150"
        :art-aspect="5"
        skin="blue"
        :text="termMode ? '▶ 执行' : chat.sendLabel"
        :enabled="sendEnabled"
        @activated="submit"
      />
    </div>

    <ComposerExpandDialog
      v-model:open="expandOpen"
      :initial-text="text"
      @confirm="onExpandConfirm"
    />
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
  right: 176px; /* side column (158px) + WC3 scrollbar + gap */
  top: 2px;
  z-index: 5;
  color: var(--war-text-muted);
  user-select: none;
}

.composer__counter.full {
  color: #ff8a70;
}

.composer__picker {
  /* position/left/bottom come from the inline popupStyle (fixed anchoring —
   * WarFrame's overflow:hidden content layer would clip absolute popups) */
  z-index: 60;
  width: min(520px, 90%);
  max-height: 320px;
  overflow-y: auto;
  border-style: solid;
  border-color: transparent;
  border-width: 13px 14px 12px 14px;
  border-image: url('/assets/ui/dropdown/dropdown_panel2.png') 21 23 20 23 fill stretch;
  box-sizing: border-box;
  background: #0d1116f0 padding-box;
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

.composer__slash-name {
  color: var(--war-gold);
}

.composer__slash-desc {
  color: var(--war-text-faint);
  margin-left: 8px;
}

.composer__field-wrap {
  position: relative;
  flex: 1;
  min-width: 0;
  display: flex;
  gap: 4px;
  overflow: hidden;
  background: #10141f;
  border-radius: 2px;
}

.composer__field {
  position: relative;
  z-index: 1;
  flex: 1;
  min-width: 0;
  resize: none;
  background: transparent;
  border: 1px solid #2a3344;
  border-radius: 2px;
  color: var(--war-text);
  caret-color: var(--war-text);
  font-family: SimSun, serif;
  line-height: 1.4;
  padding: 6px 8px;
  outline: none;
  scrollbar-width: none; /* native bar hidden — the WC3 WarScrollBar replaces it */
}

.composer__field:focus {
  border-color: var(--war-gold-input);
}

/* terminal mode: green border + caret so the mode reads at a glance */
.composer__field.term {
  border-color: #3f7a52;
  caret-color: #5cb380;
}

.composer__field.term:focus {
  border-color: #5cb380;
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

.composer__expand {
  position: absolute;
  top: 4px;
  right: 22px; /* left of the (scaled-down) WC3 scrollbar */
  z-index: 6;
  color: var(--war-gold);
  user-select: none;
  line-height: 1;
  padding: 4px 6px; /* generous invisible hit area in the corner */
  background: #10141fcc; /* keep the glyph readable over typed text */
  border-radius: 3px;
  opacity: 0; /* reveal only when the cursor reaches the corner itself */
  transition: opacity 0.15s ease;
}

.composer__expand:hover {
  opacity: 1;
}

.composer__expand:hover {
  color: var(--war-gold-bright);
}

.composer__mode {
  width: 150px;
  height: 30px;
}
</style>

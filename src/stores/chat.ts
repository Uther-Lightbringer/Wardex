// Chat runtime render state (features/chat.md). The Rust session store owns
// the authoritative segments; this store only holds what the message list
// needs to render, plus turn/queue/retry/permission/sub-agent state for the
// ACTIVE session (backend runtimes keep background sessions streaming to
// disk — switching to one re-pulls the full model, self-healing).
//
// Red line R1 — streaming is incremental end to end:
//   - `rows` is a shallowRef. Structural events (new segment, tool upsert,
//     bubbleSet, turn end) assign a new array / row object; same-identity
//     rows do NOT re-render their bubble components (keyed by row id).
//   - Same-kind chunk extension mutates the trailing segment's text IN PLACE
//     (no reactive trigger at all) and appends the chunk to the live DOM text
//     node registered by the streaming bubble (streamTarget). A bubble that
//     is scrolled out of the windowing range is not mounted; when it comes
//     back it renders the full store text — skip-and-self-heal, same as the
//     old ChatBubble.qml:127-128 comment.
//   - Markdown rendering happens only when a segment's turn is final
//     (status done/error/interrupted), once per segment.

import { defineStore } from 'pinia';
import { markRaw, nextTick } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { cmd, isTauri } from '../lib/tauri';
import { useSessionsStore } from './sessions';
import { usePrefsStore } from './prefs';

export interface ChatSegment {
  kind: 'thinking' | 'text' | 'tool';
  text?: string;
  toolCallId?: string;
  [key: string]: unknown;
}

/** Token usage of one finished turn (chat://turn payload / messages.jsonl row). */
export interface TurnUsage {
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  cachedReadTokens?: number;
  cachedWriteTokens?: number;
  thoughtTokens?: number;
}

export interface ChatMessage {
  id: string;
  role: string;
  content: string;
  createdAt: number;
  provider: string;
  status: string;
  thinking: string;
  toolCalls: unknown[];
  segments: ChatSegment[];
  attachments: string[];
  usage?: TurnUsage;
  /** Row marker for non-interactive sends ('reminder'); absent = normal. */
  kind?: string;
  /** Command rows only: process exit code, filled at run end (Rust cmd.rs). */
  exitCode?: number | null;
}

export interface ChatStatus {
  statusText: string;
  busy: boolean;
  queueLength: number;
  retryActive: boolean;
  retryCountdown: number;
  retryAttempt: number;
  retryMax: number;
  lastError: string;
  acpReady: boolean;
  imageSupported: boolean;
}

export interface ConfigOptionChoice {
  value: string;
  name: string;
  description?: string;
}

/** ACP session config option picker (kimi: model / thinking / mode). */
export interface ConfigOption {
  type: string;
  id: string;
  name: string;
  category?: string;
  currentValue: string;
  options: ConfigOptionChoice[];
}

/** ACP available_commands_update entry (raw {name, description?, input?}). */
export interface SlashCommand {
  name: string;
  description?: string;
}

export interface Subagent {
  id: string;
  kind: string;
  title: string;
  status: string;
  children: number;
  childNames: string[];
  summary: string;
  /** Task brief (prompt / swarm args JSON) for the detail dialog. */
  input: string;
  /** Final report (rawOutput), filled on completion. */
  output: string;
  /** CLI-side agent ids (wire transcript dir names), from rawOutput. */
  agentIds: string[];
  /** CLI background-task id from a `task_id:` launch ack (kind === 'task'). */
  taskId: string;
  startedAt: number;
  finishedAt: number;
  /** Last tool_call(_update) touch — stuck detection. */
  lastUpdate: number;
}

export interface QuestionOption {
  option_id: string;
  label: string;
}

/** One AskUserQuestion question, grouped from the q{n}_* option namespace
 * (parsed in Rust acp/types.rs; empty array for regular approvals). */
export interface QuestionGroup {
  index: number;
  text: string;
  multi_select: boolean;
  options: QuestionOption[];
  skip_id: string;
}

export interface PermissionRequest {
  sessionId: string;
  requestId: number;
  params: {
    toolCall?: {
      title?: string;
      kind?: string;
      content?: { content?: { text?: string } }[];
      rawInput?: Record<string, unknown>;
    };
    options?: { optionId?: string; id?: string; name?: string; kind?: string }[];
  };
  /** AskUserQuestion groups (kimi q{n}_* wire format); absent/empty = plain approval. */
  questions?: QuestionGroup[];
}

export interface SessionMeta {
  id: string;
  title: string;
  agentId: string;
  agentName: string;
  provider: string;
  workDir: string;
  projectDir: string;
  createdAt: number;
  updatedAt: number;
  messageCount: number;
  model: string;
  status: string;
  pinned?: boolean;
  /** 子会话的父会话 id；顶层会话无此字段。 */
  parentId?: string;
  /** 子会话的分支点消息 id（从父会话哪条消息 fork 出来的）。 */
  sourceMessageId?: string;
  summary: string;
}

const IDLE_STATUS: ChatStatus = {
  statusText: '就绪',
  busy: false,
  queueLength: 0,
  retryActive: false,
  retryCountdown: 0,
  retryAttempt: 0,
  retryMax: 3,
  lastError: '',
  acpReady: false,
  imageSupported: false,
};

/** 引用块长度上限：超出截断并提示（对齐 @文件 的 REF_INJECT_NOTE）。 */
const SELECTION_QUOTE_MAX = 8000;

/** 子会话层级上限（与后端 MAX_SUB_DEPTH 一致）：顶层=1，最多 3 级。 */
export const MAX_SUB_DEPTH = 3;

const MODE_SUFFIX: Record<string, string> = {
  default: ' · 需批准',
  plan: ' · 计划',
  auto: ' · 自动',
  yolo: ' · YOLO',
};

/** Registration point of the live DOM text node of the streaming segment. */
interface StreamTarget {
  rowId: string;
  kind: string;
  el: HTMLElement;
}

/** Live DOM node of a streaming command row's output <pre> (term://output). */
interface TermTarget {
  rowId: string;
  el: HTMLElement;
}

let listenersReady = false;
const unlisteners: UnlistenFn[] = [];

export const useChatStore = defineStore('chat', {
  state: () => ({
    sessionId: '',
    projectDir: '',
    meta: null as SessionMeta | null,
    rows: [] as ChatMessage[],
    status: { ...IDLE_STATUS } as ChatStatus,
    subagents: [] as Subagent[],
    /** ACP config option pickers for the active session (kimi: model /
     * thinking / mode); refreshed by acp://configOptions. */
    configOptions: [] as ConfigOption[],
    /** Slash commands per session (acp://commands); keyed so a session switch
     * never loses a background session's list (the event only re-fires when
     * the agent sends it). */
    commandsBySession: {} as Record<string, SlashCommand[]>,
    permission: null as PermissionRequest | null,
    retry: { active: false, countdown: 0, attempt: 0, maxAttempts: 3 },
    /** Mirror of the send queue; rebuilt from the backend snapshot on switch. */
    queueMirror: [] as string[],
    queueOpen: false,
    /** Bumped on acp://turn — panels with refreshOn turnEnd watch this. */
    turnSeq: 0,
    /** Bumped to request a scroll-to-end (send / session switch). */
    scrollSeq: 0,
    /** Bumped by the 刷新工作区 action-bay button (files/git panels). */
    workspaceRefreshSeq: 0,
    /** Pending attachment paths (attachment bar floats above the composer). */
    attachments: [] as string[],
    /** Pending <selection>…</selection> quote bodies — the quote bar floats
     * above the composer; on send they are wrapped back into tags. */
    composerQuotes: [] as string[],
    /** Per-session composer drafts (text + quotes + attachments), in-memory
     * only — switching sessions saves the old draft and restores the
     * target's. */
    drafts: {} as Record<string, { text: string; quotes: string[]; attachments: string[] }>,
    /** Live DOM node of the streaming segment (R1 incremental append). */
    streamTarget: null as StreamTarget | null,
    /** Live DOM node of a streaming command row's output (term://output). */
    termTarget: null as TermTarget | null,
    /** rowId → runId of live terminal runs (cancel button lookup; fed by
     * term://output, dropped on term://exit). */
    runsByRow: {} as Record<string, string>,
    /** File preview dialog state (FilesPanel opens, dialog renders). */
    previewPath: '',
    /** Line to jump to after the preview opens (0 = top of file). */
    previewLine: 0,
    /** Preview right-click → composer insert queue (@token). The Composer
     * watches seq/token and appends it to its draft; seq makes a repeated
     * identical token fire the watcher again. */
    pendingRefInsert: { seq: 0, token: '' } as { seq: number; token: string },
  }),
  getters: {
    /** Slash commands of the active session (composer `/` completion). */
    commands(): SlashCommand[] {
      return this.commandsBySession[this.sessionId] ?? [];
    },
    /**
     * The assistant row currently receiving stream chunks. NOT necessarily
     * the last row: a terminal-command row (kind 'command') can be appended
     * while a turn is still streaming, so look for the last in-flight
     * assistant row instead of assuming rows[last].
     */
    streamRow(): ChatMessage | null {
      if (!this.status.busy && !this.status.retryActive) return null;
      for (let i = this.rows.length - 1; i >= 0; i--) {
        const r = this.rows[i];
        if (r.role === 'assistant' && (r.status === 'pending' || r.status === 'streaming')) return r;
      }
      return null;
    },
    sendLabel(): string {
      if (this.status.busy) {
        return this.status.queueLength >= 10 ? '已满' : '入队';
      }
      return '发送';
    },
  },
  actions: {
    /** One-time event wiring (called from ChatPage onMounted). */
    async init(): Promise<void> {
      if (listenersReady || !isTauri) return;
      listenersReady = true;
      const sessions = useSessionsStore();
      const prefs = usePrefsStore();

      unlisteners.push(
        await listen<{ sessionId: string; kind: string; text: string }>('acp://chunk', (e) =>
          this.onChunk(e.payload),
        ),
        await listen<{ sessionId: string; tool: Record<string, unknown> }>('acp://tool', (e) =>
          this.onTool(e.payload),
        ),
        await listen<{ sessionId: string; status: string; stopReason: string }>(
          'acp://turn',
          (e) => this.onTurn(e.payload),
        ),
        // ---- terminal command runs (Composer `!` prefix, cmd.rs) ----
        // The backend owns the row's authoritative content; the frontend
        // only streams text into the live DOM node (R1, same as acp://chunk)
        // and keeps rowId → runId for the cancel button.
        await listen<{ sessionId: string; runId: string; rowId: string; text: string }>(
          'term://output',
          (e) => {
            if (e.payload.sessionId !== this.sessionId) return;
            const { rowId, runId, text } = e.payload;
            if (this.runsByRow[rowId] !== runId) {
              this.runsByRow = { ...this.runsByRow, [rowId]: runId };
            }
            const row = this.rows.find((r) => r.id === rowId);
            if (!row) return;
            if (!row.segments) row.segments = [];
            // In-place extension: no reactive trigger (R1); the bubble's DOM
            // node is fed directly below, off-window bubbles self-heal from
            // the segments on re-mount.
            row.segments.push({ kind: 'text', text } as ChatSegment);
            const t = this.termTarget;
            if (t && t.rowId === rowId && t.el.isConnected) {
              t.el.appendChild(document.createTextNode(text));
            }
          },
        ),
        await listen<{
          sessionId: string;
          runId: string;
          rowId: string;
          code: number | null;
          killed: boolean;
          truncated: boolean;
        }>('term://exit', (e) => {
          const { rowId, runId } = e.payload;
          if (this.runsByRow[rowId] === runId) {
            const rest = { ...this.runsByRow };
            delete rest[rowId];
            this.runsByRow = rest;
          }
          if (this.termTarget?.rowId === rowId) this.termTarget = null;
          // No scrollSeq bump: the final bubbleSet keeps the header+status
          // height, and a forced scroll would yank the user away from
          // reading the output they scrolled up to inspect.
        }),
        await listen<PermissionRequest>('acp://permission', (e) => {
          sessions.markPermPending(e.payload.sessionId, true);
          if (e.payload.sessionId === this.sessionId) this.permission = e.payload;
        }),
        await listen<{ sessionId: string }>('acp://permissionCleared', (e) => {
          sessions.markPermPending(e.payload.sessionId, false);
          if (e.payload.sessionId === this.sessionId) this.permission = null;
        }),
        await listen<{ sessionId: string; subagents: Subagent[] }>('acp://subagent', (e) => {
          if (e.payload.sessionId === this.sessionId) this.subagents = e.payload.subagents;
        }),
        await listen<{ sessionId: string; options: ConfigOption[] }>('acp://configOptions', (e) => {
          if (e.payload.sessionId === this.sessionId) this.configOptions = e.payload.options;
        }),
        await listen<{ sessionId: string; commands: SlashCommand[] }>('acp://commands', (e) => {
          this.commandsBySession = {
            ...this.commandsBySession,
            [e.payload.sessionId]: e.payload.commands,
          };
        }),
        await listen<{ sessionId: string; row: ChatMessage }>('chat://messageAppended', (e) => {
          if (e.payload.sessionId !== this.sessionId) return;
          this.rows = [...this.rows, markRaw(e.payload.row)];
        }),
        await listen<{ sessionId: string; row: ChatMessage }>('chat://bubbleSet', (e) => {
          if (e.payload.sessionId !== this.sessionId) return;
          const i = this.rows.findIndex((r) => r.id === e.payload.row.id);
          if (i < 0) return;
          const next = [...this.rows];
          next[i] = markRaw(e.payload.row);
          this.rows = next;
        }),
        await listen<ChatStatus & { sessionId: string }>('chat://status', (e) => {
          if (e.payload.sessionId !== this.sessionId) return;
          const { sessionId: _id, ...st } = e.payload;
          this.status = st;
          // drainQueue pops the head without telling us which — reconcile the
          // mirror against the authoritative length.
          if (st.queueLength < this.queueMirror.length) {
            this.queueMirror = this.queueMirror.slice(this.queueMirror.length - st.queueLength);
          }
          if (st.queueLength === 0) this.queueOpen = false;
        }),
        await listen<{
          sessionId: string;
          active: boolean;
          countdown: number;
          attempt: number;
          maxAttempts: number;
        }>('chat://retry', (e) => {
          if (e.payload.sessionId !== this.sessionId) return;
          const { sessionId: _id, ...r } = e.payload;
          this.retry = r;
        }),
        await listen<{ sessionId: string }>('chat://unread', (e) => {
          sessions.markUnread(e.payload.sessionId);
        }),
        await listen('store://sessions', () => {
          void sessions.refresh(this.projectDir);
          void this.refreshMeta();
        }),
        await listen('store://prefs', () => {
          void prefs.reload();
        }),
        // 用量回填完成（usage://backfilled）：当前会话的历史气泡可能刚被
        // 挂上回填用量，重新拉取一次消息模型即可显示 ↑↓ 统计。
        await listen('usage://backfilled', () => {
          if (this.sessionId) void this.loadMessages();
        }),
      );
    },

    // ---- streaming merge (mirrors sessions.rs append_text_segment) ----

    /** The assistant row that stream events append to. If no in-flight
     * assistant row exists (pending placeholder or reloaded 'streaming' row),
     * synthesize one instead of dropping the event. Like streamRow but
     * WITHOUT the busy gate: a chunk can land before the chat://status push
     * that flips busy. */
    ensureStreamRow(): ChatMessage {
      for (let i = this.rows.length - 1; i >= 0; i--) {
        const r = this.rows[i];
        // 'streaming' too: rows reloaded via loadMessages() carry the
        // persisted mid-stream status; treating them as foreign would synth
        // a 2nd bubble.
        if (r.role === 'assistant' && (r.status === 'pending' || r.status === 'streaming')) return r;
      }
      const row = markRaw({
        id: `synth-${Date.now()}-${this.rows.length}`,
        role: 'assistant',
        content: '',
        createdAt: Date.now(),
        provider: '',
        status: 'pending',
        thinking: '',
        toolCalls: [],
        segments: [],
        attachments: [],
      } as ChatMessage);
      this.rows = [...this.rows, row];
      return row;
    },

    onChunk(p: { sessionId: string; kind: string; text: string }): void {
      if (p.sessionId !== this.sessionId) return; // background: self-heal on open
      const last = this.ensureStreamRow();
      const kind = p.kind === 'thinking' ? 'thinking' : 'text';
      const segs = last.segments;
      const tail = segs[segs.length - 1];
      if (tail && tail.kind === kind) {
        // Pure extension: in-place mutation + direct DOM append, NO reactive
        // trigger (R1). Off-window bubbles skip the append and re-render the
        // full text when mounted again.
        tail.text = (tail.text ?? '') + p.text;
        const t = this.streamTarget;
        if (t && t.rowId === last.id && t.kind === kind && t.el.isConnected) {
          const node = t.el.lastChild;
          if (node && node.nodeType === Node.TEXT_NODE) {
            (node as Text).appendData(p.text);
          } else {
            t.el.appendChild(document.createTextNode(p.text));
          }
        }
      } else {
        // New segment = structural event: replace the row object so this
        // bubble re-renders ONCE and mounts the new segment node (same
        // mechanism as the old structural-change rebuild; the component
        // instance and its segOpen state survive via the row-id key).
        const segs2 = [...segs, { kind, text: p.text } as ChatSegment];
        const next = [...this.rows];
        next[next.length - 1] = markRaw({ ...last, segments: segs2 });
        this.rows = next;
      }
    },

    onTool(p: { sessionId: string; tool: Record<string, unknown> }): void {
      if (p.sessionId !== this.sessionId) return;
      const last = this.ensureStreamRow();
      const id = String(p.tool.toolCallId ?? '');
      const segs = [...last.segments];
      const i = id ? segs.findIndex((s) => s.kind === 'tool' && s.toolCallId === id) : -1;
      const seg = { ...p.tool, kind: 'tool' } as ChatSegment;
      if (i >= 0) segs[i] = seg;
      else segs.push(seg);
      const next = [...this.rows];
      next[next.length - 1] = markRaw({ ...last, segments: segs });
      this.rows = next;
    },

    onTurn(p: { sessionId: string; status: string; usage?: TurnUsage }): void {
      this.turnSeq += 1;
      if (p.sessionId !== this.sessionId) return;
      this.streamTarget = null;
      // The turn's row may not be the last one (a command row can sit after
      // it) — flip the in-flight assistant row wherever it is.
      for (let i = this.rows.length - 1; i >= 0; i--) {
        const r = this.rows[i];
        if (r.role !== 'assistant' || (r.status !== 'pending' && r.status !== 'streaming')) continue;
        // Status flip triggers the one-time markdown render of the bubble.
        // usage rides along on this structural replacement only (R1: no
        // per-chunk updates).
        const next = [...this.rows];
        next[i] = markRaw({ ...r, status: p.status, usage: p.usage ?? r.usage });
        this.rows = next;
        return;
      }
    },

    /** The streaming bubble registers its live text element after render. */
    registerStreamTarget(rowId: string, kind: string, el: HTMLElement | null): void {
      if (el) this.streamTarget = { rowId, kind, el };
      else if (this.streamTarget?.rowId === rowId) this.streamTarget = null;
    },

    // ---- terminal commands (`!` prefix, Rust cmd.rs) ----

    /** Run `cmd /c <command>` in the project dir; the backend appends a
     * kind=='command' row and streams term://output. Never touches the
     * agent context. Returns true only when the backend accepted. */
    async runCommand(command: string): Promise<boolean> {
      if (!this.sessionId) {
        this.status = { ...this.status, lastError: '没有打开的会话' };
        return false;
      }
      try {
        await cmd('run_command', {
          sessionId: this.sessionId,
          command,
          workDir: this.projectDir,
        });
        this.scrollSeq += 1;
        return true;
      } catch (e) {
        this.status = { ...this.status, lastError: String(e) };
        return false;
      }
    },

    async killRun(runId: string): Promise<void> {
      if (!runId) return;
      try {
        await cmd('kill_command', { runId });
      } catch (e) {
        console.warn('[chat] kill_command failed', e);
      }
    },

    /** The streaming command bubble registers its live output <pre>. */
    registerTermTarget(rowId: string, el: HTMLElement | null): void {
      if (el) this.termTarget = { rowId, el };
      else if (this.termTarget?.rowId === rowId) this.termTarget = null;
    },

    // ---- session lifecycle ----

    async refreshMeta(): Promise<void> {
      if (!this.sessionId || !isTauri) return;
      try {
        const m = await cmd<SessionMeta | null>('session_meta', { sessionId: this.sessionId });
        if (m) this.meta = m;
      } catch {
        /* session may be gone; rail refresh handles the listing */
      }
    },

    /** Bind/rebind the current session to a project directory (option B):
     * meta.projectDir/workDir updated Rust-side; recents touched; panels
     * refresh via workspaceRefreshSeq (git/files watch it). */
    async bindProject(dir: string): Promise<boolean> {
      if (!this.sessionId || !isTauri) return false;
      try {
        const ok = await cmd<boolean>('set_session_project', {
          sessionId: this.sessionId,
          projectDir: dir,
        });
        if (!ok) return false;
        await cmd('open_project', { dir }).catch(() => {});
        this.projectDir = dir;
        await this.refreshMeta();
        this.workspaceRefreshSeq++;
        const sessions = useSessionsStore();
        await sessions.refresh(dir);
        return true;
      } catch (e) {
        this.status.lastError = String(e);
        return false;
      }
    },

    /** Pull the full message model (open / switch / self-heal). */
    async loadMessages(): Promise<void> {
      if (!this.sessionId) {
        this.rows = [];
        return;
      }
      const rows = await cmd<ChatMessage[]>(
        'session_messages',
        { sessionId: this.sessionId },
        [],
      );
      this.streamTarget = null;
      // markRaw per row (R1): in-place streaming text extension must NOT
      // trigger reactive re-renders; structural events replace row objects.
      this.rows = rows.map((r) => markRaw(r));
    },

    /** Provisional status until the next chat://status push arrives. */
    syncStatusFromRuntime(): void {
      const sessions = useSessionsStore();
      const prefs = usePrefsStore();
      const rt = sessions.runtimeStates[this.sessionId];
      const suffix = MODE_SUFFIX[prefs.permissionMode] ?? '';
      const queue = rt?.queueLength ?? 0;
      this.status = {
        ...IDLE_STATUS,
        busy: rt?.busy ?? false,
        queueLength: queue,
        acpReady: rt?.acpRunning ?? false,
        imageSupported: rt?.imageSupported ?? false,
        statusText:
          (rt?.busy ? '生成中…' : '就绪') + suffix + (queue > 0 ? ` · 队列 ${queue}/10` : ''),
      };
      // Rebuild the mirror from the backend snapshot: the queue survives a
      // session switch, so its previews must too.
      this.queueMirror = [...(rt?.queue ?? [])];
      this.permission = null;
      this.subagents = [];
      this.configOptions = [];
      this.retry = { active: false, countdown: 0, attempt: 0, maxAttempts: 3 };
    },

    /** openSession (chat.md §5): switch the active pointer + rebind.
     * Returns false when the backend refused (enter-session guard shows
     * the 无法打开会话 banner). */
    async openSession(id: string): Promise<boolean> {
      if (id === this.sessionId) return true;
      // Switch-cost instrumentation (temporary): per-step + total timing.
      let t = performance.now();
      const t0 = t;
      const lap = (label: string): void => {
        const now = performance.now();
        console.info(`[chat] openSession ${label}: ${(now - t).toFixed(1)}ms`);
        t = now;
      };
      const sessions = useSessionsStore();
      try {
        await cmd('open_session', { sessionId: id });
      } catch (e) {
        console.warn('[chat] open_session failed', e);
        return false;
      }
      lap('open_session');
      this.sessionId = id;
      await this.refreshMeta();
      lap('refreshMeta');
      if (this.meta?.projectDir) this.projectDir = this.meta.projectDir;
      await sessions.refresh(this.projectDir);
      lap('sessions.refresh');
      this.syncStatusFromRuntime();
      await this.loadMessages();
      lap(`loadMessages(rows=${this.rows.length})`);
      await nextTick(); // rows assigned → measure the actual list re-render
      lap('render(nextTick)');
      // A permission requested while this session was in the background only
      // fired the live event for the then-active session; re-pull the stored
      // payload so the dialog reappears after switching here.
      this.permission = await cmd<PermissionRequest | null>(
        'pending_permission',
        { sessionId: id },
        null,
      );
      lap('pending_permission');
      // Same for the sub-agent/task list: re-pull the backend snapshot so the
      // panels survive switching back (syncStatusFromRuntime already cleared).
      this.subagents = (await cmd<Subagent[] | null>('get_subagents', { sessionId: id }, null)) ?? [];
      lap('get_subagents');
      // syncStatusFromRuntime cleared configOptions; a live runtime stays
      // silent until the next picker event, so ask it to re-push its cache
      // (keeps the 模型/思考 dropdowns visible after switching back).
      await cmd('resend_config_options', { sessionId: id }, null);
      lap('resend_config_options');
      console.info(`[chat] openSession total: ${(performance.now() - t0).toFixed(1)}ms`);
      this.scrollSeq += 1; // force scroll-to-end after a switch
      return true;
    },

    /** create_session for the current project and make it active. `groupId`
     * '' (default) lands the session in the 默认会话 group; a group id drops
     * it straight into that group. Returns false when the backend refused
     * (e.g. no default agent). */
    async newSession(groupId?: string): Promise<boolean> {
      const sessions = useSessionsStore();
      try {
        const id = await cmd<string>('create_session', {
          projectDir: this.projectDir,
          groupId: groupId ?? '',
        });
        this.sessionId = id;
        await this.refreshMeta();
        await sessions.refresh(this.projectDir);
        this.syncStatusFromRuntime();
        await this.loadMessages();
        this.scrollSeq += 1;
        return true;
      } catch (e) {
        this.status = { ...this.status, lastError: String(e) };
        return false;
      }
    },

    /** Move a session (and its whole sub-session tree) to a rail group;
     * groupId '' = the default group. The backend re-anchors the top-level
     * ancestor and emits store://sessions. */
    async moveSessionGroup(sessionId: string, groupId: string): Promise<boolean> {
      const sessions = useSessionsStore();
      try {
        const ok = await cmd<boolean>('move_session_group', { sessionId, groupId });
        if (ok) {
          if (sessionId === this.sessionId) await this.refreshMeta();
          await sessions.refresh(this.projectDir);
        }
        return ok;
      } catch (e) {
        this.status = { ...this.status, lastError: String(e) };
        return false;
      }
    },

    /** Entry from the folder browser: open a project + create its session. */
    async startProjectSession(dir: string): Promise<boolean> {
      this.projectDir = dir;
      return this.newSession();
    },

    /** Rail 删除会话: backend closes the runtime first, then deletes. */
    async deleteSession(id: string): Promise<void> {
      const sessions = useSessionsStore();
      try {
        await cmd('delete_session', { sessionId: id });
      } catch (e) {
        console.warn('[chat] delete_session failed', e);
        return;
      }
      if (id === this.sessionId) {
        // Land on the empty-session state (chat.md §5: 删当前会话则页面落到空会话态).
        this.sessionId = '';
        this.meta = null;
        this.rows = [];
        this.status = { ...IDLE_STATUS, statusText: '就绪' + (MODE_SUFFIX[usePrefsStore().permissionMode] ?? '') };
        this.subagents = [];
        this.configOptions = [];
        this.permission = null;
        this.queueMirror = [];
      }
      if (id in this.commandsBySession) {
        const rest = { ...this.commandsBySession };
        delete rest[id];
        this.commandsBySession = rest;
      }
      if (id in this.drafts) {
        const rest = { ...this.drafts };
        delete rest[id];
        this.drafts = rest;
      }
      await sessions.refresh(this.projectDir);
    },

    /** Fork the active session at `messageId`: the backend creates a new
     * session with all messages up to and including it; we switch to the
     * branch and prefill the composer with the clicked user message so the
     * user can edit before resending (no auto-send). */
    async branchFromMessage(messageId: string): Promise<void> {
      if (!this.sessionId) return;
      const sessions = useSessionsStore();
      if (this.sessionDepth() >= MAX_SUB_DEPTH) {
        this.status = { ...this.status, lastError: '子会话层级已达上限（3 级）' };
        return;
      }
      const clicked = this.rows.find((r) => r.id === messageId);
      const draft = clicked?.role === 'user' ? clicked.content : '';
      let meta: { id?: string } | null;
      try {
        meta = await cmd<{ id?: string } | null>('branch_session', {
          sessionId: this.sessionId,
          upToMessageId: messageId,
        });
      } catch (e) {
        this.status = { ...this.status, lastError: String(e) };
        return;
      }
      if (!meta?.id) return;
      const ok = await this.openSession(meta.id);
      if (ok && draft) sessions.pendingComposerText = draft;
    },

    // ---- sub-session ("基于选中文本提问") ----

    /** 子会话标题：选区压缩空白后截取前 24 个字符。 */
    summarizeSelection(sel: string): string {
      const t = sel.replace(/\s+/g, ' ').trim();
      const chars = [...t];
      return chars.length <= 24 ? t : chars.slice(0, 24).join('') + '…';
    },

    /** <selection>…</selection> 引用块，预填给子会话的输入框；输入框会
     * 解析成引用条（输入框上方）里的引用块 + 正文纯文本。 */
    quoteSelection(sel: string): string {
      const body =
        sel.length <= SELECTION_QUOTE_MAX
          ? sel
          : sel.slice(0, SELECTION_QUOTE_MAX) + '\n…（超出 8000 字，已截断）';
      return `<selection>\n${body}\n</selection>`;
    },

    /** 基于选中文本提问：fork 一个继承到该气泡为止上下文的子会话
     * （branch_session 写入 parentId/sourceMessageId），标题取选区摘要，
     * 切换过去并在输入框预填【引用选中内容】块，由用户补充问题（不自动发）。 */
    async askOnSelection(messageId: string, selection: string): Promise<boolean> {
      if (!this.sessionId) return false;
      const sessions = useSessionsStore();
      if (this.sessionDepth() >= MAX_SUB_DEPTH) {
        this.status = { ...this.status, lastError: '子会话层级已达上限（3 级）' };
        return false;
      }
      const title = this.summarizeSelection(selection);
      let meta: { id?: string } | null;
      try {
        meta = await cmd<{ id?: string } | null>('branch_session', {
          sessionId: this.sessionId,
          upToMessageId: messageId,
          title,
        });
      } catch (e) {
        this.status = { ...this.status, lastError: String(e) };
        return false;
      }
      if (!meta?.id) return false;
      const ok = await this.openSession(meta.id);
      if (ok) sessions.pendingComposerText = this.quoteSelection(selection);
      return ok;
    },

    /** 跳回父会话（当前会话是子会话时，聊天页「父会话 ▸」）。 */
    async jumpToParent(): Promise<void> {
      const pid = this.meta?.parentId;
      if (pid) await this.openSession(pid);
    },

    /** 当前会话的层级：顶层会话 = 1，每级子会话 +1（沿铁轨 parentId 链）。 */
    sessionDepth(): number {
      const sessions = useSessionsStore();
      let depth = 1;
      let cur = this.sessionId;
      const seen = new Set<string>();
      while (cur && !seen.has(cur)) {
        seen.add(cur);
        const row = sessions.rail.find((s) => s.sessionId === cur);
        if (!row?.parentId) break;
        depth += 1;
        cur = row.parentId;
      }
      return depth;
    },

    // ---- turn actions ----

    /**
     * sendUserMessage[WithAttachments]. Text must already be @-reference
     * expanded by the composer. Returns true only when the backend accepted
     * (sent or enqueued) — the composer clears its draft only then. The
     * backend's ack is authoritative for the queue mirror: 'enqueued' means
     * the runtime actually pushed the entry (no reliance on a possibly
     * stale status.busy).
     */
    async send(text: string, attachments: string[]): Promise<boolean> {
      if (!this.sessionId) {
        this.status = { ...this.status, lastError: '没有打开的会话' };
        return false;
      }
      try {
        const outcome = await cmd<'sent' | 'enqueued'>('send_prompt', {
          sessionId: this.sessionId,
          text,
          attachments,
        });
        if (outcome === 'enqueued') {
          // Mirror the backend snapshot's annotation (runtime.rs sync_snap).
          const label = attachments.length > 0 ? `${text.trim()} 📎${attachments.length}` : text.trim();
          this.queueMirror = [...this.queueMirror, label];
          this.queueOpen = true; // 发送后若队列非空自动展开
        }
        this.scrollSeq += 1;
        return true;
      } catch (e) {
        // User-visible failures (queue full / attachments while busy) also
        // arrive via chat://status lastError; keep the draft either way.
        this.status = { ...this.status, lastError: String(e) };
        return false;
      }
    },

    async cancel(): Promise<void> {
      if (!this.sessionId) return;
      try {
        await cmd('cancel', { sessionId: this.sessionId });
      } catch (e) {
        console.warn('[chat] cancel failed', e);
      }
    },

    /** ACP config option picker (kimi "thinking"/"model"); refreshed options
     * arrive via acp://configOptions. */
    async setConfigOption(configId: string, value: string): Promise<void> {
      if (!this.sessionId) return;
      try {
        await cmd('set_config_option', { sessionId: this.sessionId, configId, value });
      } catch (e) {
        console.warn('[chat] set_config_option failed', e);
      }
    },

    async retryCancel(): Promise<void> {
      if (!this.sessionId) return;
      try {
        await cmd('retry_cancel', { sessionId: this.sessionId });
      } catch (e) {
        console.warn('[chat] retry_cancel failed', e);
      }
    },

    async answerPermission(optionId: string, cancelled: boolean): Promise<void> {
      if (!this.sessionId) return;
      this.permission = null;
      try {
        await cmd('answer_permission', { sessionId: this.sessionId, optionId, cancelled });
      } catch (e) {
        console.warn('[chat] answer_permission failed', e);
      }
    },

    async guideAt(index: number): Promise<void> {
      this.queueMirror = this.queueMirror.filter((_, i) => i !== index);
      try {
        await cmd('guide_at', { sessionId: this.sessionId, index });
      } catch (e) {
        console.warn('[chat] guide_at failed', e);
      }
    },

    async removeQueueAt(index: number): Promise<void> {
      this.queueMirror = this.queueMirror.filter((_, i) => i !== index);
      try {
        await cmd('remove_queue_at', { sessionId: this.sessionId, index });
      } catch (e) {
        console.warn('[chat] remove_queue_at failed', e);
      }
    },

    async clearQueue(): Promise<void> {
      this.queueMirror = [];
      this.queueOpen = false;
      try {
        await cmd('clear_queue', { sessionId: this.sessionId });
      } catch (e) {
        console.warn('[chat] clear_queue failed', e);
      }
    },

    async switchAgent(agentId: string): Promise<void> {
      if (!this.sessionId) return;
      try {
        await cmd('switch_agent', { sessionId: this.sessionId, agentId });
      } catch (e) {
        this.status = { ...this.status, lastError: String(e) };
      }
    },

    openPreview(path: string, line = 0): void {
      this.previewPath = path;
      this.previewLine = line;
    },

    closePreview(): void {
      this.previewPath = '';
      this.previewLine = 0;
    },

    /** Queue an @reference token for the composer (§3.3 syntax, e.g.
     * @src/App.java:12-30; from/to ≤ 0 → whole file @src/App.java). The
     * preview dialog context menu calls this with the project-relative path
     * and the selected line range. */
    insertRef(path: string, from: number, to: number): void {
      const token =
        from <= 0 && to <= 0
          ? `@${path}`
          : from === to
            ? `@${path}:${from}`
            : `@${path}:${from}-${to}`;
      this.pendingRefInsert = { seq: this.pendingRefInsert.seq + 1, token };
    },

    /** Attachment bar rules (§3.5): ≤6, deduped case-insensitively. */
    addAttachments(paths: string[]): void {
      for (const p of paths) {
        if (!p) continue;
        const norm = p.replace(/\//g, '\\');
        if (this.attachments.length >= 6) break;
        if (this.attachments.some((a) => a.toLowerCase() === norm.toLowerCase())) continue;
        this.attachments = [...this.attachments, norm];
      }
    },

    removeAttachment(i: number): void {
      this.attachments = this.attachments.filter((_, x) => x !== i);
    },

    clearAttachments(): void {
      this.attachments = [];
    },

    addComposerQuote(q: string): void {
      if (!q) return;
      this.composerQuotes = [...this.composerQuotes, q];
    },

    removeComposerQuote(i: number): void {
      this.composerQuotes = this.composerQuotes.filter((_, x) => x !== i);
    },

    clearComposerQuotes(): void {
      this.composerQuotes = [];
    },

    /** Save/restore hook for the composer (per-session draft, §3): empty
     * drafts are dropped so the map only holds real content. */
    saveDraft(
      sessionId: string,
      text: string,
      attachments: string[],
      quotes: string[] = [],
    ): void {
      if (!sessionId) return;
      const rest = { ...this.drafts };
      if (!text && attachments.length === 0 && quotes.length === 0) delete rest[sessionId];
      else rest[sessionId] = { text, quotes: [...quotes], attachments: [...attachments] };
      this.drafts = rest;
    },
  },
});

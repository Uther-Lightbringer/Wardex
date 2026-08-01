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
import { markRaw } from 'vue';
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
  updatedAt: number;
  messageCount: number;
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
    /** Collapse/expand-all command for thinking/tool blocks (一.10): bubbles
     * keep segOpen component-local; they watch segCollapseSeq and apply
     * segCollapseOpen to every thinking/tool segment. */
    segCollapseSeq: 0,
    segCollapseOpen: false,
    /** Bumped by the 刷新工作区 action-bay button (files/git panels). */
    workspaceRefreshSeq: 0,
    /** Pending attachment paths (attachment bar floats above the composer). */
    attachments: [] as string[],
    /** Live DOM node of the streaming segment (R1 incremental append). */
    streamTarget: null as StreamTarget | null,
    /** File preview dialog state (FilesPanel opens, dialog renders). */
    previewPath: '',
  }),
  getters: {
    /** Slash commands of the active session (composer `/` completion). */
    commands(): SlashCommand[] {
      return this.commandsBySession[this.sessionId] ?? [];
    },
    /** The row currently receiving stream chunks (last assistant, busy). */
    streamRow(): ChatMessage | null {
      if (!this.status.busy && !this.status.retryActive) return null;
      const last = this.rows[this.rows.length - 1];
      return last && last.role === 'assistant' ? last : null;
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
      );
    },

    // ---- streaming merge (mirrors sessions.rs append_text_segment) ----

    /** The assistant row that stream events append to. If the last row is not
     * a pending assistant placeholder (chat://messageAppended not emitted or
     * already flipped), synthesize one instead of dropping the event. */
    ensureStreamRow(): ChatMessage {
      const last = this.rows[this.rows.length - 1];
      if (last && last.role === 'assistant' && last.status === 'pending') return last;
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
      const last = this.rows[this.rows.length - 1];
      if (last && last.role === 'assistant') {
        // Status flip triggers the one-time markdown render of the bubble.
        // usage rides along on this structural replacement only (R1: no
        // per-chunk updates).
        const next = [...this.rows];
        next[next.length - 1] = markRaw({ ...last, status: p.status, usage: p.usage ?? last.usage });
        this.rows = next;
      }
    },

    /** The streaming bubble registers its live text element after render. */
    registerStreamTarget(rowId: string, kind: string, el: HTMLElement | null): void {
      if (el) this.streamTarget = { rowId, kind, el };
      else if (this.streamTarget?.rowId === rowId) this.streamTarget = null;
    },

    /** 一.10: collapse/expand every thinking/tool block in all bubbles. */
    setAllSegsOpen(open: boolean): void {
      this.segCollapseOpen = open;
      this.segCollapseSeq += 1;
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
      const sessions = useSessionsStore();
      try {
        await cmd('open_session', { sessionId: id });
      } catch (e) {
        console.warn('[chat] open_session failed', e);
        return false;
      }
      this.sessionId = id;
      await this.refreshMeta();
      if (this.meta?.projectDir) this.projectDir = this.meta.projectDir;
      await sessions.refresh(this.projectDir);
      this.syncStatusFromRuntime();
      await this.loadMessages();
      // A permission requested while this session was in the background only
      // fired the live event for the then-active session; re-pull the stored
      // payload so the dialog reappears after switching here.
      this.permission = await cmd<PermissionRequest | null>(
        'pending_permission',
        { sessionId: id },
        null,
      );
      this.scrollSeq += 1; // force scroll-to-end after a switch
      return true;
    },

    /** create_session for the current project and make it active.
     * Returns false when the backend refused (e.g. no default agent). */
    async newSession(): Promise<boolean> {
      const sessions = useSessionsStore();
      try {
        const id = await cmd<string>('create_session', { projectDir: this.projectDir });
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
      await sessions.refresh(this.projectDir);
    },

    /** Fork the active session at `messageId`: the backend creates a new
     * session with all messages up to and including it; we switch to the
     * branch and prefill the composer with the clicked user message so the
     * user can edit before resending (no auto-send). */
    async branchFromMessage(messageId: string): Promise<void> {
      if (!this.sessionId) return;
      const sessions = useSessionsStore();
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

    /** Endpoint (non-picker) model switch: backend persists it onto the
     * agent and respawns the CLI with the KIMI_MODEL_* env injection. */
    async setSessionModel(model: string): Promise<void> {
      if (!this.sessionId) return;
      try {
        await cmd('set_session_model', { sessionId: this.sessionId, model });
      } catch (e) {
        this.status = { ...this.status, lastError: String(e) };
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

    openPreview(path: string): void {
      this.previewPath = path;
    },

    closePreview(): void {
      this.previewPath = '';
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
  },
});

// Session rail data: per-project session list, agent list, unread flags,
// per-session runtime snapshots and permission-pending flags. Render state
// only — the authoritative data lives in the Rust stores; every mutation
// goes through a Tauri command and `store://sessions` triggers a re-pull.
//
// This store never imports the chat store (one-way dependency: chat →
// sessions) to keep the module graph acyclic.

import { defineStore } from 'pinia';
import { cmd, isTauri } from '../lib/tauri';
import { copyText } from '../lib/clipboard';

export interface RailSession {
  sessionId: string;
  title: string;
  updatedAt: number;
  messageCount: number;
  pinned: boolean;
}

/** One row of the sessions index (list_sessions; SessionIndexRow in Rust). */
export interface SessionIndexRow {
  id: string;
  title: string;
  agentName: string;
  provider: string;
  updatedAt: number;
  createdAt: number;
  messageCount: number;
  status: string;
  summary: string;
  projectDir: string;
  pinned: boolean;
}

/** Full-text search hit (SearchHit in Rust; ≤3 per session, ≤50 total). */
export interface SearchHit {
  sessionId: string;
  sessionTitle: string;
  projectDir: string;
  snippet: string;
  timestamp: number;
  updatedAt: number;
  hitCount: number;
  titleOnly: boolean;
}

export interface AgentInfo {
  id: string;
  name: string;
  provider: string;
  enabled: boolean;
  avatarPath: string;
  isDefault: boolean;
}

export interface RuntimeState {
  busy: boolean;
  acpRunning: boolean;
  queueLength: number;
  agentId: string;
  imageSupported: boolean;
  lastActivity: number;
}

/** One messages.jsonl row as returned by session_messages (camelCase). */
export interface StoredMessage {
  id: string;
  role: string;
  content: string;
  createdAt: number;
  provider: string;
  status: string;
  thinking: string;
  toolCalls: unknown[];
  segments: { kind?: string; text?: string }[];
  attachments: string[];
}

/** Visible text of a stored row: text segments joined, content as fallback. */
export function visibleText(row: StoredMessage): string {
  if (row.segments && row.segments.length > 0) {
    const t = row.segments
      .filter((s) => s.kind === 'text')
      .map((s) => s.text ?? '')
      .join('');
    if (t.trim().length > 0) return t;
  }
  return row.content === '…' ? '' : row.content;
}

export const useSessionsStore = defineStore('sessions', {
  state: () => ({
    rail: [] as RailSession[],
    agents: [] as AgentInfo[],
    defaultAgentId: '',
    unreadIds: [] as string[],
    runtimeStates: {} as Record<string, RuntimeState>,
    /** Sessions with an unanswered permission request (rail gold dot). */
    permPending: [] as string[],
    /** One-shot composer prefill ("基于此提问"); consumed by the composer. */
    pendingComposerText: '',
    /** Full sessions index for the session-select page (updatedAt desc). */
    all: [] as SessionIndexRow[],
    /** Frontend full-text search generation: stale responses are dropped
     * (double insurance — the Rust engine also supersedes in-flight scans). */
    searchGeneration: 0,
  }),
  getters: {
    agentById(): (id: string) => AgentInfo | undefined {
      return (id: string) => this.agents.find((a) => a.id === id);
    },
    /** Rail dot state: waiting (perm pending) > running (busy) > idle. */
    dotState(): (id: string) => 'running' | 'waiting' | 'idle' {
      return (id: string) => {
        if (this.permPending.includes(id)) return 'waiting';
        if (this.runtimeStates[id]?.busy) return 'running';
        return 'idle';
      };
    },
  },
  actions: {
    async refresh(projectDir: string): Promise<void> {
      if (!isTauri) return;
      try {
        const [rail, states, unread] = await Promise.all([
          cmd<RailSession[]>('sessions_for_project', { projectDir }, []),
          cmd<Record<string, RuntimeState>>('runtime_states', undefined, {}),
          cmd<string[]>('unread_sessions', undefined, []),
        ]);
        // Pinned first (stable within each group by updatedAt desc — the
        // backend list is already updatedAt-desc).
        this.rail = [...rail].sort((a, b) => Number(b.pinned) - Number(a.pinned));
        this.runtimeStates = states;
        this.unreadIds = unread;
      } catch (e) {
        console.warn('[sessions] refresh failed', e);
      }
    },

    async refreshAgents(): Promise<void> {
      if (!isTauri) return;
      try {
        const v = await cmd<{ agents?: AgentInfo[]; defaultAgentId?: string }>('list_agents');
        this.agents = v.agents ?? [];
        this.defaultAgentId = v.defaultAgentId ?? '';
      } catch (e) {
        console.warn('[sessions] list_agents failed', e);
      }
    },

    /** reloadSessions (session-select page enter): re-pull the whole index
     * plus the unread set. Grouping/sorting happens in the page. */
    async reloadAll(): Promise<void> {
      if (!isTauri) return;
      try {
        const [all, unread] = await Promise.all([
          cmd<SessionIndexRow[]>('list_sessions', undefined, []),
          cmd<string[]>('unread_sessions', undefined, []),
        ]);
        this.all = all;
        this.unreadIds = unread;
      } catch (e) {
        console.warn('[sessions] reloadAll failed', e);
      }
    },

    /**
     * searchMessages (500ms debounced by the caller). Empty query cancels
     * any in-flight scan and resolves to []. Returns null when a newer
     * query superseded this one — the caller must NOT touch its results
     * state in that case (generation double-check, spec §3.2).
     */
    async searchMessages(query: string): Promise<SearchHit[] | null> {
      const gen = ++this.searchGeneration;
      if (!isTauri) return [];
      let hits: SearchHit[] = [];
      try {
        hits = await cmd<SearchHit[]>('search_messages', { query }, []);
      } catch (e) {
        console.warn('[sessions] search_messages failed', e);
      }
      if (gen !== this.searchGeneration) return null; // superseded
      return hits;
    },

    async rename(id: string, title: string): Promise<void> {
      await cmd('rename_session', { sessionId: id, title });
    },

    async setPinned(id: string, pinned: boolean): Promise<void> {
      await cmd('set_session_pinned', { sessionId: id, pinned });
    },

    markPermPending(id: string, pending: boolean): void {
      const has = this.permPending.includes(id);
      if (pending && !has) this.permPending = [...this.permPending, id];
      if (!pending && has) this.permPending = this.permPending.filter((s) => s !== id);
    },

    markUnread(id: string): void {
      if (!this.unreadIds.includes(id)) this.unreadIds = [...this.unreadIds, id];
    },

    /**
     * 复制会话内容 (chat.md §7.5): "User: …" / "Assistant: …" lines, pending
     * assistant rows skipped, no thinking/tool blocks. session_messages
     * ensures the model is resident (LRU ≤5), so unopened sessions parse
     * straight from JSONL on the Rust side.
     */
    async copyTranscript(id: string): Promise<string> {
      const rows = await cmd<StoredMessage[]>('session_messages', { sessionId: id }, []);
      const lines: string[] = [];
      for (const r of rows) {
        if (r.role === 'assistant' && r.status === 'pending') continue;
        const text = visibleText(r).trim();
        if (!text) continue;
        lines.push(`${r.role === 'user' ? 'User' : 'Assistant'}: ${text}`);
      }
      if (lines.length === 0) return '会话没有可复制的消息';
      const ok = await copyText(lines.join('\n\n'));
      return ok ? '' : '复制失败';
    },
  },
});

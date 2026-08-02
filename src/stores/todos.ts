// Unified todo/reminder board (todos.json). Every row has a scope:
//   session — belongs to a session; due → popup (or push into the session
//             when notifyMode == "push", MCP agent self-wakeup rows)
//   project — belongs to a project (projectDir); due → backend auto-creates
//             a new session and emits todos://projectDue (three-way dialog)
//   global  — app-level; due → popup notification
// Backed by Rust commands; listens to todos://changed for external writes
// (MCP subprocess, due tick, other windows).

import { defineStore } from 'pinia';
import { listen } from '@tauri-apps/api/event';
import { cmd, isTauri } from '../lib/tauri';

export type TodoScope = 'session' | 'project' | 'global';
export type NotifyMode = 'popup' | 'push';

export interface TodoRow {
  id: string;
  title: string;
  done: boolean;
  createdAt: number;
  doneAt: number;
  scope: TodoScope;
  sessionId: string;
  projectDir: string;
  dueAtMs: number;
  notifiedAtMs: number;
  notifyMode: NotifyMode;
}

export interface TodoGroups {
  session: TodoRow[];
  project: TodoRow[];
  global: TodoRow[];
  done: TodoRow[];
}

let listenerReady = false;
let fallbackSeq = 0;

export const useTodosStore = defineStore('todos', {
  state: () => ({
    groups: { session: [], project: [], global: [], done: [] } as TodoGroups,
    /** Context of the last load — used by refresh() (panel/page scope). */
    sessionId: '',
    projectDir: '',
    loaded: false,
  }),
  getters: {
    pending(): TodoRow[] {
      return [...this.groups.session, ...this.groups.project, ...this.groups.global];
    },
    done(): TodoRow[] {
      return this.groups.done;
    },
    /** Overdue pending rows (dueAtMs in the past) — shown 已到期. */
    overdue(): TodoRow[] {
      const now = Date.now();
      return this.pending.filter((r) => r.dueAtMs > 0 && r.dueAtMs <= now);
    },
  },
  actions: {
    /** One-time event wiring: external writes (MCP, due tick, commands)
     * re-pull the current context. Called from the panel/page on mount. */
    async init(): Promise<void> {
      if (listenerReady || !isTauri) return;
      listenerReady = true;
      await listen('todos://changed', () => void this.refresh());
    },
    async load(sessionId: string, projectDir: string): Promise<void> {
      this.sessionId = sessionId;
      this.projectDir = projectDir;
      this.loaded = true;
      if (!isTauri) return;
      try {
        const g = await cmd<TodoGroups>('todos_list', { sessionId, projectDir });
        this.groups = g ?? { session: [], project: [], global: [], done: [] };
      } catch (e) {
        console.warn('[todos] todos_list failed', e);
      }
    },
    async refresh(): Promise<void> {
      await this.load(this.sessionId, this.projectDir);
    },
    async add(
      title: string,
      scope: TodoScope,
      sessionId: string,
      projectDir: string,
      dueAtMs: number,
      notifyMode: NotifyMode = 'popup',
    ): Promise<void> {
      const t = title.trim();
      if (!t) return;
      if (isTauri) {
        await cmd('todo_add', {
          title: t,
          scope,
          sessionId,
          projectDir,
          dueAtMs,
          notifyMode,
        }).catch((e) => console.warn('[todos] add', e));
        await this.refresh();
      } else {
        this.groups[scope] = [
          {
            id: `local-${++fallbackSeq}`,
            title: t,
            done: false,
            createdAt: Date.now(),
            doneAt: 0,
            scope,
            sessionId,
            projectDir,
            dueAtMs,
            notifiedAtMs: 0,
            notifyMode,
          },
          ...this.groups[scope],
        ];
      }
    },
    async toggle(id: string): Promise<void> {
      if (isTauri) {
        await cmd('todo_toggle', { id }).catch((e) => console.warn('[todos] toggle', e));
        await this.refresh();
      } else {
        for (const list of Object.values(this.groups)) {
          const r = list.find((x) => x.id === id);
          if (r) {
            r.done = !r.done;
            r.doneAt = r.done ? Date.now() : 0;
            break;
          }
        }
      }
    },
    async remove(id: string): Promise<void> {
      if (isTauri) {
        await cmd('todo_remove', { id }).catch((e) => console.warn('[todos] remove', e));
        await this.refresh();
      } else {
        for (const key of Object.keys(this.groups) as (keyof TodoGroups)[]) {
          this.groups[key] = this.groups[key].filter((x) => x.id !== id);
        }
      }
    },
    async clearDone(): Promise<void> {
      if (isTauri) {
        await cmd('todos_clear_done').catch((e) => console.warn('[todos] clearDone', e));
        await this.refresh();
      } else {
        this.groups.done = [];
      }
    },
  },
});

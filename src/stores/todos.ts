// Todo board (todos.json). App-level personal todos, fully independent of
// chat/session data. Backed by Rust commands with an in-memory-only
// fallback for plain-browser preview.

import { defineStore } from 'pinia';
import { cmd, isTauri } from '../lib/tauri';

export interface TodoRow {
  id: string;
  title: string;
  done: boolean;
  createdAt: number;
  doneAt: number;
}

let fallbackSeq = 0;

export const useTodosStore = defineStore('todos', {
  state: () => ({
    rows: [] as TodoRow[],
    loaded: false,
  }),
  getters: {
    pending(): TodoRow[] {
      return this.rows.filter((r) => !r.done);
    },
    done(): TodoRow[] {
      return this.rows.filter((r) => r.done);
    },
  },
  actions: {
    async load(): Promise<void> {
      if (this.loaded) return;
      this.loaded = true;
      if (!isTauri) return;
      try {
        this.rows = (await cmd<TodoRow[]>('todos_list')) ?? [];
      } catch (e) {
        console.warn('[todos] todos_list failed', e);
      }
    },
    async refresh(): Promise<void> {
      this.loaded = false;
      await this.load();
    },
    async add(title: string): Promise<void> {
      const t = title.trim();
      if (!t) return;
      if (isTauri) {
        await cmd('todo_add', { title: t }).catch((e) => console.warn('[todos] add', e));
        await this.refresh();
      } else {
        this.rows.push({
          id: `local-${++fallbackSeq}`,
          title: t,
          done: false,
          createdAt: Date.now(),
          doneAt: 0,
        });
      }
    },
    async toggle(id: string): Promise<void> {
      if (isTauri) {
        await cmd('todo_toggle', { id }).catch((e) => console.warn('[todos] toggle', e));
        await this.refresh();
      } else {
        const r = this.rows.find((x) => x.id === id);
        if (r) {
          r.done = !r.done;
          r.doneAt = r.done ? Date.now() : 0;
        }
      }
    },
    async remove(id: string): Promise<void> {
      if (isTauri) {
        await cmd('todo_remove', { id }).catch((e) => console.warn('[todos] remove', e));
        await this.refresh();
      } else {
        this.rows = this.rows.filter((x) => x.id !== id);
      }
    },
    async clearDone(): Promise<void> {
      if (isTauri) {
        await cmd('todos_clear_done').catch((e) => console.warn('[todos] clearDone', e));
        await this.refresh();
      } else {
        this.rows = this.rows.filter((x) => !x.done);
      }
    },
  },
});

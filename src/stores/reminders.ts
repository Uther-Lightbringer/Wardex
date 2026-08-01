// Session reminders. Agent (or the user) can schedule a reminder for the
// ACTIVE session; the Rust side owns the timer and fires a chat message when
// due. This store mirrors the current session's list and syncs it via the
// chat://reminders event; session switch re-pulls via reminders_list.

import { defineStore } from 'pinia';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { cmd, isTauri } from '../lib/tauri';

export interface Reminder {
  id: string;
  sessionId: string;
  content: string;
  dueAtMs: number;
  createdAtMs: number;
  source: 'agent' | 'user';
  done: boolean;
}

let fallbackSeq = 0;
let listenerReady = false;
const unlisteners: UnlistenFn[] = [];

export const useRemindersStore = defineStore('reminders', {
  state: () => ({
    /** The session this mirror belongs to ('' = none). */
    sessionId: '',
    rows: [] as Reminder[],
    loaded: false,
  }),
  getters: {
    pending(): Reminder[] {
      return this.rows.filter((r) => !r.done);
    },
    done(): Reminder[] {
      return this.rows.filter((r) => r.done);
    },
  },
  actions: {
    /** One-time event wiring (called from RemindersPanel onMounted). */
    async init(): Promise<void> {
      if (listenerReady || !isTauri) return;
      listenerReady = true;
      unlisteners.push(
        await listen<{ sessionId: string; reminders: Reminder[] }>('chat://reminders', (e) => {
          if (e.payload.sessionId === this.sessionId) this.rows = e.payload.reminders;
        }),
      );
    },
    /** (Re)bind the mirror to a session — called on mount and session switch. */
    async load(sessionId: string): Promise<void> {
      this.sessionId = sessionId;
      this.loaded = true;
      if (!sessionId || !isTauri) {
        this.rows = [];
        return;
      }
      try {
        this.rows = (await cmd<Reminder[]>('reminders_list', { sessionId })) ?? [];
      } catch (e) {
        console.warn('[reminders] reminders_list failed', e);
      }
    },
    async refresh(): Promise<void> {
      await this.load(this.sessionId);
    },
    async add(content: string, minutes: number): Promise<void> {
      const c = content.trim();
      if (!c || !this.sessionId || minutes <= 0) return;
      if (isTauri) {
        await cmd('reminder_add', { sessionId: this.sessionId, content: c, minutes }).catch((e) =>
          console.warn('[reminders] add', e),
        );
        await this.refresh();
      } else {
        const now = Date.now();
        this.rows.push({
          id: `local-${++fallbackSeq}`,
          sessionId: this.sessionId,
          content: c,
          dueAtMs: now + minutes * 60_000,
          createdAtMs: now,
          source: 'user',
          done: false,
        });
      }
    },
    async cancel(id: string): Promise<void> {
      if (isTauri) {
        await cmd('reminder_cancel', { id }).catch((e) => console.warn('[reminders] cancel', e));
        await this.refresh();
      } else {
        this.rows = this.rows.filter((x) => x.id !== id);
      }
    },
  },
});

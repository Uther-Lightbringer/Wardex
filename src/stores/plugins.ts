// Plugin registry store (插件化改造 P0/P2): mirrors the Rust-side scan of
// <data root>/wardex-plugins/ + builtins. The settings page manages toggles;
// WarDock derives extra drawer tabs from UI plugins (kind containing "ui").
// 「生效」(apply) restarts idle pi runtimes so new extension files load; the
// frontend then re-pulls this list so new UI panels appear/disappear.

import { defineStore } from 'pinia';
import { cmd } from '../lib/tauri';

export interface PluginInfo {
  id: string;
  name: string;
  version: string;
  /** 'tool' | 'ui' | 'tool+ui' */
  kind: string;
  enabled: boolean;
  builtin: boolean;
  entry: string;
  ui: string;
  dir: string;
  /** 'drawer' | 'dialog' | 'both' */
  surface: string;
}

export interface ApplyResult {
  restarted: number;
  skipped: number;
}

export const usePluginsStore = defineStore('plugins', {
  state: () => ({
    list: [] as PluginInfo[],
    loaded: false,
    applying: false,
    lastApply: null as ApplyResult | null,
    /** Plugin files changed since the last 生效 (polled). */
    pending: false,
    /** User approved a model-initiated apply while the turn was still busy —
     * flushed on the next turn end (restarting pi mid-turn would kill it). */
    deferredApply: false,
  }),
  getters: {
    /** Enabled UI plugins → WarDock drawer tabs (surface includes drawer). */
    uiPanels(state): PluginInfo[] {
      return state.list.filter(
        (p) => p.enabled && p.ui && p.kind.includes('ui') && p.surface !== 'dialog',
      );
    },
    /** Enabled UI plugins rendered ONLY as floating dialogs (rail button
     *  opens the window directly). 'both' keeps its drawer tab and can be
     *  promoted over the bridge at runtime. */
    dialogPanels(state): PluginInfo[] {
      return state.list.filter((p) => p.enabled && p.ui && p.kind.includes('ui') && p.surface === 'dialog');
    },
  },
  actions: {
    async load(): Promise<void> {
      try {
        this.list = await cmd<PluginInfo[]>('plugins_list', undefined, []);
      } catch {
        this.list = [];
      }
      this.loaded = true;
      this.startPendingPoll();
    },
    /** Cheap poll for "有未生效的变更" hint (model edits happen outside the
     * UI's knowledge — polling is the only way to notice them). */
    startPendingPoll(): void {
      if (pollTimer !== null) return;
      pollTimer = setInterval(() => {
        void cmd<boolean>('plugins_pending', undefined, false).then((v) => {
          this.pending = v;
        });
      }, 4000);
    },
    /** Model-initiated apply (plugin_apply tool approved). If a turn is in
     * flight, defer — restarting pi mid-turn would kill the response. */
    requestApplyAfterTurn(): void {
      void import('./chat').then(({ useChatStore }) => {
        const chat = useChatStore();
        if (!chat.sessionId || !chat.status.busy) {
          void this.apply();
        } else {
          this.deferredApply = true;
        }
      });
    },
    /** Called from chat.onTurn on every turn event; applies once idle. */
    flushDeferred(): void {
      if (!this.deferredApply) return;
      this.deferredApply = false;
      void this.apply();
    },
    async rescan(): Promise<void> {
      this.list = await cmd<PluginInfo[]>('plugins_rescan', undefined, []);
    },
    async toggle(id: string, enabled: boolean): Promise<void> {
      // Optimistic flip; reload on failure to resync.
      const p = this.list.find((x) => x.id === id);
      if (p) p.enabled = enabled;
      try {
        await cmd('plugins_toggle', { id, enabled });
      } catch {
        await this.load();
      }
    },
    async remove(id: string): Promise<void> {
      await cmd('plugins_delete', { id });
      await this.rescan();
    },
    async rootDir(): Promise<string> {
      return cmd<string>('plugins_root_dir', undefined, '');
    },
    async apply(): Promise<ApplyResult> {
      this.applying = true;
      try {
        const r = await cmd<ApplyResult>('plugins_apply', undefined, { restarted: 0, skipped: 0 });
        this.lastApply = r;
        this.pending = false;
        await this.load();
        return r;
      } finally {
        this.applying = false;
      }
    },
  },
});

let pollTimer: ReturnType<typeof setInterval> | null = null;

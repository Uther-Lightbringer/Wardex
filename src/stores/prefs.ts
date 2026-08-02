// User preferences: fontScale (0.85~1.30) + panel dock layout memory.
// Backed by user_prefs.json via Rust commands; localStorage fallback keeps
// the skeleton usable in a plain browser (vite dev without Tauri).

import { defineStore } from 'pinia';
import { cmd, isTauri } from '../lib/tauri';

// Drawer-dock layout memory: per-panel `order` (排序). The drawer WIDTH is
// shared across all panels as `panelWidth` (dragged once, applies to every
// tab). Open state is transient (never persisted). Legacy entries may carry
// stale `open` / `height` / `width` keys — `width` is only migrated by the
// backend into the shared panelWidth on first load, the rest are ignored.
export interface PanelLayoutEntry {
  width?: number;
  order?: number;
}

export const FONT_SCALE_MIN = 0.85;
export const FONT_SCALE_MAX = 1.3;
/** Fixed dropdown steps (ConfigPage.qml:529) */
export const FONT_SCALE_STEPS = [0.85, 1.0, 1.15, 1.3];

const LS_FONT_SCALE = 'wardex.fontScale';
const LS_PANEL_LAYOUT = 'wardex.panelLayout';

function clampScale(s: number): number {
  if (!Number.isFinite(s)) return 1.0;
  return Math.min(FONT_SCALE_MAX, Math.max(FONT_SCALE_MIN, s));
}

export const usePrefsStore = defineStore('prefs', {
  state: () => ({
    fontScale: 1.0,
    panelLayout: {} as Record<string, PanelLayoutEntry>,
    userName: '阿尔萨斯',
    /** WarDex mode id: default | plan | auto | yolo (global, per user_prefs) */
    permissionMode: 'default',
    /** File-preview dialog size memory; 0 = not dragged yet (A4 default). */
    previewWidth: 0,
    previewHeight: 0,
    /** Chat-page left rail column width (px); draggable handle in the rail. */
    railWidth: 240,
    /** Shared right-dock drawer width (px) — one width for ALL dock tabs. */
    panelWidth: 240,
    userAvatarPath: '',
    loaded: false,
  }),
  getters: {
    /** fs(n): every reading-type font size goes through this (docs §1.4) */
    fs(): (n: number) => number {
      return (n: number) => Math.round(n * this.fontScale);
    },
  },
  actions: {
    async load(): Promise<void> {
      if (this.loaded) return;
      this.loaded = true;
      if (isTauri) {
        try {
          const p = await cmd<{
            fontScale?: number;
            panelLayout?: Record<string, PanelLayoutEntry>;
            userName?: string;
            permissionMode?: string;
            previewWidth?: number;
            previewHeight?: number;
            railWidth?: number;
            userAvatarPath?: string;
            panelWidth?: number;
          }>('get_prefs');
          this.fontScale = clampScale(p.fontScale ?? 1.0);
          this.panelLayout = p.panelLayout ?? {};
          this.userName = p.userName || '阿尔萨斯';
          this.permissionMode = p.permissionMode || 'default';
          this.previewWidth = p.previewWidth ?? 0;
          this.previewHeight = p.previewHeight ?? 0;
          this.railWidth = p.railWidth ?? 240;
          this.panelWidth = p.panelWidth ?? 240;
          this.userAvatarPath = p.userAvatarPath ?? '';
          return;
        } catch (e) {
          console.warn('[prefs] get_prefs failed', e);
        }
      }
      // Browser fallback
      try {
        const s = Number(localStorage.getItem(LS_FONT_SCALE));
        if (s) this.fontScale = clampScale(s);
        const pl = localStorage.getItem(LS_PANEL_LAYOUT);
        if (pl) this.panelLayout = JSON.parse(pl);
      } catch {
        /* ignore */
      }
    },

    /** store://prefs arrived (permission mode / config page edits): re-pull. */
    async reload(): Promise<void> {
      this.loaded = false;
      await this.load();
    },

    /** 我的名字 (§8.2): trimmed ≤24 chars backend-side; empty falls back to
     * 阿尔萨斯 at display time. */
    async setUserName(name: string): Promise<void> {
      const n = name.trim().slice(0, 24);
      this.userName = n || '阿尔萨斯';
      if (isTauri) {
        try {
          await cmd('set_user_name', { name: n });
        } catch (e) {
          console.warn('[prefs] set_user_name failed', e);
        }
      }
    },

    /** 上传我的头像: import (backend copies + center-crops to 128×128 PNG).
     * Returns false on import failure (UI shows 头像导入失败). */
    async importUserAvatar(localPath: string): Promise<boolean> {
      if (!isTauri) return false;
      try {
        const ok = await cmd<boolean>('set_user_avatar_from_file', { localPath });
        if (ok) await this.reload();
        return ok;
      } catch (e) {
        console.warn('[prefs] set_user_avatar_from_file failed', e);
        return false;
      }
    },

    /** 恢复默认头像: drop the custom file, back to the built-in portrait. */
    async clearUserAvatar(): Promise<void> {
      if (isTauri) {
        try {
          await cmd('clear_user_avatar');
        } catch (e) {
          console.warn('[prefs] clear_user_avatar failed', e);
        }
      }
      this.userAvatarPath = '';
    },

    async setPermissionMode(mode: string): Promise<void> {      this.permissionMode = mode;
      if (isTauri) {
        try {
          await cmd('set_permission_mode', { mode });
        } catch (e) {
          console.warn('[prefs] set_permission_mode failed', e);
        }
      }
    },

    async setPreviewSize(width: number, height: number): Promise<void> {
      this.previewWidth = Math.round(width);
      this.previewHeight = Math.round(height);
      if (isTauri) {
        try {
          await cmd('set_preview_size', { width: this.previewWidth, height: this.previewHeight });
        } catch (e) {
          console.warn('[prefs] set_preview_size failed', e);
        }
      }
    },

    /** 铁轨宽度：拖拽中只改本地 state（不落盘），松手后调用一次持久化。 */
    setRailWidthLocal(width: number): void {
      this.railWidth = Math.round(width);
    },

    async setRailWidth(width: number): Promise<void> {
      this.railWidth = Math.round(width);
      if (isTauri) {
        try {
          await cmd('set_rail_width', { width: this.railWidth });
        } catch (e) {
          console.warn('[prefs] set_rail_width failed', e);
        }
      }
    },

    /** 抽屉面板宽度：拖拽中只改本地 state（不落盘），松手后调用一次持久化。 */
    setPanelWidthLocal(width: number): void {
      this.panelWidth = Math.round(width);
    },

    async setPanelWidth(width: number): Promise<void> {
      this.panelWidth = Math.round(width);
      if (isTauri) {
        try {
          await cmd('set_panel_width', { width: this.panelWidth });
        } catch (e) {
          console.warn('[prefs] set_panel_width failed', e);
        }
      }
    },

    async setFontScale(scale: number): Promise<void> {
      this.fontScale = clampScale(scale);
      if (isTauri) {
        try {
          await cmd('set_font_scale', { scale: this.fontScale });
          return;
        } catch (e) {
          console.warn('[prefs] set_font_scale failed', e);
        }
      }
      localStorage.setItem(LS_FONT_SCALE, String(this.fontScale));
    },

    /** Merge a panel's layout entry locally (instant UI feedback). */
    setPanelLayoutLocal(id: string, entry: PanelLayoutEntry): void {
      this.panelLayout = {
        ...this.panelLayout,
        [id]: { ...this.panelLayout[id], ...entry },
      };
    },

    /** Persist one panel's entry (WarDock debounces the calls). */
    async persistPanelLayout(id: string): Promise<void> {
      const entry = this.panelLayout[id] ?? {};
      if (isTauri) {
        try {
          await cmd('set_panel_layout', { panelId: id, entry });
          return;
        } catch (e) {
          console.warn('[prefs] set_panel_layout failed', e);
        }
      }
      localStorage.setItem(LS_PANEL_LAYOUT, JSON.stringify(this.panelLayout));
    },
  },
});

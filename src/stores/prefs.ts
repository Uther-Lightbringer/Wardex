// User preferences: fontScale (0.85~1.30) + panel dock layout memory.
// Backed by user_prefs.json via Rust commands; localStorage fallback keeps
// the skeleton usable in a plain browser (vite dev without Tauri).

import { defineStore } from 'pinia';
import { cmd, isTauri } from '../lib/tauri';
import { DEFAULT_BG, loadBackground, type BgConfig } from '../lib/background';

// Drawer-dock layout memory: per-panel `order` (排序). The drawer WIDTH is
// shared across all panels as `panelWidth` (dragged once, applies to every
// tab). Open state is transient (never persisted). Legacy entries may carry
// stale `open` / `height` / `width` keys — `width` is only migrated by the
// backend into the shared panelWidth on first load, the rest are ignored.
export interface PanelLayoutEntry {
  width?: number;
  order?: number;
}

/** 战场监控页兵营落点：projectDir → 沙盘区比例坐标（0..1，窗口缩放安全）。 */
export interface MonitorLayoutEntry {
  x: number;
  y: number;
}

/** 监控页步兵（会话）持久化：sessionId → 沙盘比例坐标 + 优先级标记
 * （priority 1..4 = 区域在 monitorZones 里的序号，0/无 = 未标记）。 */
export interface MonitorFootmanEntry {
  rx?: number;
  ry?: number;
  priority?: number;
}

/** 监控页优先级区域（塔 = 圆心，区域 = 以塔为圆心的圆，可部署/移动/拆/改名/改色/调半径）。 */
export interface MonitorZone {
  id: string;
  name: string;
  color: string;
  /** 塔圆心世界层比例坐标（0..1），窗口缩放安全。 */
  x: number;
  y: number;
  /** 圆形区域半径（世界层比例，相对世界宽；乘 worldW 得像素，屏幕正圆）。 */
  r: number;
  enabled: boolean;
  /** 是否已部署到地图上（false = 未部署，在塔列表中等待部署）。 */
  deployed: boolean;
}

/** 默认塔布局：4 个塔按原艾森豪威尔四宫格中心摆，默认均未部署（需在塔列表中选择部署）。 */
export const DEFAULT_MONITOR_ZONES: MonitorZone[] = [
  { id: 'q1', name: '重要·紧急', color: '#e14b43', x: 0.27, y: 0.18, r: 0.09, enabled: true, deployed: false },
  { id: 'q2', name: '重要·不紧急', color: '#e8983d', x: 0.73, y: 0.18, r: 0.09, enabled: true, deployed: false },
  { id: 'q3', name: '不重要·紧急', color: '#d4b94a', x: 0.27, y: 0.46, r: 0.09, enabled: true, deployed: false },
  { id: 'q4', name: '不重要·不紧急', color: '#57a55f', x: 0.73, y: 0.46, r: 0.09, enabled: true, deployed: false },
];

export const FONT_SCALE_MIN = 0.85;
export const FONT_SCALE_MAX = 1.3;
/** Fixed dropdown steps (ConfigPage.qml:529) */
export const FONT_SCALE_STEPS = [0.85, 1.0, 1.15, 1.3];

/** 对话页透明度档位（纯净风格）：100% 不透明 → 50% 半透明。 */
export const CHAT_ALPHA_STEPS = [1.0, 0.9, 0.8, 0.7, 0.6, 0.5];
export function clampChatAlpha(v: number): number {
  if (!Number.isFinite(v)) return 0.8;
  return Math.min(1.0, Math.max(0.5, v));
}

/** 背景亮度档位（纯净风格，作用于背景图/视频）：50% 最暗 → 150% 最亮。 */
export const BG_BRIGHTNESS_STEPS = [0.5, 0.75, 1.0, 1.25, 1.5];
export function clampBgBrightness(v: number): number {
  if (!Number.isFinite(v)) return 1.0;
  return Math.min(1.5, Math.max(0.5, v));
}

/** 页面颜色预置色板（纯净风格表面层底色；浅色系为主）。 */
export const PAGE_COLOR_OPTIONS: { hex: string; label: string }[] = [
  { hex: '#ffffff', label: '纯白' },
  { hex: '#f2f4f7', label: '浅灰' },
  { hex: '#faf6ee', label: '米白' },
  { hex: '#eef3fb', label: '浅蓝' },
  { hex: '#eff5ee', label: '浅绿' },
  { hex: '#f4eff8', label: '浅紫' },
];
const DEFAULT_PAGE_COLOR = '#ffffff';
export function clampPageColor(hex: string): string {
  const h = String(hex ?? '').trim().toLowerCase();
  return /^#[0-9a-f]{6}$/.test(h) ? h : DEFAULT_PAGE_COLOR;
}
/** #rrggbb → "r, g, b"（供 warTheme.css 的 rgba(var(--war-page-rgb), a) 使用）。 */
export function hexToRgbTriple(hex: string): string {
  const h = clampPageColor(hex).slice(1);
  const r = parseInt(h.slice(0, 2), 16);
  const g = parseInt(h.slice(2, 4), 16);
  const b = parseInt(h.slice(4, 6), 16);
  return `${r}, ${g}, ${b}`;
}

const LS_FONT_SCALE = 'wardex.fontScale';
const LS_PANEL_LAYOUT = 'wardex.panelLayout';
const LS_UI_STYLE = 'wardex.uiStyle';
const LS_CHAT_ALPHA = 'wardex.chatAlpha';
const LS_PAGE_COLOR = 'wardex.pageColor';
const LS_BG_BRIGHTNESS = 'wardex.bgBrightness';

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
    /** @引用展开方式：'model' 只发标记让模型自读最新内容（默认）；'inject'
     * 发送时把文件内容注入消息。in-memory（暂未持久化到 user_prefs）。 */
    refExpandMode: 'model' as 'model' | 'inject',
    /** File-preview dialog size memory; 0 = not dragged yet (A4 default). */
    previewWidth: 0,
    previewHeight: 0,
    /** Chat-page left rail column width (px); draggable handle in the rail. */
    railWidth: 240,
    /** Shared right-dock drawer width (px) — one width for ALL dock tabs. */
    panelWidth: 240,
    /** 输入框高度(px)：0=未拖过，用响应式 clamp。 */
    composerHeight: 0,
    /** 右下操作台宽度(px)：0=未拖过，用默认 354。 */
    actionBayWidth: 0,
    /** 右下操作台高度(px)：0=未拖过，用响应式公式。 */
    actionBayHeight: 0,
    /** 战场监控页：projectDir → 兵营落点（比例坐标）。 */
    monitorLayout: {} as Record<string, MonitorLayoutEntry>,
    /** 战场监控页：步兵（会话）持久化位置 + 优先级标记。 */
    monitorFootmen: {} as Record<string, MonitorFootmanEntry>,
    /** 战场监控页：优先级区域（默认艾森豪威尔四宫格）。 */
    monitorZones: DEFAULT_MONITOR_ZONES.map((z) => ({ ...z })),
    /** 战场监控页：区域总开关。 */
    monitorZonesOn: true,
    /** 后台会话（监控步兵）完成时是否弹桌面通知。 */
    taskDoneNotify: true,
    /** 界面风格 id（themes.ts 注册表）：'war' | 'pure'。 */
    uiStyle: 'war' as string,
    /** 对话页内容透明度（纯净风格，0.5~1.0，默认 0.8）。 */
    chatAlpha: 0.8,
    /** 页面颜色（纯净风格表面层底色，hex #rrggbb，默认纯白）。 */
    pageColor: DEFAULT_PAGE_COLOR,
    /** 背景亮度（纯净风格，0.5~1.5，默认 1.0，作用于背景图/视频）。 */
    bgBrightness: 1.0,
    /** 监控页会话小窗尺寸(px)：0=未拖过，用默认 560×74vh。 */
    monitorChatWidth: 0,
    monitorChatHeight: 0,
    userAvatarPath: '',
    /** 界面背景：当前生效的背景（默认视频 / background.json / 用户自定义）。 */
    background: { ...DEFAULT_BG } as BgConfig,
    /** 用户自定义背景类型（'' = 无自定义，走默认）。 */
    bgType: '',
    /** 用户自定义背景文件绝对路径（'' = 无自定义）。 */
    bgPath: '',
    loaded: false,
  }),
  getters: {
    /** fs(n): every reading-type font size goes through this (docs §1.4) */
    fs(): (n: number) => number {
      return (n: number) => Math.round(n * this.fontScale);
    },
    /** 是否已设置自定义界面背景（决定「恢复默认」是否可点）。 */
    hasCustomBackground(): boolean {
      return !!this.bgPath;
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
            composerHeight?: number;
            actionBayWidth?: number;
            actionBayHeight?: number;
            monitorLayout?: Record<string, MonitorLayoutEntry>;
            monitorFootmen?: Record<string, MonitorFootmanEntry>;
            monitorZones?: MonitorZone[];
            monitorZonesOn?: boolean;
            monitorChatWidth?: number;
            monitorChatHeight?: number;
            taskDoneNotify?: boolean;
            uiStyle?: string;
            chatAlpha?: number;
            pageColor?: string;
            bgBrightness?: number;
            backgroundType?: string;
            backgroundPath?: string;
          }>('get_prefs');
          this.fontScale = clampScale(p.fontScale ?? 1.0);
          this.panelLayout = p.panelLayout ?? {};
          this.userName = p.userName || '阿尔萨斯';
          this.permissionMode = p.permissionMode || 'default';
          this.previewWidth = p.previewWidth ?? 0;
          this.previewHeight = p.previewHeight ?? 0;
          this.railWidth = p.railWidth ?? 240;
          this.panelWidth = p.panelWidth ?? 240;
          this.composerHeight = p.composerHeight ?? 0;
          this.actionBayWidth = p.actionBayWidth ?? 0;
          this.actionBayHeight = p.actionBayHeight ?? 0;
          this.monitorLayout = p.monitorLayout ?? {};
          this.monitorFootmen = p.monitorFootmen ?? {};
          // 圆形塔区域；旧版矩形格式（w/h 无 r）不兼容 → 直接回落默认布局
          const storedZones = p.monitorZones ?? [];
          const zonesValid =
            storedZones.length > 0 && storedZones.every((z) => z.r != null && z.x != null && z.y != null);
          this.monitorZones = zonesValid
            ? storedZones.map((z) => ({ ...z, deployed: z.deployed ?? true })) // 旧圆形数据视为已部署
            : DEFAULT_MONITOR_ZONES.map((z) => ({ ...z }));
          this.monitorZonesOn = p.monitorZonesOn ?? true;
          this.monitorChatWidth = p.monitorChatWidth ?? 0;
          this.monitorChatHeight = p.monitorChatHeight ?? 0;
          this.taskDoneNotify = p.taskDoneNotify ?? true;
          this.uiStyle = p.uiStyle === 'pure' ? 'pure' : 'war';
          this.chatAlpha = clampChatAlpha(p.chatAlpha ?? 0.8);
          this.pageColor = clampPageColor(p.pageColor ?? DEFAULT_PAGE_COLOR);
          this.bgBrightness = clampBgBrightness(p.bgBrightness ?? 1.0);
          this.userAvatarPath = p.userAvatarPath ?? '';
          this.bgType = p.backgroundType ?? '';
          this.bgPath = p.backgroundPath ?? '';
          this.background = await loadBackground();
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
        const us = localStorage.getItem(LS_UI_STYLE);
        if (us === 'pure') this.uiStyle = 'pure';
        const ca = Number(localStorage.getItem(LS_CHAT_ALPHA));
        if (ca) this.chatAlpha = clampChatAlpha(ca);
        const pc = localStorage.getItem(LS_PAGE_COLOR);
        if (pc) this.pageColor = clampPageColor(pc);
        const bb = Number(localStorage.getItem(LS_BG_BRIGHTNESS));
        if (bb) this.bgBrightness = clampBgBrightness(bb);
      } catch {
        /* ignore */
      }
      this.background = await loadBackground();
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

    /** 上传自定义界面背景: backend copies the file into the data dir and
     * stores its path. Returns false on import failure (UI shows 背景导入失败). */
    async uploadBackground(localPath: string): Promise<boolean> {
      if (!isTauri) return false;
      try {
        const path = await cmd<string>('set_background_from_file', { localPath });
        if (!path) return false;
        await this.reload();
        return true;
      } catch (e) {
        console.warn('[prefs] set_background_from_file failed', e);
        return false;
      }
    },

    /** 恢复默认界面背景: drop the custom file, back to default video. */
    async clearBackground(): Promise<void> {
      if (isTauri) {
        try {
          await cmd('clear_background');
        } catch (e) {
          console.warn('[prefs] clear_background failed', e);
        }
      }
      await this.reload();
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

    /** 输入框高度：拖拽中只改本地 state（不落盘），松手后调用一次持久化。 */
    setComposerHeightLocal(height: number): void {
      this.composerHeight = Math.round(height);
    },

    async setComposerHeight(height: number): Promise<void> {
      this.composerHeight = Math.round(height);
      if (isTauri) {
        try {
          await cmd('set_composer_height', { height: this.composerHeight });
        } catch (e) {
          console.warn('[prefs] set_composer_height failed', e);
        }
      }
    },

    /** 操作台宽度：拖拽中只改本地 state（不落盘），松手后调用一次持久化。 */
    setActionBayWidthLocal(width: number): void {
      this.actionBayWidth = Math.round(width);
    },

    async setActionBayWidth(width: number): Promise<void> {
      this.actionBayWidth = Math.round(width);
      if (isTauri) {
        try {
          await cmd('set_action_bay_width', { width: this.actionBayWidth });
        } catch (e) {
          console.warn('[prefs] set_action_bay_width failed', e);
        }
      }
    },

    /** 操作台高度：拖拽中只改本地 state（不落盘），松手后调用一次持久化。 */
    setActionBayHeightLocal(height: number): void {
      this.actionBayHeight = Math.round(height);
    },

    async setActionBayHeight(height: number): Promise<void> {
      this.actionBayHeight = Math.round(height);
      if (isTauri) {
        try {
          await cmd('set_action_bay_height', { height: this.actionBayHeight });
        } catch (e) {
          console.warn('[prefs] set_action_bay_height failed', e);
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

    /** 后台会话（监控步兵）完成弹桌面通知的开关。 */
    async setTaskDoneNotify(enabled: boolean): Promise<void> {
      this.taskDoneNotify = enabled;
      if (isTauri) {
        try {
          await cmd('set_task_done_notify', { enabled });
        } catch (e) {
          console.warn('[prefs] set_task_done_notify failed', e);
        }
      }
    },

    /** 界面风格（war | pure）：立即生效 + 落盘。 */
    async setUiStyle(style: string): Promise<void> {
      this.uiStyle = style === 'pure' ? 'pure' : 'war';
      if (isTauri) {
        try {
          await cmd('set_ui_style', { style: this.uiStyle });
        } catch (e) {
          console.warn('[prefs] set_ui_style failed', e);
        }
      } else {
        try {
          localStorage.setItem(LS_UI_STYLE, this.uiStyle);
        } catch {
          /* ignore */
        }
      }
    },

    /** 对话页透明度（0.5~1.0）：立即生效 + 落盘。 */
    async setChatAlpha(alpha: number): Promise<void> {
      this.chatAlpha = clampChatAlpha(alpha);
      if (isTauri) {
        try {
          await cmd('set_chat_alpha', { alpha: this.chatAlpha });
        } catch (e) {
          console.warn('[prefs] set_chat_alpha failed', e);
        }
      } else {
        try {
          localStorage.setItem(LS_CHAT_ALPHA, String(this.chatAlpha));
        } catch {
          /* ignore */
        }
      }
    },

    /** 页面颜色（#rrggbb）：立即生效 + 落盘。 */
    async setPageColor(hex: string): Promise<void> {
      this.pageColor = clampPageColor(hex);
      if (isTauri) {
        try {
          await cmd('set_page_color', { hex: this.pageColor });
        } catch (e) {
          console.warn('[prefs] set_page_color failed', e);
        }
      } else {
        try {
          localStorage.setItem(LS_PAGE_COLOR, this.pageColor);
        } catch {
          /* ignore */
        }
      }
    },

    /** 背景亮度（0.5~1.5）：立即生效 + 落盘。 */
    async setBgBrightness(v: number): Promise<void> {
      this.bgBrightness = clampBgBrightness(v);
      if (isTauri) {
        try {
          await cmd('set_bg_brightness', { v: this.bgBrightness });
        } catch (e) {
          console.warn('[prefs] set_bg_brightness failed', e);
        }
      } else {
        try {
          localStorage.setItem(LS_BG_BRIGHTNESS, String(this.bgBrightness));
        } catch {
          /* ignore */
        }
      }
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

    /** 监控页会话小窗尺寸：拖拽松手后调用一次持久化；0/0 = 清除回默认。 */
    async setMonitorChatSize(width: number, height: number): Promise<void> {
      this.monitorChatWidth = Math.round(width);
      this.monitorChatHeight = Math.round(height);
      if (isTauri) {
        try {
          await cmd('set_monitor_chat_size', { width: this.monitorChatWidth, height: this.monitorChatHeight });
        } catch (e) {
          console.warn('[prefs] set_monitor_chat_size failed', e);
        }
      }
    },

    /** 战场监控页兵营落点：先改本地（即时反馈）再落盘；entry=null = 销毁。 */
    async setMonitorLayout(projectDir: string, entry: MonitorLayoutEntry | null): Promise<void> {
      const next = { ...this.monitorLayout };
      if (entry) next[projectDir] = entry;
      else delete next[projectDir];
      this.monitorLayout = next;
      if (isTauri) {
        try {
          await cmd('set_monitor_layout', { projectDir, entry });
        } catch (e) {
          console.warn('[prefs] set_monitor_layout failed', e);
        }
      }
    },
    /** 监控页步兵持久化：先改本地（即时反馈）再落盘；entry=null = 清除
     * （回栏位 / 会话删除时清理）。 */
    async setMonitorFootman(sessionId: string, entry: MonitorFootmanEntry | null): Promise<void> {
      const next = { ...this.monitorFootmen };
      if (entry) next[sessionId] = entry;
      else delete next[sessionId];
      this.monitorFootmen = next;
      if (isTauri) {
        try {
          await cmd('set_monitor_footman', { sessionId, entry });
        } catch (e) {
          console.warn('[prefs] set_monitor_footman failed', e);
        }
      }
    },

    /** 监控页区域：整表替换（先改本地再落盘）；空表 → 前端回退默认模板。 */
    async setMonitorZones(zones: MonitorZone[]): Promise<void> {
      this.monitorZones = zones;
      if (isTauri) {
        try {
          await cmd('set_monitor_zones', { zones });
        } catch (e) {
          console.warn('[prefs] set_monitor_zones failed', e);
        }
      }
    },

    /** 监控页区域总开关。 */
    async setMonitorZonesOn(enabled: boolean): Promise<void> {
      this.monitorZonesOn = enabled;
      if (isTauri) {
        try {
          await cmd('set_monitor_zones_on', { enabled });
        } catch (e) {
          console.warn('[prefs] set_monitor_zones_on failed', e);
        }
      }
    },
  },
});

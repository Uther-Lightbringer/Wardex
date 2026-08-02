// Panel registry — the ONLY extension point of the info panel dock
// (docs/panels.md §1). Adding a panel = one component file + one entry here.
// Components are dynamic imports: collapsed panels load no code and fetch no
// data ("不可见不工作", docs/performance.md §1.4).

import type { Component } from 'vue';

export type RefreshTrigger = 'turnEnd' | 'sessionSwitch' | 'expand' | 'manual';

/** Max drawer panel width (px). The right grid column is sized by the
 * 354px action bay; keeping rail(44) + panel + window-rail clearance(34)
 * ≤ 354 means the column NEVER grows, so opening a drawer never squeezes
 * the chat area. 354 − 44 − 34 = 276. */
export const PANEL_MAX_W = 276;

/** Min drawer panel width (px), matches the frontend drag clamp. */
export const PANEL_MIN_W = 200;

/** Default drawer width (px) — shared by ALL dock tabs (one drag applies
 * to every panel, persisted as `panelWidth`). Also the double-click reset
 * target, matching the left rail's 240 default. */
export const PANEL_DEFAULT_W = 240;

export interface PanelDef {
  id: string; // 'git' | 'files' | 'db' | ... globally unique
  title: string; // Chinese title: 版本控制 / 工作区文件 / 数据库
  icon?: string; // /assets/... icon, optional
  component: () => Promise<{ default: Component }>; // lazy — not loaded while collapsed
  defaultOpen: boolean;
  defaultWidth: number; // px — informational; the SHARED prefs.panelWidth drives rendering
  order: number; // default ordering (v1: fixed, drag-reorder deferred)
  refreshOn: RefreshTrigger[];
}

export const panelRegistry: PanelDef[] = [
  {
    id: 'agent',
    title: '会话信息',
    component: () => import('./AgentPanel.vue'),
    defaultOpen: true, // startup-expanded (WarDock opens the first such panel)
    defaultWidth: 220,
    order: 10,
    refreshOn: ['sessionSwitch'],
  },
  {
    id: 'tasks',
    title: '后台任务',
    component: () => import('./TasksPanel.vue'),
    defaultOpen: false,
    defaultWidth: 220,
    order: 12,
    refreshOn: ['turnEnd', 'sessionSwitch'],
  },
  {
    id: 'todos',
    title: '待办',
    component: () => import('./TodosPanel.vue'),
    defaultOpen: false,
    defaultWidth: 220,
    order: 15,
    refreshOn: ['turnEnd', 'sessionSwitch', 'manual'],
  },
  {
    id: 'git',
    title: '版本控制',
    component: () => import('./GitPanel.vue'),
    defaultOpen: false,
    defaultWidth: 222, // capped by PANEL_MAX_W (drag-resize persists)
    order: 20,
    refreshOn: ['turnEnd', 'sessionSwitch', 'expand', 'manual'],
  },
  {
    id: 'files',
    title: '工作区文件',
    component: () => import('./FilesPanel.vue'),
    defaultOpen: false,
    defaultWidth: 222,
    order: 30,
    refreshOn: ['sessionSwitch', 'expand', 'manual'],
  },
  {
    id: 'db',
    title: '数据库',
    component: () => import('./DbPanel.vue'),
    defaultOpen: false,
    defaultWidth: 222,
    order: 35,
    refreshOn: ['sessionSwitch'],
  },
];

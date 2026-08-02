// Panel registry — the ONLY extension point of the info panel dock
// (docs/panels.md §1). Adding a panel = one component file + one entry here.
// Components are dynamic imports: collapsed panels load no code and fetch no
// data ("不可见不工作", docs/performance.md §1.4).

import type { Component } from 'vue';

export type RefreshTrigger = 'turnEnd' | 'sessionSwitch' | 'expand' | 'manual';

/** Max drawer panel width (px). The right grid column also hosts the 300px
 * action bay; keeping rail(44) + panel + window-rail clearance(34) ≤ 300
 * means the column NEVER grows, so opening a drawer never squeezes the
 * chat area. 300 − 44 − 34 = 222. */
export const PANEL_MAX_W = 222;

export interface PanelDef {
  id: string; // 'git' | 'files' | 'db' | ... globally unique
  title: string; // Chinese title: 版本控制 / 工作区文件 / 数据库
  icon?: string; // /assets/... icon, optional
  component: () => Promise<{ default: Component }>; // lazy — not loaded while collapsed
  defaultOpen: boolean;
  defaultWidth: number; // px, used when there is no panelLayout memory
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
];

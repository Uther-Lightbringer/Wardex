// Panel registry — the ONLY extension point of the info panel dock
// (docs/panels.md §1). Adding a panel = one component file + one entry here.
// Components are dynamic imports: collapsed panels load no code and fetch no
// data ("不可见不工作", docs/performance.md §1.4).

import type { Component } from 'vue';

export type RefreshTrigger = 'turnEnd' | 'sessionSwitch' | 'expand' | 'manual';

export interface PanelDef {
  id: string; // 'git' | 'files' | 'db' | ... globally unique
  title: string; // Chinese title: 版本控制 / 工作区文件 / 数据库
  icon?: string; // /assets/... icon, optional
  component: () => Promise<{ default: Component }>; // lazy — not loaded while collapsed
  defaultOpen: boolean;
  defaultHeight: number; // px, used when there is no panelLayout memory
  order: number; // default ordering (v1: fixed, drag-reorder deferred)
  refreshOn: RefreshTrigger[];
  /** Pinned open at the top, no collapse arrow (the agent panel). */
  alwaysOpen?: boolean;
}

export const panelRegistry: PanelDef[] = [
  {
    id: 'agent',
    title: '会话信息',
    component: () => import('./AgentPanel.vue'),
    defaultOpen: true,
    defaultHeight: 180,
    order: 10,
    refreshOn: ['sessionSwitch'],
    alwaysOpen: true, // pinned top, never collapses (docs/panels.md §2)
  },
  {
    id: 'reminders',
    title: '提醒',
    component: () => import('./RemindersPanel.vue'),
    defaultOpen: false,
    defaultHeight: 200,
    order: 15,
    refreshOn: ['turnEnd', 'sessionSwitch', 'manual'],
  },
  {
    id: 'git',
    title: '版本控制',
    component: () => import('./GitPanel.vue'),
    defaultOpen: true,
    defaultHeight: 260, // roomier default for the diff view (drag-resize persists)
    order: 20,
    refreshOn: ['turnEnd', 'sessionSwitch', 'expand', 'manual'],
  },
  {
    id: 'files',
    title: '工作区文件',
    component: () => import('./FilesPanel.vue'),
    defaultOpen: false,
    defaultHeight: 260,
    order: 30,
    refreshOn: ['sessionSwitch', 'expand', 'manual'],
  },
];

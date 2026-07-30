// Global UI gate + banner + modal visibility.
// `busy` is the uiGate equivalent (src/UiGate.h in the old app): while a page
// transition runs, every WarButton grays out and shortcuts go dead.

import { defineStore } from 'pinia';

let bannerTimer: ReturnType<typeof setTimeout> | null = null;

export const useUiStore = defineStore('ui', {
  state: () => ({
    busy: false,
    bannerText: '',
    folderDialogOpen: false,
    /** Why the folder dialog is open: 'open' = open project as new session
     * (default), 'bind' = bind the current session to a project dir. */
    folderDialogPurpose: 'open' as 'open' | 'bind',
    /** Window-size driven UI scale for the main menu (docs/ui-design.md §5.1) */
    uiScale: 1,
  }),
  actions: {
    showBanner(msg: string): void {
      this.bannerText = msg;
      if (bannerTimer) clearTimeout(bannerTimer);
      bannerTimer = setTimeout(() => {
        this.bannerText = '';
      }, 3500);
    },
    updateUiScale(w: number, h: number): void {
      this.uiScale = Math.max(0.45, Math.min(w / 1280, h / 720));
    },
  },
});

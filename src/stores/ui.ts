// Global UI gate + banner + modal visibility.
// `busy` is the uiGate equivalent (src/UiGate.h in the old app): while a page
// transition runs, every WarButton grays out and shortcuts go dead.

import { defineStore } from 'pinia';

let bannerTimer: ReturnType<typeof setTimeout> | null = null;

/** Payload of the backend `wardex://agentStartFailed` event: a session whose
 * ACP spawn failed while it was NOT the active session (startup prewarm,
 * project-due session, monitor mini-chat, plugin re-apply...). The frontend
 * drops `chat://status` for those, so the backend raises a modal instead. */
export interface StartFailure {
  sessionId: string;
  agentName: string;
  sessionTitle: string;
  projectDir: string;
  error: string;
}

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
    /** Modal shown when a background session's agent fails to start. */
    startFailure: null as StartFailure | null,
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
    showStartFailure(f: StartFailure): void {
      // Latest failure wins: the dialog holds one payload, and piling up
      // dialogs for a batch of broken sessions would just be noise.
      this.startFailure = f;
    },
    closeStartFailure(): void {
      this.startFailure = null;
    },
  },
});

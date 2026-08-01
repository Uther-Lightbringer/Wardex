// Page state machine + the WC3 three-stage transition (docs/ui-design.md §5.6).
// Six page states: main | config | sessionSelect | chat | todo | usage.
//
// All navigation is the same "pull up → wait for the popUp SFX → drop down":
//   1. Up-slide (770ms, ease-in-quad) of the current layer + popUp SFX.
//   2. Wait out the popUp audible length (1280ms total from the click, so
//      ~510ms after the up-slide ends). The drop may NOT start earlier.
//   3. Drop (popDown SFX): main menu slides down 770ms ease-out-quad; overlay
//      page content drops 750ms ease-out-quad (ShellFrame dropDuration).
// The whole transition runs under uiGate busy (ui.busy) so every WarButton
// is dead until the new page has landed.
//
// Layer model (mirrors Main.qml):
//   menuY     — main-menu band (both rails ride it), 0 on screen / -2400 off
//   overlayY  — whole overlay band, only used for overlay→main and
//               overlay→overlay up-slides
//   contentY  — per-page content band inside the overlay (PageShell binds
//               it); parked above the viewport before a page becomes visible
//               so it never flashes one frame at its final position.

import { defineStore } from 'pinia';
import { delay, easeInQuad, easeOutQuad, tween } from '../lib/animate';
import { play, POP_UP_SOUND_MS } from '../lib/sfx';
import { useUiStore } from './ui';

export type PageId = 'main' | 'config' | 'sessionSelect' | 'chat' | 'todo' | 'usage';

const POP_UP_DUR = 770; // up-slide of menu / overlay band
const MENU_DOWN_DUR = 770; // main menu slide-down
const CONTENT_DROP_DUR = 750; // ShellFrame drop-in for overlay pages
const OFF_Y = -2400; // off-screen parking position (Main.qml offY)

type Phase = 'idle' | 'up' | 'down';

function parkPx(): number {
  return Math.max(window.innerHeight, 900);
}

export const useNavStore = defineStore('nav', {
  state: () => ({
    page: 'main' as PageId,
    phase: 'idle' as Phase,
    menuY: 0,
    overlayY: OFF_Y,
    contentY: 0,
    /** Pages are built on first visit and kept resident (v-show afterwards). */
    visited: { main: true } as Record<PageId, boolean>,
  }),

  actions: {
    /** main → overlay, or overlay → other overlay. */
    async goOverlay(target: PageId): Promise<void> {
      const ui = useUiStore();
      if (this.phase !== 'idle') return;
      if (this.page === target) return;

      this.visited[target] = true;
      ui.busy = true;
      this.phase = 'up';
      play('popUp');
      const t0 = Date.now();

      if (this.page === 'main') {
        // Mount the target page now (content parked above) so its build
        // overlaps the up-slide, then slide the menu band off.
        this.contentY = -parkPx();
        this.overlayY = 0;
        this.page = target;
        await tween(0, OFF_Y, POP_UP_DUR, easeInQuad, (v) => (this.menuY = v));
        this.menuY = OFF_Y;
      } else {
        // overlay → overlay: lift the whole band with the current page, then
        // swap and reset the band; the new page's content is parked above.
        await tween(0, OFF_Y, POP_UP_DUR, easeInQuad, (v) => (this.overlayY = v));
        this.overlayY = 0;
        this.contentY = -parkPx();
        this.page = target;
      }

      // Wait out the popUp SFX audible length from the click (1280ms gate).
      await delay(Math.max(0, POP_UP_SOUND_MS - (Date.now() - t0)));

      this.phase = 'down';
      play('popDown');
      await tween(-parkPx(), 0, CONTENT_DROP_DUR, easeOutQuad, (v) => (this.contentY = v));
      this.contentY = 0;
      this.phase = 'idle';
      ui.busy = false;
    },

    /** overlay → main menu. */
    async goMain(): Promise<void> {
      const ui = useUiStore();
      if (this.phase !== 'idle') return;
      if (this.page === 'main') {
        this.menuY = 0;
        this.overlayY = OFF_Y;
        return;
      }

      ui.busy = true;
      this.phase = 'up';
      play('popUp');
      const t0 = Date.now();

      await tween(0, OFF_Y, POP_UP_DUR, easeInQuad, (v) => (this.overlayY = v));
      this.overlayY = OFF_Y;
      await delay(Math.max(0, POP_UP_SOUND_MS - (Date.now() - t0)));

      this.phase = 'down';
      this.page = 'main';
      play('popDown');
      await tween(OFF_Y, 0, MENU_DOWN_DUR, easeOutQuad, (v) => (this.menuY = v));
      this.menuY = 0;
      this.phase = 'idle';
      ui.busy = false;
    },
  },
});

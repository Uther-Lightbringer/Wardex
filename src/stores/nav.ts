// Page state machine + theme-aware page transition (docs/ui-design.md §5.6).
// Page states: main | hub | config | settings | sessionSelect | chat | todo |
// usage | monitor. hub is the 更多功能 mid-level menu grouping settings /
// config / usage / todo.
//
// The transition follows the theme (themeOf(prefs.uiStyle).kind):
//   war  — WC3 three-stage "pull up → popUp gate → drop down":
//     1. Up-slide (450ms, ease-in-quad) of the current layer + popUp SFX.
//     2. Wait the drop gate (950ms total from the click, so ~500ms after the
//        up-slide ends). The drop may NOT start earlier — popDown would cut
//        the popUp tail (gate is tuned down from the raw 1280ms audible
//        length for snappier transitions; see lib/sfx.ts).
//     3. Drop (popDown SFX): main menu slides down 450ms ease-out-quad;
//        overlay page content drops 450ms ease-out-quad (ShellFrame drop).
//   pure — modern fade: current layer fades out 300ms (ease-in-quad), then
//     the new page fades in 400ms (ease-out-quad) while rising 14px
//     (contentY 14→0). No SFX, no drop gate.
// The whole transition runs under uiGate busy (ui.busy) so every WarButton
// is dead until the new page has landed.
//
// Layer model (mirrors Main.qml):
//   menuY     — main-menu band (both rails ride it), 0 on screen / -2400 off
//   overlayY  — whole overlay band, used for overlay→main and overlay→overlay
//               up-slides (war only; pure keeps it 0)
//   contentY  — per-page content band inside the overlay (PageShell binds
//               it); parked above the viewport before a page becomes visible
//               so it never flashes one frame at its final position
//   menuOpacity / contentOpacity — pure-theme fade states (war keeps both 1)

import { defineStore } from 'pinia';
import { delay, easeInQuad, easeOutQuad, tween } from '../lib/animate';
import { play } from '../lib/sfx';
import { themeOf } from '../lib/themes';
import { usePrefsStore } from './prefs';
import { useUiStore } from './ui';

export type PageId = 'main' | 'hub' | 'config' | 'settings' | 'sessionSelect' | 'chat' | 'todo' | 'usage' | 'monitor';

const POP_UP_DUR = 450; // war: up-slide of menu / overlay band
const MENU_DOWN_DUR = 450; // war: main menu slide-down
const CONTENT_DROP_DUR = 450; // war: ShellFrame drop-in for overlay pages
const OFF_Y = -2400; // off-screen parking position (Main.qml offY)

// pure theme: modern fade-in with a subtle rise (no SFX, no drop gate).
const PURE_FADE_OUT_MS = 300; // current layer fade-out
const PURE_FADE_IN_MS = 400; // new page fade-in
const PURE_RISE_PX = 14; // content starts 14px low and rises to 0

/** Drop gate (ms from the click): the popUp wav stays audible ~1280ms (see
 * sfx.ts), but 950ms covers most of its body — the popDown start cuts the
 * inaudible tail (single-channel audio) and keeps the swap snappy. */
const DROP_GATE_MS = 950;

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
    /** pure-theme fade states (war keeps both at 1). */
    menuOpacity: 1,
    contentOpacity: 1,
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
      const pure = themeOf(usePrefsStore().uiStyle).kind === 'plain';
      this.phase = 'up';

      if (pure) {
        // Fade the current layer out (no SFX, no drop gate).
        if (this.page === 'main') {
          await tween(1, 0, PURE_FADE_OUT_MS, easeInQuad, (v) => (this.menuOpacity = v));
        } else {
          await tween(1, 0, PURE_FADE_OUT_MS, easeInQuad, (v) => (this.contentOpacity = v));
        }
        // Swap page; the new content starts transparent + 14px low (no flash).
        // Pure has no band slides, but menuY / overlayY must sit at 0 — they
        // may still hold war values (OFF_Y) after a live theme switch.
        this.menuY = 0;
        this.overlayY = 0;
        this.contentOpacity = 0;
        this.contentY = PURE_RISE_PX;
        this.page = target;
        this.phase = 'down';
        await Promise.all([
          tween(0, 1, PURE_FADE_IN_MS, easeOutQuad, (v) => (this.contentOpacity = v)),
          tween(PURE_RISE_PX, 0, PURE_FADE_IN_MS, easeOutQuad, (v) => (this.contentY = v)),
        ]);
        this.contentY = 0;
        this.contentOpacity = 1;
        this.phase = 'idle';
        ui.busy = false;
        return;
      }

      // — war: WC3 three-stage pull-up / drop-down —
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
      await delay(Math.max(0, DROP_GATE_MS - (Date.now() - t0)));

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
        this.menuOpacity = 1;
        this.contentOpacity = 1;
        return;
      }

      ui.busy = true;
      const pure = themeOf(usePrefsStore().uiStyle).kind === 'plain';
      this.phase = 'up';

      if (pure) {
        await tween(1, 0, PURE_FADE_OUT_MS, easeInQuad, (v) => (this.contentOpacity = v));
        this.phase = 'down';
        // Bands stay parked on screen in pure (fade only); normalize them in
        // case a live theme switch left war slide values behind.
        this.menuY = 0;
        this.overlayY = 0;
        this.page = 'main';
        await tween(0, 1, PURE_FADE_IN_MS, easeOutQuad, (v) => (this.menuOpacity = v));
        this.menuOpacity = 1;
        this.phase = 'idle';
        ui.busy = false;
        return;
      }

      // — war: WC3 three-stage pull-up / drop-down —
      play('popUp');
      const t0 = Date.now();

      await tween(0, OFF_Y, POP_UP_DUR, easeInQuad, (v) => (this.overlayY = v));
      this.overlayY = OFF_Y;
      await delay(Math.max(0, DROP_GATE_MS - (Date.now() - t0)));

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

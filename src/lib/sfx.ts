// UI sound effects — Web Audio equivalent of the old Win32 PlaySound module
// (AppSound.cpp). Three events, single-channel semantics:
//   - same-name requests within 200ms are dropped (throttle)
//   - starting a new clip stops the previous one (single channel)
//   - all three clips are preloaded at startup
// The 1280ms audible-length popUp gate lives in stores/nav.ts.

const files: Record<string, string> = {
  click: '/assets/Sound/BigButtonClick.wav',
  popUp: '/assets/Sound/RightGlueScreenPopUp.wav',
  popDown: '/assets/Sound/RightGlueScreenPopDown.wav',
};

export type SfxName = keyof typeof files;

/** Audible length of RightGlueScreenPopUp.wav (file 1866ms; the ~590ms tail
 * below ~2% peak is inaudible). Page transitions must wait this long from
 * the click before the drop-in may start. */
export const POP_UP_SOUND_MS = 1280;

const THROTTLE_MS = 200;

let current: HTMLAudioElement | null = null;
const lastPlay: Record<string, number> = {};
const preloaded: Record<string, HTMLAudioElement> = {};

/** Preload all clips. Call once at app startup. The first user gesture may
 * still reject play() — every play() call itself doubles as the unlock. */
export function preloadSfx(): void {
  for (const [name, src] of Object.entries(files)) {
    const a = new Audio(src);
    a.preload = 'auto';
    preloaded[name] = a;
  }
}

export function play(name: SfxName): void {
  const now = Date.now();
  if (now - (lastPlay[name] ?? 0) < THROTTLE_MS) return; // same-name throttle
  lastPlay[name] = now;
  // Single channel: stop the previous clip before starting the new one.
  if (current) {
    current.pause();
    current.currentTime = 0;
  }
  // Reuse the preloaded element when it is idle so replays of the same WAV
  // start instantly; otherwise fall back to a fresh element.
  const idle = preloaded[name];
  const a = idle && idle.paused ? idle : new Audio(files[name]);
  current = a;
  a.currentTime = 0;
  a.play().catch(() => {
    // Rejected before the first user gesture — harmless, next click unlocks.
  });
}

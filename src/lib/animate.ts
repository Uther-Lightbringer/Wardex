// Tiny rAF tween helper used by the page state machine (stores/nav.ts).
// Easing curves match Qt Easing.InQuad / Easing.OutQuad.

export const easeInQuad = (t: number): number => t * t;
export const easeOutQuad = (t: number): number => 1 - (1 - t) * (1 - t);

export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Tween a numeric value with requestAnimationFrame; resolves at the end. */
export function tween(
  from: number,
  to: number,
  durationMs: number,
  ease: (t: number) => number,
  onUpdate: (v: number) => void,
): Promise<void> {
  return new Promise((resolve) => {
    const t0 = performance.now();
    const step = (now: number): void => {
      const t = Math.min(1, (now - t0) / durationMs);
      onUpdate(from + (to - from) * ease(t));
      if (t < 1) requestAnimationFrame(step);
      else resolve();
    };
    requestAnimationFrame(step);
  });
}

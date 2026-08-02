// Background configuration (docs/ui-design.md §8, docs/assets.md).
// Default: bundled LodolonFall.jpg. background.json next to the exe overrides
// it (resolved by the Rust `background_config` command, old main.cpp rules);
// in browser/dev preview we fall back to fetching /background.json.
// `type: "video"` plays in a muted looping <video> (WebView2 native H.264);
// `type: "model"` (Three.js glTF) is a deferred TODO and maps to image.

import { cmd, fileSrc, isTauri } from './tauri';

export interface BgConfig {
  type: 'image' | 'video' | 'model';
  source: string;
}

export const DEFAULT_BG: BgConfig = {
  type: 'image',
  source: '/assets/background/LodolonFall.jpg',
};

/** Resolve the background: exe-adjacent background.json via Rust first. */
export async function loadBackground(): Promise<BgConfig> {
  if (isTauri) {
    try {
      const cfg = await cmd<{ type?: string; source?: string }>('background_config');
      if (cfg && typeof cfg.source === 'string' && cfg.source) {
        if (cfg.type === 'image' || cfg.type === 'video') {
          return { type: cfg.type, source: mapSource(cfg.source) };
        }
        // model (deferred) / anything else → default image
      }
      return DEFAULT_BG;
    } catch {
      return DEFAULT_BG;
    }
  }
  // Browser preview fallback: served /background.json (drop one into public/).
  try {
    const res = await fetch('/background.json', { cache: 'no-store' });
    if (!res.ok) return DEFAULT_BG;
    const cfg = (await res.json()) as { type?: string; source?: string };
    if (
      cfg &&
      (cfg.type === 'image' || cfg.type === 'video') &&
      typeof cfg.source === 'string' &&
      cfg.source
    ) {
      return { type: cfg.type, source: mapSource(cfg.source) };
    }
    return DEFAULT_BG;
  } catch {
    return DEFAULT_BG;
  }
}

/** qrc:/absolute/relative source forms → webview-loadable URLs. */
function mapSource(src: string): string {
  if (src.startsWith('qrc:')) {
    // qrc:/qt/qml/WarDex/assets/... → bundled /assets/...
    const m = src.match(/assets\/(.+)$/);
    return m ? `/assets/${m[1]}` : DEFAULT_BG.source;
  }
  if (src.startsWith('/assets/')) return src;
  // Plain filesystem path (absolute, or relative anchored at the exe dir by
  // the Rust side) → asset protocol URL. gif/webp animate natively in <img>.
  return fileSrc(src);
}

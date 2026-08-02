// Tauri IPC helpers. Every command call goes through `cmd`, which no-ops
// (returns fallback) when running in a plain browser (vite dev without the
// Tauri runtime) so the UI skeleton stays previewable.

import { invoke, convertFileSrc } from '@tauri-apps/api/core';

export const isTauri =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export async function cmd<T>(
  name: string,
  args?: Record<string, unknown>,
  fallback?: T,
): Promise<T> {
  if (!isTauri) {
    if (fallback !== undefined) return fallback;
    throw new Error(`cmd(${name}): Tauri runtime not available`);
  }
  // Switch-lag instrumentation: backend bodies measured <50ms while the
  // frontend saw ~1s — log slow IPC round trips per command to catch where
  // the dispatch stalls (temporary).
  const t0 = performance.now();
  try {
    return await invoke<T>(name, args);
  } finally {
    const dt = performance.now() - t0;
    if (dt > 100) console.info(`[ipc] ${name}: ${dt.toFixed(1)}ms`);
  }
}

/** Local file → asset-protocol URL (attachments, avatars, preview images). */
export function fileSrc(path: string): string {
  return convertFileSrc(path);
}

/** Open a file with the system default handler (tauri-plugin-opener). */
export async function openPath(path: string): Promise<void> {
  if (!isTauri) return;
  try {
    await invoke('plugin:opener|open_path', { path });
  } catch (e) {
    console.warn('[opener] open_path failed', e);
  }
}

/** Open a URL in the system browser (tauri-plugin-opener). */
export async function openUrl(url: string): Promise<void> {
  if (!isTauri) return;
  try {
    await invoke('plugin:opener|open_url', { url });
  } catch (e) {
    console.warn('[opener] open_url failed', e);
  }
}

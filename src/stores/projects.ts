// Recent projects (projects.json): recent list (max 8, newest first) +
// aliases map. Display name = alias or folder base name.

import { defineStore } from 'pinia';
import { cmd, isTauri } from '../lib/tauri';

export interface RecentProject {
  path: string;
  lastOpenedAt: number; // ms epoch
}

export function folderBaseName(path: string): string {
  const parts = String(path).split(/[\\/]/).filter((s) => s.length > 0);
  return parts.length ? parts[parts.length - 1] : String(path);
}

export const useProjectsStore = defineStore('projects', {
  state: () => ({
    recent: [] as RecentProject[],
    aliases: {} as Record<string, string>,
  }),
  getters: {
    displayName(): (path: string) => string {
      return (path: string) => {
        if (this.aliases[path]) return this.aliases[path];
        // aliases are keyed by canonical dir; compare case-insensitively
        const lower = path.toLowerCase();
        for (const [k, v] of Object.entries(this.aliases)) {
          if (k.toLowerCase() === lower && v) return v;
        }
        return folderBaseName(path);
      };
    },
  },
  actions: {
    async load(): Promise<void> {
      if (!isTauri) return;
      try {
        const v = await cmd<{ recent?: RecentProject[]; aliases?: Record<string, string> }>(
          'list_projects',
        );
        this.recent = v.recent ?? [];
        this.aliases = v.aliases ?? {};
      } catch (e) {
        console.warn('[projects] list_projects failed', e);
      }
    },
    /** Touch recents and return the canonical dir (open_project command). */
    async open(dir: string): Promise<void> {
      if (!isTauri) return;
      try {
        await cmd('open_project', { dir });
      } catch (e) {
        console.warn('[projects] open_project failed', e);
      }
      await this.load();
    },
    async remove(dir: string): Promise<void> {
      if (!isTauri) return;
      try {
        await cmd('remove_project', { dir });
      } catch (e) {
        console.warn('[projects] remove_project failed', e);
      }
      await this.load();
    },
    async setAlias(dir: string, alias: string): Promise<void> {
      if (!isTauri) return;
      try {
        await cmd('set_project_alias', { dir, alias });
      } catch (e) {
        console.warn('[projects] set_project_alias failed', e);
      }
      await this.load();
    },
  },
});

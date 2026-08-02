// Agent configuration store (agents/index.json + agents/<id>.json, spec §7-§9)
// plus the provider registry view and the CLI probe cache. The Rust stores
// are authoritative; every mutation is a Tauri command followed by a re-pull.
//
// Kept separate from stores/sessions.ts (which holds the rail's lightweight
// AgentInfo list for the chat page) because the config page needs the FULL
// record — apiKey, mcpServers, extraArgs — that the rail never touches.

import { defineStore } from 'pinia';
import { cmd, isTauri } from '../lib/tauri';

/** Full agent record (store/agents.rs Agent, camelCase). */
export interface AgentRecord {
  id: string;
  name: string;
  provider: string;
  model: string;
  baseUrl: string;
  /** "" = 跟随 CLI；否则为 thinking effort 档位（low/high/…，见 models.rs）。 */
  defaultEffort: string;
  /** 刷新批量同步写入 config.toml 的 max_context_size，单位 K；0 = 256K 兜底。 */
  maxContextK: number;
  cliPath: string;
  /** PLAINTEXT from the backend — display surfaces must maskKey() it. */
  apiKey: string;
  extraArgs: string;
  /** Raw JSON array TEXT; parsed only when a session starts. */
  mcpServers: string;
  avatarPath: string;
  enabled: boolean;
  isDefault: boolean;
  createdAt: number;
  updatedAt: number;
}

/** Save patch; absent key = field untouched (Rust Option semantics). */
export type AgentPatch = Partial<
  Pick<
    AgentRecord,
    | 'name'
    | 'provider'
    | 'model'
    | 'baseUrl'
    | 'defaultEffort'
    | 'maxContextK'
    | 'cliPath'
    | 'apiKey'
    | 'extraArgs'
    | 'mcpServers'
    | 'avatarPath'
    | 'enabled'
  >
>;

/** Provider registry view (provider_specs command; SpecView in Rust). */
export interface SpecView {
  id: string;
  displayName: string;
  defaultCommand: string;
  acpArgs: string;
  installHint: string;
  baseUrlHint: string;
  chatCapable: boolean;
}

/** CLI probe outcome (probe_cli command; ProbeResult in Rust). */
export interface ProbeResult {
  providerId: string;
  found: boolean;
  path: string;
  version: string;
  error: string;
  message: string;
}

/** maskKey (agents.rs): ≤8 chars fully masked, else left(3)+"****"+right(4). */
export function maskKey(key: string): string {
  if (!key) return '';
  const chars = [...key];
  if (chars.length <= 8) return '********';
  return chars.slice(0, 3).join('') + '****' + chars.slice(-4).join('');
}

/** OpenCode Go 端点识别：Base URL 命中 opencode.ai 的 zen/go 即需要前缀。 */
export function isOpenCodeGoBaseUrl(baseUrl: string): boolean {
  return baseUrl.trim().includes('opencode.ai/zen/go');
}

/**
 * OpenCode Go 模型 ID 规范（docs/zh-cn/go.md）：模型引用需带
 * `opencode-go/<model-id>` 前缀。`fetch_models` 返回的是裸 id（如
 * `deepseek-v4-flash`），此处按需补齐前缀；非 Go 端点原样返回。
 */
export function goModelId(baseUrl: string, id: string): string {
  if (!isOpenCodeGoBaseUrl(baseUrl)) return id;
  return id.startsWith('opencode-go/') ? id : `opencode-go/${id}`;
}

/**
 * isBareCliPath (spec §9.2): empty, or the provider's defaultCommand with an
 * optional .exe/.cmd suffix. Bare paths are (re)probed automatically; the
 * custom provider never probes (no canonical CLI).
 */
export function isBareCliPath(spec: SpecView | undefined, cliPath: string): boolean {
  if (!spec || spec.id === 'custom') return false;
  const p = cliPath.trim().toLowerCase();
  if (!p) return true;
  const base = spec.defaultCommand.toLowerCase();
  return p === base || p === base + '.exe' || p === base + '.cmd';
}

export const useAgentsStore = defineStore('agents', {
  state: () => ({
    agents: [] as AgentRecord[],
    defaultAgentId: '',
    specs: [] as SpecView[],
    /** Probe results by provider id — switching agent/provider re-displays
     * the cached line instantly (spec §9.2). */
    probeCache: {} as Record<string, ProbeResult>,
    probing: {} as Record<string, boolean>,
    /** Endpoint /models cache by agent id (stale-while-revalidate): the chat
     * page renders the cached list instantly on session open and refreshes
     * in the background. In-memory only — one cold fetch per agent after
     * an app restart. Invalidated when baseUrl/apiKey changes. */
    modelsByAgent: {} as Record<string, string[]>,
    modelsFetching: {} as Record<string, boolean>,
    lastError: '',
    /** Non-fatal save warning (e.g. kimi config.toml effort sync failed). */
    lastWarning: '',
    loaded: false,
  }),
  getters: {
    specOf(): (id: string) => SpecView | undefined {
      return (id: string) => this.specs.find((s) => s.id === id.trim().toLowerCase());
    },
    byId(): (id: string) => AgentRecord | undefined {
      return (id: string) => this.agents.find((a) => a.id === id);
    },
    /** canUseForChat: enabled + a registered chat-capable provider. */
    usable(): (a: AgentRecord) => boolean {
      return (a: AgentRecord) => {
        const spec = this.specOf(a.provider);
        return a.enabled && !!spec && spec.chatCapable;
      };
    },
  },
  actions: {
    async refresh(): Promise<void> {
      if (!isTauri) return;
      try {
        const v = await cmd<{ agents?: AgentRecord[]; defaultAgentId?: string }>('list_agents');
        this.agents = v.agents ?? [];
        this.defaultAgentId = v.defaultAgentId ?? '';
        this.loaded = true;
      } catch (e) {
        console.warn('[agents] list_agents failed', e);
      }
    },

    async loadSpecs(): Promise<void> {
      if (!isTauri) return;
      try {
        this.specs = await cmd<SpecView[]>('provider_specs', undefined, []);
      } catch (e) {
        console.warn('[agents] provider_specs failed', e);
      }
    },

    /** 新建 Agent (§7): provider kimi / model moonshot-v1-auto / cliPath ""
     * (empty → auto-probe). Returns the new id. */
    async create(name: string): Promise<string> {
      const id = await cmd<string>('create_agent', { name });
      await this.refresh();
      return id;
    },

    /** saveCurrent: patch write-through. The apiKey guard lives in Rust
     * (empty/still-masked keeps the old value). */
    async save(id: string, patch: AgentPatch): Promise<boolean> {
      try {
        this.lastError = '';
        this.lastWarning = (await cmd<string | null>('save_agent', { agentId: id, patch }, null)) ?? '';
        // Endpoint identity changed → the cached /models list no longer
        // belongs to this agent.
        if ('baseUrl' in patch || 'apiKey' in patch) {
          const rest = { ...this.modelsByAgent };
          delete rest[id];
          this.modelsByAgent = rest;
        }
        await this.refresh();
        return true;
      } catch (e) {
        this.lastError = String(e);
        return false;
      }
    },

    /** Endpoint /models with stale-while-revalidate: single-flight per
     * agent; the UI reads modelsByAgent synchronously and re-renders when
     * the fresh list lands. Failures keep the previous cache. */
    async ensureEndpointModels(agentId: string): Promise<void> {
      if (!isTauri || !agentId || this.modelsFetching[agentId]) return;
      const a = this.byId(agentId);
      const baseUrl = a?.baseUrl.trim() ?? '';
      if (!baseUrl) return; // CLI-managed agents have no endpoint list
      this.modelsFetching = { ...this.modelsFetching, [agentId]: true };
      try {
        const ids = await cmd<string[]>('fetch_models', {
          baseUrl,
          apiKey: a?.apiKey ?? '',
        });
        // OpenCode Go 端点：裸 id 补 `opencode-go/` 前缀，与配置页刷新一致。
        const prefixed = ids.map((id) => goModelId(baseUrl, id));
        // The agent may have been edited mid-flight; only adopt the result
        // when the endpoint identity is unchanged.
        if (this.byId(agentId)?.baseUrl.trim() === baseUrl) {
          this.modelsByAgent = { ...this.modelsByAgent, [agentId]: prefixed };
        }
      } catch (e) {
        console.warn(`[agents] fetch_models(${agentId}) failed, keeping cache`, e);
      } finally {
        this.modelsFetching = { ...this.modelsFetching, [agentId]: false };
      }
    },

    /** 删除 Agent (§9.3): no confirmation; when the default goes away the
     * backend hands the flag to the first remaining agent. */
    async remove(id: string): Promise<boolean> {
      try {
        this.lastError = '';
        await cmd('delete_agent', { agentId: id });
        await this.refresh();
        return true;
      } catch (e) {
        this.lastError = String(e);
        return false;
      }
    },

    async setDefault(id: string): Promise<boolean> {
      try {
        this.lastError = '';
        await cmd('set_default_agent', { agentId: id });
        await this.refresh();
        return true;
      } catch (e) {
        this.lastError = String(e);
        return false;
      }
    },

    /** probe_cli: async scan, cached per provider. The caller checks
     * result.providerId against its CURRENT draft provider before applying
     * anything (stale-provider guard, spec §9.2). */
    async probe(providerId: string, preferredPath: string): Promise<ProbeResult | null> {
      const key = providerId.trim().toLowerCase();
      if (this.probing[key]) return null;
      this.probing = { ...this.probing, [key]: true };
      try {
        const r = await cmd<ProbeResult>('probe_cli', {
          providerId: key,
          preferredPath: preferredPath ?? '',
        });
        this.probeCache = { ...this.probeCache, [key]: r };
        return r;
      } catch (e) {
        console.warn('[agents] probe_cli failed', e);
        return null;
      } finally {
        this.probing = { ...this.probing, [key]: false };
      }
    },

    /** testAgent (§9.3): single-flight in Rust; null = a test is already
     * running (this click was ignored). Success = ACP initialize handshake. */
    async test(id: string): Promise<string | null> {
      try {
        this.lastError = '';
        return await cmd<string | null>('test_agent', { agentId: id });
      } catch (e) {
        return String(e);
      }
    },
  },
});

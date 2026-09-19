<script setup lang="ts">
// Agent config page (features/sessions-and-config.md 第二部分):
// left = Agent list (★ default gold rim, zebra, autosave-on-switch) +
// 新建 Agent; right = scrollable Agent editor (draft + dirty flag); right
// bottom = 保存并返回/返回.
//   应用级偏好（我的名字/字体缩放/通知/头像/背景/界面风格/透明度/页面颜色）
//   已拆分到「设置」页 SettingsPage.vue；本页只保留 Agent 编辑。
//   - Draft model: fields edit a local draft + dirty flag; only an explicit
//     save writes through (switch row / new / set-default / test / save-back
//     all save first when dirty).
//   - apiKey arrives PLAINTEXT from the backend but is loaded into the draft
//     masked (maskKey); the Rust apiKey guard ignores masked/empty write-backs.
//   - CLI probe is async with a per-provider result cache; results arriving
//     after a provider switch are discarded (stale-provider guard, spec §9.2).
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import PageShell from '../components/PageShell.vue';
import WarFrame from '../components/war/WarFrame.vue';
import WarButton from '../components/war/WarButton.vue';
import WarDropdown from '../components/war/WarDropdown.vue';
import WarDialog from '../components/war/WarDialog.vue';
import { cmd, fileSrc, isTauri, openUrl } from '../lib/tauri';
import { copyText } from '../lib/clipboard';
import { useNavStore } from '../stores/nav';
import { CHAT_ALPHA_STEPS, FONT_SCALE_STEPS, usePrefsStore } from '../stores/prefs';
import { THEMES } from '../lib/themes';
import {
  goModelId,
  isBareCliPath,
  maskKey,
  useAgentsStore,
  type AgentRecord,
  type PiProbeResult,
  type TestAgentResult,
  type TestFrame,
} from '../stores/agents';

const nav = useNavStore();
const prefs = usePrefsStore();
const agents = useAgentsStore();

const BUILTIN_AGENT_AVATAR = '/assets/ui/avatars/avatar_agent.png';

// ---------------------------------------------------------------------------
// Selection + draft (spec §9.1)
// ---------------------------------------------------------------------------

const selectedId = ref('');
const draft = reactive({
  name: '',
  provider: 'pi',
  model: '',
  baseUrl: '',
  /** 允许的思考强度档位（low/high/…，空 = 全部；后端 models.rs effective_efforts）。 */
  effortOptions: [] as string[],
  /** 上下文长度（K），随思考强度声明写入 config.toml 的 max_context_size；0/空 = 256。 */
  maxContextK: 256,
  /** 模型是否支持图片输入（视觉/多模态）；勾选 → pi models.json 声明
   * input: ["text","image"]，否则只声明 text，pi 会丢掉所有图片。 */
  supportsImage: true,
  cliPath: '',
  piDir: '',
  apiKey: '',
  extraArgs: '',
  mcpServers: '',
  avatarPath: '',
});
const dirty = ref(false);
const statusMsg = ref('');

const spec = computed(() => agents.specOf(draft.provider));
const providerOptions = computed(() => agents.specs.map((s) => s.displayName));
const providerIndex = computed(() => agents.specs.findIndex((s) => s.id === draft.provider));
const isCustom = computed(() => draft.provider === 'custom');

// Preset base URLs (OpenAI-compatible roots). Picking one fills the Base URL
// field; the field stays free-text so any custom endpoint still works.
const baseUrlPresets: { name: string; url: string }[] = [
  { name: 'DeepSeek', url: 'https://api.deepseek.com/v1' },
  { name: 'Kimi', url: 'https://api.kimi.com/coding/v1' },
  { name: 'OpenCode Zen', url: 'https://opencode.ai/zen/go/v1' },
];
const baseUrlPresetNames = baseUrlPresets.map((p) => p.name);

function onBaseUrlPreset(i: number): void {
  const p = baseUrlPresets[i];
  if (!p) return;
  draft.baseUrl = p.url;
  markDirty();
}

// ---- model candidates (fetch_models / kimi aliases; see models.rs) ----
const modelCandidates = ref<string[]>([]);
const fetchingModels = ref(false);
const modelFetchMsg = ref('');

/** Context-size input guard: K units, 0 (=256K fallback)…4096. */
function clampContextK(v: number): number {
  const n = Math.round(Number(v) || 0);
  return Math.min(4096, Math.max(0, n));
}

async function refreshModels(): Promise<void> {
  fetchingModels.value = true;
  modelFetchMsg.value = '';
  const out = new Set<string>();
  try {
    if (draft.baseUrl.trim()) {
      try {
        // draft.apiKey holds the MASKED display value (maskKey) unless the
        // user just typed a new one. Masked values must not be sent as the
        // Bearer token — fall back to the store record's plaintext key.
        const rec = selectedId.value ? agents.byId(selectedId.value) : undefined;
        const apiKey = draft.apiKey.includes('****') ? (rec?.apiKey ?? '') : draft.apiKey;
        const ids = await cmd<string[]>('fetch_models', {
          baseUrl: draft.baseUrl.trim(),
          apiKey,
        });
        // OpenCode Go 端点（https://opencode.ai/zen/go/v1）：裸 id 自动补
        // `opencode-go/` 前缀，否则拿到的 deepseek-v4-flash 等无法被引用。
        ids.map((id) => goModelId(draft.baseUrl, id)).forEach((id) => out.add(id));
      } catch (e) {
        modelFetchMsg.value = String(e);
      }
    }
    // Global config.toml aliases are only a fallback: when the agent has its
    // own Base URL, its /models list is the per-agent source of truth and the
    // shared aliases would just pollute every kimi agent's picker.
    if (draft.provider === 'kimi' && !draft.baseUrl.trim()) {
      try {
        const aliases = await cmd<string[]>('kimi_model_aliases');
        aliases.forEach((a) => out.add(a));
      } catch {
        /* missing config.toml is fine */
      }
    }
    modelCandidates.value = [...out].sort();
    if (!modelFetchMsg.value && modelCandidates.value.length === 0) {
      modelFetchMsg.value = draft.baseUrl.trim()
        ? '没有拿到模型列表'
        : '填写 Base URL 后点刷新，或使用本机配置别名';
    }
  } finally {
    fetchingModels.value = false;
  }
}

function onModelPick(i: number): void {
  const m = modelCandidates.value[i];
  if (!m) return;
  draft.model = m;
  markDirty();
}

// ---- allowed thinking-effort levels (multi-select) ----
// 候选来自后端 effort_options（red line C3: no hardcoded lists in the UI）。
// 勾选的档位随保存写入 kimi config.toml 的 support_efforts / opencode.json
// 的 model variants；对话页强度下拉只显示这些。
const effortValues = ref<string[]>([]);
const EFFORT_DISPLAY: Record<string, string> = { low: 'Low', medium: 'Medium', high: 'High', xhigh: 'XHigh', max: 'Max' };
const effortLabels = computed(() => effortValues.value.map((v) => EFFORT_DISPLAY[v] ?? v));

function effortChecked(v: string): boolean {
  return draft.effortOptions.includes(v);
}

function toggleEffort(v: string): void {
  const i = draft.effortOptions.indexOf(v);
  if (i >= 0) draft.effortOptions.splice(i, 1);
  else draft.effortOptions.push(v);
  markDirty();
}

onMounted(() => {
  void cmd<string[]>('effort_options', undefined, []).then((v) => {
    effortValues.value = v;
  });
});

const nameInput = ref<HTMLInputElement | null>(null);

function markDirty(): void {
  dirty.value = true;
}

function loadAgent(a: AgentRecord): void {
  selectedId.value = a.id;
  draft.name = a.name;
  draft.provider = a.provider;
  draft.model = a.model;
  draft.baseUrl = a.baseUrl;
  draft.effortOptions = [...(a.effortOptions ?? [])];
  draft.maxContextK = a.maxContextK || 256;
  draft.supportsImage = a.supportsImage !== false;
  draft.cliPath = a.cliPath;
  draft.piDir = a.piDir;
  draft.apiKey = maskKey(a.apiKey); // display surface: masked only (§9.5)
  draft.extraArgs = a.extraArgs;
  draft.mcpServers = a.mcpServers;
  draft.avatarPath = a.avatarPath;
  dirty.value = false;
  // Bare CLI path → async auto-probe on load (§9.2 trigger #1).
  if (isBareCliPath(spec.value, draft.cliPath)) {
    void nextTick(() => void probe(true));
  }
}

async function selectRow(a: AgentRecord): Promise<void> {
  if (a.id === selectedId.value) return;
  // Switching rows with unsaved edits saves the current one first (§7).
  if (dirty.value) await saveCurrent();
  const fresh = agents.byId(a.id);
  if (fresh) loadAgent(fresh);
}

async function saveCurrent(): Promise<boolean> {
  if (!selectedId.value) return true;
  if (!dirty.value) return true;
  const ok = await agents.save(selectedId.value, {
    name: draft.name,
    provider: draft.provider,
    model: draft.model,
    baseUrl: draft.baseUrl,
    effortOptions: [...draft.effortOptions],
    maxContextK: clampContextK(draft.maxContextK),
    supportsImage: draft.supportsImage,
    cliPath: draft.cliPath,
    piDir: draft.piDir,
    apiKey: draft.apiKey,
    extraArgs: draft.extraArgs,
    mcpServers: draft.mcpServers,
    avatarPath: draft.avatarPath,
  });
  if (ok) {
    dirty.value = false;
    statusMsg.value = agents.lastWarning ? `已保存（${agents.lastWarning}）` : '已保存';
  } else {
    statusMsg.value = agents.lastError || '保存失败';
  }
  return ok;
}

async function createAgent(): Promise<void> {
  if (dirty.value) await saveCurrent();
  try {
    const id = await agents.create('新 Agent');
    const a = agents.byId(id);
    if (a) loadAgent(a);
    statusMsg.value = '已新建 Agent';
    // Name field focused + fully selected (§7).
    void nextTick(() => {
      nameInput.value?.focus();
      nameInput.value?.select();
    });
  } catch (e) {
    statusMsg.value = String(e);
  }
}

async function setDefaultAgent(): Promise<void> {
  if (!selectedId.value) return;
  if (!(await saveCurrent())) return;
  const ok = await agents.setDefault(selectedId.value);
  statusMsg.value = ok ? '已设为默认' : agents.lastError || '设为默认失败';
}

async function deleteAgent(): Promise<void> {
  if (!selectedId.value) return;
  const ok = await agents.remove(selectedId.value);
  if (!ok) {
    statusMsg.value = agents.lastError || '删除失败';
    return;
  }
  // Fall back to the default agent (backend hands the flag to the first
  // remaining one) or clear the selection (§9.3).
  const fallback = agents.byId(agents.defaultAgentId);
  if (fallback) loadAgent(fallback);
  else selectedId.value = '';
  statusMsg.value = '已删除';
}

// ---------------------------------------------------------------------------
// Provider dropdown + CLI auto-probe (spec §9.2)
// ---------------------------------------------------------------------------

function onProviderChange(i: number): void {
  const s = agents.specs[i];
  if (!s || s.id === draft.provider) return;
  draft.provider = s.id;
  markDirty();
  // Bare path for the NEW provider → auto-probe one tick later (§9.2).
  if (isBareCliPath(s, draft.cliPath)) {
    void nextTick(() => void probe(true));
  }
}

/**
 * probe(): async scan. autoFill = the "bare value" mode (load / provider
 * switch / test-connect prep): a found path is written back into the CLI
 * field. Manual 检测 with an explicit path only reports (§9.2).
 * For pi the scan is probe_pi (node + plugin dir), not a CLI probe.
 */
async function probe(autoFill: boolean, preferredPath = ''): Promise<void> {
  const s = agents.specOf(draft.provider);
  if (!s || s.id === 'custom') return; // custom never probes
  if (s.id === 'pi') {
    const preferred = preferredPath || draft.piDir.trim();
    const r = await agents.probePi(preferred);
    if (!r) return;
    if (r.found && r.pluginReady) statusMsg.value = 'Pi 环境就绪';
    else statusMsg.value = r.message || 'Pi 插件未就绪';
    return;
  }
  const providerAtStart = draft.provider;
  const preferred = preferredPath || (isBareCliPath(s, draft.cliPath) ? '' : draft.cliPath);
  const r = await agents.probe(providerAtStart, preferred);
  // Stale-provider guard: the user switched provider while scanning (§9.2).
  if (!r || r.providerId !== draft.provider || draft.provider !== providerAtStart) return;
  if (r.found) {
    if (autoFill && r.path) {
      draft.cliPath = r.path;
      markDirty();
      statusMsg.value = '已自动填入 CLI 路径';
    } else {
      statusMsg.value = '已找到 CLI';
    }
  }
}

/** Probe status line (builtin providers with a selected row only). */
const probeLine = computed<{ text: string; cls: string } | null>(() => {
  const s = spec.value;
  if (!selectedId.value || !s || s.id === 'custom') return null;
  // pi: 编译二进制是否可用 + 插件目录状态。
  if (s.id === 'pi') {
    const r = agents.probeCache['pi'] as unknown as (PiProbeResult & { found?: boolean }) | undefined;
    if (!r) return null;
    if (!r.found) {
      return { text: r.message || 'Pi 二进制未就绪，请先用 bundle-pi.mjs 生成', cls: 'err' };
    }
    if (!r.pluginReady) {
      return { text: r.message || 'Pi 插件未就绪', cls: 'err' };
    }
    return { text: `Pi 就绪 @ ${r.pluginDir}`, cls: 'ok' };
  }
  if (agents.probing[s.id]) return { text: `正在检测 ${s.displayName}…`, cls: '' };
  const r = agents.probeCache[s.id];
  if (!r) return null;
  if (r.found) {
    const ver = r.version ? ` ${r.version}` : '';
    return { text: `已找到 ${s.displayName}${ver} @ ${r.path}`, cls: 'ok' };
  }
  return { text: r.message || s.installHint, cls: 'err' };
});

async function browseCli(): Promise<void> {
  // pi 的目录是插件目录，不是可执行文件；写入 piDir。
  if (spec.value?.id === 'pi') {
    const picked = await openFileDialog({ multiple: false, directory: true });
    if (typeof picked !== 'string' || !picked) return;
    draft.piDir = picked;
    markDirty();
    void probe(false, picked);
    return;
  }
  const picked = await openFileDialog({
    multiple: false,
    filters: [{ name: '程序', extensions: ['exe'] }],
  });
  if (typeof picked !== 'string' || !picked) return;
  draft.cliPath = picked;
  markDirty();
  // Verify the chosen file (old probePath): explicit path → report only.
  void probe(false, picked);
}

// ---------------------------------------------------------------------------
// 如何安装 dialog (kimi only; text from the backend, red line C3)
// ---------------------------------------------------------------------------

const installOpen = ref(false);
const installText = ref('');
const installUrl = ref('');

async function loadInstallHelp(): Promise<void> {
  if (!isTauri) return;
  try {
    const v = await cmd<{ text?: string; url?: string }>('install_help');
    installText.value = v.text ?? '';
    installUrl.value = v.url ?? '';
  } catch (e) {
    console.warn('[config] install_help failed', e);
  }
}

// ---------------------------------------------------------------------------
// 测试连接 (§9.3): success = ACP initialize handshake + 一次真实模型调用
// (session/new + 一词 prompt)，single-flight
// ---------------------------------------------------------------------------

const testing = ref(false);
const testResult = ref<TestAgentResult | null>(null);
const testDetailOpen = ref(false);
const testSelIdx = ref(0);

/** Currently selected transcript frame in the detail dialog. */
const selectedFrame = computed<TestFrame | null>(() => {
  const t = testResult.value?.transcript;
  return t && t[testSelIdx.value] ? t[testSelIdx.value] : (t?.[0] ?? null);
});

function openTestDetail(): void {
  testSelIdx.value = 0;
  testDetailOpen.value = true;
}

/** Copy the currently selected frame's content (pretty-printed). */
const copiedFrame = ref(false);
async function copySelectedFrame(): Promise<void> {
  if (!selectedFrame.value) return;
  copiedFrame.value = await copyText(prettyFrame(selectedFrame.value.text));
  if (copiedFrame.value) setTimeout(() => (copiedFrame.value = false), 1500);
}

async function testConnection(): Promise<void> {
  if (testing.value || !selectedId.value) return;
  testResult.value = null;
  if (!(await saveCurrent())) return;
  const s = agents.specOf(draft.provider);
  if (!s?.chatCapable) {
    statusMsg.value = '该 Provider 暂不支持测试';
    return;
  }
  // Builtin (non-pi) providers with a bare CLI path resolve it first and ask
  // for a second click once the probe has landed (§9.2). pi resolves its
  // binary/plugin dir on the backend instead.
  if (s.id !== 'pi' && isBareCliPath(s, draft.cliPath)) {
    statusMsg.value = '正在解析 CLI 路径，完成后请再点测试连接';
    void probe(true);
    return;
  }
  testing.value = true;
  try {
    const r = await agents.test(selectedId.value);
    if (r === null) return; // another test was already running (ignored)
    testResult.value = r;
    statusMsg.value = r.ok ? '✔ 测试成功：连接与模型调用均通过' : `✘ 测试失败：${r.message}`;
  } finally {
    testing.value = false;
  }
}

/** Pretty-print a captured frame's JSON for display; keep as-is if it isn't JSON. */
function prettyFrame(text: string): string {
  const t = text.trim();
  if (!t) return t;
  try {
    return JSON.stringify(JSON.parse(t), null, 2);
  } catch {
    return text;
  }
}

// ---------------------------------------------------------------------------
// Agent 头像（draft 字段，§9.1）：引用图片路径，不复制。
// ---------------------------------------------------------------------------

const agentAvatarUrl = computed(() =>
  draft.avatarPath ? fileSrc(draft.avatarPath) : BUILTIN_AGENT_AVATAR,
);

async function pickAgentAvatar(): Promise<void> {
  const picked = await openFileDialog({
    multiple: false,
    filters: [{ name: '图片', extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp'] }],
  });
  if (typeof picked !== 'string' || !picked) return;
  // Absolute-path reference, NOT copied into the data dir (§9.1).
  draft.avatarPath = picked;
  markDirty();
  statusMsg.value = '头像已选择，保存后生效';
}

// ---------------------------------------------------------------------------
// tryBack: 未保存的更改 three-way dialog (§6)
// ---------------------------------------------------------------------------

const unsavedOpen = ref(false);

function tryBack(): void {
  if (dirty.value) unsavedOpen.value = true;
  else void nav.goOverlay('hub');
}

async function saveAndBack(): Promise<void> {
  unsavedOpen.value = false;
  if (await saveCurrent()) await nav.goOverlay('hub');
}

function discardAndBack(): void {
  unsavedOpen.value = false;
  dirty.value = false;
  void nav.goOverlay('hub');
}

async function onSaveAndReturn(): Promise<void> {
  if (await saveCurrent()) await nav.goOverlay('hub');
}

// Esc → tryBack (the unsaved dialog capture-stops Esc before this fires).
function onPageKey(e: KeyboardEvent): void {
  if (nav.page !== 'config') return;
  if (e.key === 'Escape') tryBack();
}

// ---------------------------------------------------------------------------
// Page enter: select the default agent (§6)
// ---------------------------------------------------------------------------

async function initPage(): Promise<void> {
  await Promise.all([agents.loadSpecs(), agents.refresh(), prefs.load(), loadInstallHelp()]);
  if (!selectedId.value && agents.agents.length > 0) {
    const def = agents.byId(agents.defaultAgentId) ?? agents.agents[0];
    loadAgent(def);
  }
}

onMounted(() => {
  void initPage();
  window.addEventListener('keydown', onPageKey);
});
onBeforeUnmount(() => window.removeEventListener('keydown', onPageKey));

// Kept-alive page: refresh the list when coming back (agent edits from
// other flows — e.g. rail switch — land via store events anyway, but the
// agents list has no event; a re-pull is cheap and always correct).
watch(
  () => nav.page,
  (p) => {
    if (p !== 'config') return;
    void agents.refresh().then(() => {
      const cur = agents.byId(selectedId.value);
      if (cur) {
        // Re-sync the editor with disk unless the user has unsaved edits.
        if (!dirty.value) loadAgent(cur);
      } else if (agents.agents.length > 0) {
        loadAgent(agents.byId(agents.defaultAgentId) ?? agents.agents[0]);
      } else {
        selectedId.value = '';
      }
    });
  },
);

const pageKeysOn = computed(() => nav.page === 'config');
</script>

<template>
  <PageShell :embed="52">
    <div class="cfg">
      <!-- left: Agent list -->
      <WarFrame
        class="cfg__left"
        src="/assets/ui/frames/frame_iron_panel.png"
        :slice="[96, 110, 69, 108]"
        :hole="[56, 25, 21, 24]"
        :content-left-extra="16"
      >
        <div class="cfg__col">
          <div class="cfg__title war-font-title war-outline-black" :style="{ fontSize: prefs.fs(18) + 'px' }">
            Agent 配置
          </div>
          <div class="cfg__count" :style="{ fontSize: prefs.fs(12) + 'px' }">
            {{ agents.agents.length > 0 ? `共 ${agents.agents.length} 个 · 点击选中编辑` : '暂无 Agent — 点击下方新建' }}
          </div>

          <div class="cfg__list">
            <div
              v-for="(a, i) in agents.agents"
              :key="a.id"
              class="cfg__agent-row"
              :class="{ zebra: i % 2 === 1, selected: a.id === selectedId, 'is-default': a.id === agents.defaultAgentId }"
              @click="selectRow(a)"
            >
              <span class="cfg__agent-star" :style="{ fontSize: prefs.fs(13) + 'px' }">
                {{ a.id === agents.defaultAgentId ? '★' : '·' }}
              </span>
              <div class="cfg__agent-text">
                <div class="cfg__agent-name" :style="{ fontSize: prefs.fs(13) + 'px' }">{{ a.name }}</div>
                <div class="cfg__agent-sub" :style="{ fontSize: prefs.fs(10) + 'px' }">
                  {{ a.provider }}<template v-if="a.id === agents.defaultAgentId"> · 默认</template>
                  <template v-if="!agents.usable(a)">
                    <span class="cfg__agent-unusable"> · 暂不可对话</span>
                  </template>
                </div>
              </div>
            </div>
          </div>

          <div class="cfg__left-foot">
            <WarButton skin="dialog" :width="190" :art-aspect="5.34" text="新建 Agent" @activated="createAgent" />
          </div>
        </div>
      </WarFrame>

      <!-- right top: user settings + agent editor (scrollable) -->
      <WarFrame
        class="cfg__right-top"
        src="/assets/ui/frames/frame_iron_panel.png"
        :slice="[96, 110, 69, 108]"
        :hole="[56, 25, 21, 24]"
      >
        <div class="cfg__scroll">
          <!-- ===== Agent 编辑器 (§9) ===== -->
          <div class="cfg__section-title" :style="{ fontSize: prefs.fs(15) + 'px' }">
            {{ selectedId ? 'Agent 编辑器' : '请先在左侧新建或选择 Agent' }}
          </div>

          <div class="cfg__editor" :class="{ disabled: !selectedId }">
            <div class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">名称</span>
              <input
                ref="nameInput"
                v-model="draft.name"
                class="war-input cfg__input"
                placeholder="例如：工作 Kimi"
                :style="{ fontSize: prefs.fs(13) + 'px' }"
                @input="markDirty"
              />
            </div>

            <div class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">Provider</span>
              <WarDropdown
                class="cfg__dropdown"
                :options="providerOptions"
                :model-value="providerIndex"
                @activated="onProviderChange"
              />
            </div>
            <div v-if="spec?.installHint" class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
              {{ spec.installHint }}
            </div>

            <div class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">Model</span>
              <div class="cfg__baseurl-row">
                <input
                  v-model="draft.model"
                  class="war-input cfg__input"
                  placeholder="模型 id；kimi 可填 config.toml 别名"
                  :style="{ fontSize: prefs.fs(13) + 'px' }"
                  @input="markDirty"
                />
                <WarDropdown
                  v-if="modelCandidates.length > 0"
                  class="cfg__baseurl-presets"
                  :options="modelCandidates"
                  display-text="选择…"
                  @activated="onModelPick"
                />
                <WarButton
                  skin="dialog"
                  :width="80"
                  :art-aspect="5.34"
                  :text="fetchingModels ? '刷新中…' : '刷新'"
                  :enabled="!fetchingModels"
                  @activated="refreshModels"
                />
              </div>
            </div>
            <div v-if="modelFetchMsg" class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
              {{ modelFetchMsg }}
            </div>

            <div v-if="draft.provider === 'kimi' && draft.baseUrl.trim()" class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">上下文长度（K）</span>
              <input
                v-model.number="draft.maxContextK"
                type="number"
                min="0"
                max="4096"
                step="8"
                class="war-input cfg__input"
                :style="{ fontSize: prefs.fs(13) + 'px' }"
                @input="markDirty"
              />
            </div>
            <div v-if="draft.provider === 'kimi' && draft.baseUrl.trim()" class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
               保存时把该模型写入 config.toml（apiKey 明文），上下文长度统一为该值 ×1024，0 = 256K
            </div>

            <div class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">思考强度（多选）</span>
              <div class="cfg__effort-list">
                <label
                  v-for="(v, i) in effortValues"
                  :key="v"
                  class="cfg__effort-item"
                  :style="{ fontSize: prefs.fs(12) + 'px' }"
                >
                  <input type="checkbox" :checked="effortChecked(v)" @change="toggleEffort(v)" />
                  {{ effortLabels[i] }}
                </label>
              </div>
            </div>
            <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
              勾选该 Agent 可用的思考强度档位；对话页强度下拉只显示这些（思考恒开启）。全不勾 = 全部档位可用；kimi 保存时写入 config.toml 的 support_efforts，opencode 生成模型 variants，pi 由驱动注入思考档位下拉
            </div>

            <div class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">多模态（视觉）</span>
              <div class="cfg__effort-list">
                <label class="cfg__effort-item" :style="{ fontSize: prefs.fs(12) + 'px' }">
                  <input type="checkbox" :checked="draft.supportsImage" @change="draft.supportsImage = ($event.target as HTMLInputElement).checked; markDirty()" />
                  该模型可以读图片（截图 / 读取图片文件）
                </label>
              </div>
            </div>
            <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
              只对 Pi 生效：填写了自定义 Base URL 的模型会被写进 ~/.pi/agent/models.json，而 pi 不会自动探测模型能力，缺省按「纯文本」处理——所有图片（粘贴的截图、模型 read 的图片）都会被换成占位符。勾选=声明 input: ["text","image"]；端点确实是纯文本模型时请取消勾选，否则可能报 400
            </div>

            <div v-if="spec?.baseUrlHint" class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
              {{ spec.baseUrlHint }}
            </div>
            <div class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">Base URL（可选）</span>
              <div class="cfg__baseurl-row">
                <input v-model="draft.baseUrl" class="war-input cfg__input" :style="{ fontSize: prefs.fs(13) + 'px' }" @input="markDirty" />
                <WarDropdown
                  class="cfg__baseurl-presets"
                  :options="baseUrlPresetNames"
                  display-text="预置…"
                  @activated="onBaseUrlPreset"
                />
              </div>
            </div>

            <div v-if="draft.provider === 'pi'" class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">Pi 插件目录</span>
              <input
                v-model="draft.piDir"
                class="war-input cfg__input"
                :placeholder="'留空自动定位（workspace 同级 pi 目录 / WARDEX_PI_DIR / 打包内置）'"
                :style="{ fontSize: prefs.fs(13) + 'px' }"
                @input="markDirty"
              />
            </div>
            <div v-else class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">CLI 路径</span>
              <input
                v-model="draft.cliPath"
                class="war-input cfg__input"
                :placeholder="
                  isCustom
                    ? 'CLI 可执行文件完整路径'
                    : draft.provider === 'pi'
                      ? '留空自动定位（workspace 同级 pi 目录或 WARDEX_PI_DIR）'
                      : '留空自动探测'
                "
                :style="{ fontSize: prefs.fs(13) + 'px' }"
                @input="markDirty"
              />
            </div>
            <div class="cfg__btn-row cfg__cli-btns">
              <WarButton
                v-if="!isCustom"
                skin="dialog"
                :width="120"
                :art-aspect="5.34"
                :text="
                  agents.probing[draft.provider]
                    ? '检测中…'
                    : draft.provider === 'pi'
                      ? '检测环境'
                      : '检测 CLI'
                "
                :enabled="!agents.probing[draft.provider]"
                @activated="
                  draft.provider === 'pi'
                    ? probe(false, draft.piDir.trim())
                    : probe(isBareCliPath(spec, draft.cliPath))
                "
              />
              <WarButton skin="dialog" :width="120" :art-aspect="5.34" text="浏览…" @activated="browseCli" />
              <WarButton
                v-if="draft.provider === 'kimi'"
                skin="dialog"
                :width="120"
                :art-aspect="5.34"
                text="如何安装"
                @activated="installOpen = true"
              />
            </div>
            <div
              v-if="probeLine"
              class="cfg__probe-line"
              :class="probeLine.cls"
              :style="{ fontSize: prefs.fs(11) + 'px' }"
            >
              {{ probeLine.text }}
            </div>

            <div class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">API Key</span>
              <input
                v-model="draft.apiKey"
                type="password"
                class="war-input cfg__input"
                :style="{ fontSize: prefs.fs(13) + 'px' }"
                @input="markDirty"
              />
            </div>

            <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
              额外参数（追加在 ACP 启动参数后；custom 时即为完整启动参数）
            </div>
            <div class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">额外参数</span>
              <input v-model="draft.extraArgs" class="war-input cfg__input" :style="{ fontSize: prefs.fs(13) + 'px' }" @input="markDirty" />
            </div>

            <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
              MCP Servers（JSON 数组，建会话时通过 ACP 下发；格式错误将被忽略并记日志）
            </div>
            <textarea
              v-model="draft.mcpServers"
              class="war-input cfg__textarea"
              rows="4"
              placeholder='[{"name":"demo","command":"mcp-server","args":[]}]'
              :style="{ fontSize: prefs.fs(12) + 'px' }"
              @input="markDirty"
            ></textarea>

            <div class="cfg__avatar-row" :class="{ disabled: !selectedId }">
              <img class="cfg__avatar" :src="agentAvatarUrl" draggable="false" />
              <div class="cfg__avatar-side">
                <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
                  AI 头像（当前 Agent，引用图片路径不复制）\n{{ selectedId ? '留空使用内置默认' : '请先在左侧新建或选择 Agent' }}
                </div>
                <div class="cfg__btn-row">
                  <WarButton skin="dialog" :width="130" :art-aspect="5.34" text="选择图片…" :enabled="!!selectedId" @activated="pickAgentAvatar" />
                  <WarButton skin="dialog" :width="130" :art-aspect="5.34" text="重置" :enabled="!!selectedId" @activated="draft.avatarPath = ''; markDirty()" />
                </div>
              </div>
            </div>

            <!-- bottom actions (§9.3) -->
            <div class="cfg__btn-row cfg__editor-actions">
              <WarButton
                skin="dialog"
                :width="130"
                :art-aspect="5.34"
                text="设为默认"
                :enabled="selectedId !== '' && selectedId !== agents.defaultAgentId"
                @activated="setDefaultAgent"
              />
              <WarButton
                skin="dialog"
                :width="130"
                :art-aspect="5.34"
                :text="testing ? '测试中…' : '测试连接'"
                :enabled="selectedId !== '' && !testing"
                @activated="testConnection"
              />
              <WarButton skin="dialog" :width="130" :art-aspect="5.34" text="删除 Agent" :enabled="selectedId !== ''" @activated="deleteAgent" />
            </div>

            <div v-if="statusMsg" class="cfg__status" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ statusMsg }}</div>

            <div v-if="testResult" class="cfg__result-row">
              <WarButton skin="dialog" :width="150" :art-aspect="5.34" text="查看测试结果" @activated="openTestDetail" />
            </div>
          </div>
        </div>
      </WarFrame>

      <!-- right bottom: action bar -->
      <WarFrame
        class="cfg__right-bottom"
        src="/assets/ui/frames/frame_iron_bar.png"
        :slice="[62, 110, 70, 108]"
        :hole="[22, 24, 21, 24]"
      >
        <div class="cfg__actions">
          <WarButton :width="276" text="保存并返回" :shortcut-active="pageKeysOn" @activated="onSaveAndReturn" />
          <WarButton
            :width="276"
            text="返回(B)"
            shortcut-key="B"
            :shortcut-active="pageKeysOn"
            @activated="tryBack"
          />
        </div>
      </WarFrame>
    </div>

    <!-- 未保存的更改 (§6) -->
    <WarDialog v-model:open="unsavedOpen" title-text="未保存的更改" message-text="当前 Agent 配置有未保存的修改。">
      <WarButton skin="dialog" :width="180" :art-aspect="5.34" text="保存并返回" @activated="saveAndBack" />
      <WarButton skin="dialog" :width="150" :art-aspect="5.34" text="丢弃" @activated="discardAndBack" />
      <WarButton skin="dialog" :width="150" :art-aspect="5.34" text="取消" @activated="unsavedOpen = false" />
    </WarDialog>

    <!-- 如何安装 (kimi only, §9.2) -->
    <WarDialog v-model:open="installOpen" title-text="安装 Kimi CLI" :message-text="installText" :dialog-width="640">
      <WarButton skin="dialog" :width="150" :art-aspect="5.34" text="打开链接" @activated="installUrl && openUrl(installUrl)" />
      <WarButton
        skin="dialog"
        :width="150"
        :art-aspect="5.34"
        text="重新检测"
        @activated="installOpen = false; probe(true)"
      />
      <WarButton skin="dialog" :width="150" :art-aspect="5.34" text="关闭" @activated="installOpen = false" />
    </WarDialog>

    <!-- 测试连接结果详情：请求 / 响应 transcript -->
    <WarDialog
      v-model:open="testDetailOpen"
      title-text="测试结果"
      :message-text="testResult?.message ?? ''"
      :dialog-width="860"
      :button-zone-y="0.62"
      :button-zone-h="0.22"
    >
      <template #plate>
        <div class="cfg__detail" :style="{ fontSize: prefs.fs(10) + 'px' }">
          <div class="cfg__detail-tabs">
            <button
              v-for="(f, i) in testResult?.transcript ?? []"
              :key="i"
              type="button"
              class="cfg__tab"
              :class="[f.dir === 'req' ? 'is-req' : 'is-res', { active: i === testSelIdx }]"
              @click="testSelIdx = i"
            >
              {{ f.dir === 'req' ? '请求' : '响应' }} {{ i + 1 }}
            </button>
          </div>
          <div class="cfg__detail-head" v-if="selectedFrame">
            <span class="cfg__trace-dir">{{ selectedFrame.dir === 'req' ? '请求' : '响应' }} {{ testSelIdx + 1 }}</span>
            <button type="button" class="cfg__copy" @click="copySelectedFrame">
              {{ copiedFrame ? '已复制' : '复制' }}
            </button>
          </div>
          <pre v-if="selectedFrame" class="cfg__trace-json">{{ prettyFrame(selectedFrame.text) }}</pre>
          <div v-else class="cfg__detail-empty">（无记录）</div>
        </div>
      </template>
      <WarButton skin="dialog" :width="150" :art-aspect="5.34" text="关闭" @activated="testDetailOpen = false" />
    </WarDialog>
  </PageShell>
</template>

<style scoped>
.cfg {
  display: grid;
  grid-template-columns: 48fr 52fr;
  grid-template-rows: 1fr max(162px, 20%);
  gap: 10px;
  height: 100%;
  padding-top: 4px;
  padding-bottom: 8px;
  box-sizing: border-box;
}

.cfg__left {
  grid-row: 1;
  grid-column: 1;
  min-height: 0;
}

.cfg__right-top {
  grid-row: 1;
  grid-column: 2;
  min-height: 0;
}

.cfg__right-bottom {
  grid-row: 2;
  grid-column: 2;
}

.cfg__col {
  display: flex;
  flex-direction: column;
  gap: 10px;
  height: 100%;
}

.cfg__title {
  color: var(--war-text-dim);
  flex: none;
}

.cfg__count {
  flex: none;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
}

/* ---- agent list ---- */
.cfg__list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.cfg__agent-row {
  flex: none;
  height: 48px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 8px;
  border: 1px solid transparent;
  box-sizing: border-box;
  user-select: none;
}

.cfg__agent-row.zebra {
  background: #14182066;
}

.cfg__agent-row:hover {
  background: var(--war-blue-row);
}

.cfg__agent-row.selected {
  background: #1a3a6e;
}

.cfg__agent-row.is-default {
  border-color: #c9a227;
}

.cfg__agent-star {
  flex: none;
  width: 16px;
  text-align: center;
  color: var(--war-gold);
  font-family: SimSun, serif;
}

.cfg__agent-text {
  flex: 1;
  min-width: 0;
}

.cfg__agent-name {
  color: var(--war-text);
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.cfg__agent-sub {
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.cfg__agent-unusable {
  color: #a06040;
}

.cfg__left-foot {
  flex: none;
  display: flex;
  justify-content: center;
}

/* ---- right form ---- */
.cfg__scroll {
  height: 100%;
  overflow-y: auto;
  scrollbar-width: none;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding-right: 4px;
}

.cfg__section-title {
  flex: none;
  color: var(--war-gold);
  font-weight: bold;
  font-family: SimSun, serif;
}

.cfg__divider {
  flex: none;
  height: 1px;
  background: var(--war-border);
  margin: 4px 0;
}

.cfg__field {
  flex: none;
  display: flex;
  align-items: center;
  gap: 12px;
}

.cfg__check-row {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 8px;
  cursor: pointer;
  color: var(--war-text-light-gold);
}

.cfg__check-text {
  font-family: SimSun, serif;
}

.cfg__label {
  flex: none;
  width: 108px;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  text-align: right;
}

.cfg__input {
  flex: 1;
  height: 30px;
  min-width: 0;
}

.cfg__dropdown {
  width: 180px;
  height: 30px;
}

.cfg__baseurl-row {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.cfg__baseurl-presets {
  flex: none;
  width: 110px;
  height: 30px;
}

.cfg__hint {
  flex: none;
  color: var(--war-text-faint);
  font-family: SimSun, serif;
  white-space: pre-line;
}

.cfg__textarea {
  flex: none;
  resize: none;
  padding: 6px 8px;
  line-height: 1.4;
}

/* ---- allowed thinking-effort multi-select ---- */
.cfg__effort-list {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-wrap: wrap;
  gap: 6px 14px;
}

.cfg__effort-item {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  color: var(--war-text-dim);
  font-family: SimSun, serif;
  cursor: pointer;
  user-select: none;
}

.cfg__effort-item input[type='checkbox'] {
  accent-color: var(--war-gold);
}

.cfg__avatar-row {
  flex: none;
  display: flex;
  align-items: center;
  gap: 12px;
}

.cfg__avatar-row.disabled {
  opacity: 0.45;
  pointer-events: none;
}

.cfg__avatar {
  flex: none;
  width: 56px;
  height: 56px;
  object-fit: cover;
  border: 1px solid var(--war-gold-dim);
  background: #141018;
  box-sizing: border-box;
}

.cfg__avatar-side {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.cfg__btn-row {
  display: flex;
  gap: 10px;
}

.cfg__cli-btns {
  flex: none;
}

.cfg__probe-line {
  flex: none;
  font-family: SimSun, serif;
  color: var(--war-text-muted);
  overflow-wrap: break-word;
}

.cfg__probe-line.ok {
  color: #80f0a0;
}

.cfg__probe-line.err {
  color: var(--war-error);
}

.cfg__editor {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.cfg__editor.disabled {
  opacity: 0.45;
  pointer-events: none;
}

.cfg__editor-actions {
  flex: none;
  margin-top: 6px;
}

.cfg__status {
  flex: none;
  color: var(--war-gold);
  font-family: SimSun, serif;
  white-space: pre-line;
  overflow-wrap: break-word;
}

.cfg__result-row {
  flex: none;
  display: flex;
  align-items: center;
}

.cfg__detail {
  width: 100%;
  max-height: 230px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  box-sizing: border-box;
  overflow: hidden;
}

.cfg__detail-tabs {
  flex: none;
  display: flex;
  gap: 6px;
  overflow-x: auto;
  padding-bottom: 2px;
}

.cfg__tab {
  flex: none;
  padding: 3px 10px;
  border: 1px solid var(--war-glass-border);
  border-radius: 3px;
  background: rgba(0, 0, 0, 0.25);
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  cursor: pointer;
  white-space: nowrap;
}

.cfg__tab.is-req {
  color: var(--war-gold-dim);
}

.cfg__tab.active {
  border-color: var(--war-gold);
  color: var(--war-gold);
  background: rgba(242, 207, 107, 0.12);
}

.cfg__detail-head {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.cfg__copy {
  flex: none;
  padding: 2px 10px;
  border: 1px solid var(--war-glass-border);
  border-radius: 3px;
  background: rgba(0, 0, 0, 0.25);
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  cursor: pointer;
}

.cfg__copy:hover {
  color: var(--war-gold);
  border-color: var(--war-gold-dim);
}

.cfg__detail-empty {
  color: var(--war-text-muted);
  font-family: SimSun, serif;
}

.cfg__trace-json {
  flex: 1;
  min-height: 0;
  margin: 0;
  font-family: Consolas, 'Courier New', monospace;
  font-size: 10px;
  line-height: 1.45;
  color: var(--war-text);
  white-space: pre-wrap;
  word-break: break-all;
  overflow-y: auto;
  border: 1px solid var(--war-glass-border);
  border-radius: 3px;
  padding: 4px 6px;
  background: rgba(0, 0, 0, 0.25);
  box-sizing: border-box;
}

.cfg__actions {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
}
</style>

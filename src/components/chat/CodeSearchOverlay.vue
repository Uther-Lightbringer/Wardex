<script setup lang="ts">
// Project-wide code search overlay: Ctrl+F (code search) and Ctrl+\ (Java
// interface lookup) on the chat page. A floating frame_popup panel with a
// query input, live results (debounced + generation-superseded) and keyboard
// navigation: ↑/↓ move, Enter opens the preview at the hit line, Esc closes
// (capture-stopped so the page-level Esc handler never sees it), Ctrl+F
// refocuses the input.
//
// kind='code'  → search_code with mode/ext/regular-option controls.
// kind='iface' → interface lookup backed by codegraph (external CLI). When
// codegraph is missing the overlay shows an install card (copy command /
// copy prompt / ask-the-AI / re-probe) with a 先用简易搜索 fallback that
// reuses the heuristic text scan.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { cmd } from '../../lib/tauri';
import { copyText } from '../../lib/clipboard';
import { useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';
import WarScrollBar from '../war/WarScrollBar.vue';

interface CodeHit {
  line: number;
  text: string;
}

interface CodeMatch {
  file: string;
  hits: CodeHit[];
}

interface InterfaceHit {
  file: string;
  line: number;
  name: string;
  text: string;
}

interface FlatHit {
  file: string;
  line: number;
  text: string;
  name?: string;
}

const props = withDefaults(defineProps<{ kind?: 'code' | 'iface' }>(), { kind: 'code' });
const emit = defineEmits<{ (e: 'close'): void }>();

const chat = useChatStore();
const prefs = usePrefsStore();

const isIface = computed(() => props.kind === 'iface');

// ---- codegraph integration (kind='iface') ----
interface CodegraphStatus {
  installed: boolean;
  path?: string | null;
  build: 'idle' | 'building' | 'done' | 'error';
  buildError?: string;
  indexExists: boolean;
}

const cgStatus = ref<CodegraphStatus | null>(null);
const cgLoading = ref(false);
const usingFallback = ref(false); // V1 text scan when codegraph is missing
let cgTimer: ReturnType<typeof setInterval> | null = null;

const INSTALL_CMD = 'npm install -g @optave/codegraph';
const INSTALL_PROMPT =
  '请帮我在当前电脑上安装 codegraph CLI（npm 包 @optave/codegraph）。请在终端依次执行：\n' +
  '1. npm install -g @optave/codegraph\n' +
  '2. codegraph --version 确认安装成功（需要 Node.js 22.6 及以上）\n' +
  '安装完成后告诉我结果；如果遇到权限或 Node 版本问题，请一并处理。';

/** Search usable for iface: the V1 fallback is on, or codegraph is installed
 * with a finished build (no stale index). Always true for code kind. */
const ifaceReady = computed(
  () =>
    usingFallback.value ||
    (cgStatus.value?.installed === true &&
      cgStatus.value.build !== 'building' &&
      cgStatus.value.indexExists),
);
/** Searching against the codegraph index (vs the V1 text scan). */
const ifaceUsingCodegraph = computed(
  () => !usingFallback.value && cgStatus.value?.installed === true,
);
const showSearch = computed(() => (isIface.value ? ifaceReady.value : true));

async function loadCgStatus(): Promise<void> {
  if (!isIface.value || !chat.projectDir) return;
  cgLoading.value = true;
  try {
    const s = await cmd<CodegraphStatus>('codegraph_status', { projectDir: chat.projectDir });
    cgStatus.value = s;
    if (s.installed) usingFallback.value = false; // now usable via codegraph
    if (s.installed && s.build === 'building') startCgPoll();
    else stopCgPoll();
  } catch (e) {
    cgStatus.value = null;
    error.value = String(e);
  }
  cgLoading.value = false;
}

function startCgPoll(): void {
  if (cgTimer) return;
  cgTimer = setInterval(async () => {
    try {
      const s = await cmd<CodegraphStatus>('codegraph_status', { projectDir: chat.projectDir });
      cgStatus.value = s;
      if (s.build !== 'building') {
        stopCgPoll();
        if (s.indexExists && query.value.trim()) void run();
      }
    } catch {
      /* transient — keep polling */
    }
  }, 1000);
}

function stopCgPoll(): void {
  if (cgTimer) {
    clearInterval(cgTimer);
    cgTimer = null;
  }
}

async function startBuild(): Promise<void> {
  try {
    await cmd('codegraph_build', { projectDir: chat.projectDir });
    cgStatus.value = { ...(cgStatus.value as CodegraphStatus), build: 'building' };
    startCgPoll();
  } catch (e) {
    error.value = String(e);
  }
}

async function reprobe(): Promise<void> {
  try {
    await cmd<{ installed: boolean }>('codegraph_reprobe');
    await loadCgStatus();
  } catch (e) {
    error.value = String(e);
  }
}

function openPlot(): void {
  void cmd('codegraph_plot', { projectDir: chat.projectDir }).catch((e) => {
    error.value = String(e);
  });
}

function copyTextToClip(s: string): void {
  void copyText(s);
}

async function askAiInstall(): Promise<void> {
  const ok = await chat.newSession();
  if (!ok) {
    error.value = '创建会话失败，请先选择一个项目';
    return;
  }
  await chat.send(INSTALL_PROMPT, []);
  emit('close'); // let the user watch the AI install it
}

const query = ref('');
const flat = ref<FlatHit[]>([]);
const index = ref(0);
const searching = ref(false);
const done = ref(false);
const error = ref('');

// ---- search options (mode chips / extension filter / regex) — code kind ----
// 内容 and 文件名 are independent multi-select chips (both on = match both);
// at least one stays on so the effective mode is never ambiguous.
const modeContent = ref(true);
const modeFilename = ref(false);
const extInput = ref('');
const useRegex = ref(false);

const mode = computed<'content' | 'filename' | 'both'>(() =>
  modeContent.value && modeFilename.value
    ? 'both'
    : modeFilename.value
      ? 'filename'
      : 'content',
);

function toggleContent(): void {
  if (modeContent.value && !modeFilename.value) return; // would turn everything off
  modeContent.value = !modeContent.value;
}

function toggleFilename(): void {
  if (!modeContent.value && modeFilename.value) return; // would turn everything off
  modeFilename.value = !modeFilename.value;
}

// ---- persist the option row across sessions (localStorage) ----
const SEARCH_OPTS_KEY = 'wardex-search-opts';

function loadOpts(): void {
  try {
    const raw = localStorage.getItem(SEARCH_OPTS_KEY);
    if (!raw) return;
    const o = JSON.parse(raw) as { content?: unknown; filename?: unknown; exts?: unknown; regex?: unknown };
    if (typeof o.content === 'boolean') modeContent.value = o.content;
    if (typeof o.filename === 'boolean') modeFilename.value = o.filename;
    if (typeof o.exts === 'string') extInput.value = o.exts;
    if (typeof o.regex === 'boolean') useRegex.value = o.regex;
  } catch {
    /* corrupted storage — keep defaults */
  }
}

function saveOpts(): void {
  try {
    localStorage.setItem(
      SEARCH_OPTS_KEY,
      JSON.stringify({
        content: modeContent.value,
        filename: modeFilename.value,
        exts: extInput.value,
        regex: useRegex.value,
      }),
    );
  } catch {
    /* storage unavailable — skip */
  }
}

watch([modeContent, modeFilename, extInput, useRegex], saveOpts);

const inputEl = ref<HTMLInputElement | null>(null);
const listEl = ref<HTMLDivElement | null>(null);

let seq = 0;
let debounce: ReturnType<typeof setTimeout> | null = null;

const resultCount = (): number => flat.value.length;

function highlightSegments(text: string, q: string): { text: string; hit: boolean }[] {
  const ql = q.trim().toLowerCase();
  if (!ql) return [{ text, hit: false }];
  const out: { text: string; hit: boolean }[] = [];
  let rest = text;
  while (rest) {
    const i = rest.toLowerCase().indexOf(ql);
    if (i < 0) {
      out.push({ text: rest, hit: false });
      break;
    }
    if (i > 0) out.push({ text: rest.slice(0, i), hit: false });
    out.push({ text: rest.slice(i, i + ql.length), hit: true });
    rest = rest.slice(i + ql.length);
  }
  return out;
}

/** "*.java" / ".java" / "java, kt" → ["java", "kt"]. */
function parseExts(): string[] {
  return extInput.value
    .split(/[,，\s]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

function run(): void {
  const q = query.value;
  const mySeq = ++seq;
  if (debounce) clearTimeout(debounce);
  error.value = '';
  // code mode needs text; iface-fallback lists everything on an empty query.
  if (!q.trim() && !isIface.value) {
    flat.value = [];
    done.value = false;
    searching.value = false;
    return;
  }
  searching.value = true;
  done.value = false;
  debounce = setTimeout(async () => {
    try {
      if (isIface.value) {
        if (ifaceUsingCodegraph.value) {
          // codegraph cannot list-all on an empty query — show a hint instead.
          if (!q.trim()) {
            flat.value = [];
            done.value = true;
            searching.value = false;
            return;
          }
          const res = await cmd<InterfaceHit[]>('codegraph_query_interfaces', {
            projectDir: chat.projectDir,
            query: q,
          });
          if (mySeq !== seq) return; // superseded by a newer keystroke
          flat.value = res.map((h) => ({
            file: h.file,
            line: h.line,
            text: h.text,
            name: h.name,
          }));
        } else {
          const res = await cmd<InterfaceHit[]>('search_java_interfaces', {
            root: chat.projectDir,
            query: q,
          });
          if (mySeq !== seq) return; // superseded by a newer keystroke
          flat.value = res.map((h) => ({
            file: h.file,
            line: h.line,
            text: h.text,
            name: h.name,
          }));
        }
      } else {
        const res = await cmd<CodeMatch[]>('search_code', {
          root: chat.projectDir,
          query: q,
          mode: mode.value,
          exts: parseExts(),
          regex: useRegex.value,
        });
        if (mySeq !== seq) return; // superseded by a newer keystroke
        flat.value = res.flatMap((m) =>
          m.hits.map((h) => ({ file: m.file, line: h.line, text: h.text })),
        );
      }
      index.value = 0;
    } catch (e) {
      if (mySeq !== seq) return;
      flat.value = [];
      error.value = String(e);
    } finally {
      if (mySeq === seq) {
        searching.value = false;
        done.value = true;
      }
    }
  }, 150);
}

watch([query, mode, extInput, useRegex, isIface], run);

watch(isIface, (v) => {
  if (v) {
    usingFallback.value = false;
    void loadCgStatus();
  }
});

function openAt(i: number): void {
  const hit = flat.value[i];
  if (!hit) return;
  emit('close');
  // The preview dialog needs an absolute path: projectDir + relative file.
  const abs = chat.projectDir.replace(/[\\/]+$/, '') + '\\' + hit.file.replace(/\//g, '\\');
  chat.openPreview(abs, hit.line);
}

function onKey(e: KeyboardEvent): void {
  if (e.ctrlKey && e.key.toLowerCase() === 'f') {
    e.preventDefault();
    e.stopPropagation();
    inputEl.value?.focus();
    inputEl.value?.select();
    return;
  }
  if (e.key === 'Escape') {
    e.stopPropagation();
    emit('close');
    return;
  }
  const n = resultCount();
  if (n === 0) return;
  if (e.key === 'ArrowDown') {
    e.preventDefault();
    index.value = (index.value + 1) % n;
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    index.value = (index.value - 1 + n) % n;
  } else if (e.key === 'Enter') {
    e.preventDefault();
    openAt(index.value);
  }
}


// Keep the keyboard-selected row visible inside the scrollable list.
watch(index, () => {
  listEl.value?.children[index.value]?.scrollIntoView({ block: 'nearest' });
});

onMounted(() => {
  window.addEventListener('keydown', onKey, true);
  loadOpts();
  if (isIface.value) void loadCgStatus();
  inputEl.value?.focus();
  inputEl.value?.select();
});

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKey, true);
  if (debounce) clearTimeout(debounce);
  stopCgPoll();
  seq++; // invalidate any in-flight search
});
</script>

<template>
  <div class="cs-mask" @mousedown.self="emit('close')">
    <div class="cs">
      <div class="cs__frame"></div>
      <div class="cs__inner">
        <div class="cs__title war-outline-black" :style="{ fontSize: prefs.fs(13) + 'px' }">
          {{ isIface ? '查询接口' : '搜索代码' }}
        </div>

        <!-- iface: codegraph status gates (install / build) -->
        <template v-if="isIface && !showSearch">
          <div class="cs__hint" v-if="!chat.projectDir" :style="{ fontSize: prefs.fs(12) + 'px' }">
            当前会话未绑定项目，无法搜索
          </div>
          <div v-else-if="cgLoading" class="cs__hint" :style="{ fontSize: prefs.fs(12) + 'px' }">
            正在检查 codegraph…
          </div>

          <div v-else-if="!cgStatus?.installed" class="cs-card">
            <div class="cs-card__title" :style="{ fontSize: prefs.fs(14) + 'px' }">
              需要安装 codegraph
            </div>
            <div class="cs-card__body" :style="{ fontSize: prefs.fs(12) + 'px' }">
              接口查询依赖 codegraph（npm 包 @optave/codegraph，需要 Node.js 22.6+）。
              安装后点击「重新检测」，或直接复制提示词让大模型帮你安装。
            </div>
            <div class="cs-card__row">
              <button class="cs-card__btn" @click="copyTextToClip(INSTALL_CMD)">复制安装命令</button>
              <button class="cs-card__btn" @click="copyTextToClip(INSTALL_PROMPT)">复制提示词</button>
              <button class="cs-card__btn" @click="askAiInstall">让 AI 安装</button>
              <button class="cs-card__btn" @click="reprobe">重新检测</button>
            </div>
            <div class="cs-card__link" @click="usingFallback = true">先用简易搜索（无需安装）</div>
          </div>

          <div v-else-if="cgStatus.build === 'building'" class="cs-card">
            <div class="cs-card__title" :style="{ fontSize: prefs.fs(14) + 'px' }">
              正在构建索引…
            </div>
            <div class="cs-card__body" :style="{ fontSize: prefs.fs(12) + 'px' }">
              codegraph 正在分析项目文件，首次构建大项目可能需要数十秒。
              可以关闭此窗口，完成后会自动可用。
            </div>
          </div>

          <div v-else-if="cgStatus.build === 'error'" class="cs-card">
            <div class="cs-card__title cs-card__title--err" :style="{ fontSize: prefs.fs(14) + 'px' }">
              索引构建失败
            </div>
            <div class="cs-card__body" :style="{ fontSize: prefs.fs(12) + 'px' }">
              {{ cgStatus.buildError || '未知错误' }}
            </div>
            <div class="cs-card__row">
              <button class="cs-card__btn on" @click="startBuild">重试</button>
            </div>
          </div>

          <div v-else class="cs-card">
            <div class="cs-card__title" :style="{ fontSize: prefs.fs(14) + 'px' }">
              尚未构建索引
            </div>
            <div class="cs-card__body" :style="{ fontSize: prefs.fs(12) + 'px' }">
              首次使用需要先为当前项目构建 codegraph 索引。
            </div>
            <div class="cs-card__row">
              <button class="cs-card__btn on" @click="startBuild">构建索引</button>
            </div>
          </div>
        </template>

        <!-- search UI (code kind always; iface when ready) -->
        <template v-else>
          <div class="cs__bar">
            <input
              ref="inputEl"
              v-model="query"
              class="cs__input"
              type="text"
              spellcheck="false"
              :placeholder="
                isIface
                  ? ifaceUsingCodegraph
                    ? '输入接口名…'
                    : '输入接口名（留空=列出全部）…'
                  : '输入要搜索的内容…'
              "
              :style="{ fontSize: prefs.fs(13) + 'px' }"
            />
            <span class="cs__meta" :style="{ fontSize: prefs.fs(11) + 'px' }">
              <template v-if="searching">搜索中…</template>
              <template v-else-if="done && !error && resultCount() > 0">{{ resultCount() }} 条结果</template>
            </span>
          </div>

          <!-- iface tools: graph + rebuild (+ fallback toggle) -->
          <div v-if="isIface" class="cs__iface-tools" :style="{ fontSize: prefs.fs(11) + 'px' }">
            <button class="cs-card__btn" @click="openPlot">查看图谱</button>
            <button class="cs-card__btn" @click="startBuild">重建索引</button>
            <span v-if="usingFallback" class="cs-card__link" @click="usingFallback = false"
              >启用 codegraph</span
            >
          </div>

          <!-- search options: mode chips / file-type filter / regex (code only) -->
          <div v-if="!isIface" class="cs__opts" :style="{ fontSize: prefs.fs(11) + 'px' }">
            <span
              class="cs__opt-btn"
              :class="{ on: modeContent }"
              @click="toggleContent"
              >内容</span
            >
            <span
              class="cs__opt-btn"
              :class="{ on: modeFilename }"
              @click="toggleFilename"
              >文件名</span
            >
            <span class="cs__opt-btn" :class="{ on: useRegex }" @click="useRegex = !useRegex"
              >.* 正则</span
            >
            <span class="cs__opt-label">文件类型</span>
            <input
              v-model="extInput"
              class="cs__ext"
              type="text"
              spellcheck="false"
              placeholder="java,kt（留空=全部）"
              :style="{ fontSize: prefs.fs(11) + 'px' }"
            />
          </div>

          <div class="cs__hint" v-if="!chat.projectDir" :style="{ fontSize: prefs.fs(12) + 'px' }">
            当前会话未绑定项目，无法搜索
          </div>

          <div class="cs__scrollwrap" v-if="flat.length > 0">
            <div ref="listEl" class="cs__list">
              <div
                v-for="(hit, i) in flat"
                :key="i"
                class="cs__row"
                :class="{ 'cs__row--sel': i === index }"
                @mousedown.prevent
                @click="openAt(i)"
              >
                <div class="cs__row-top">
                  <span v-if="hit.name" class="cs__iface" :style="{ fontSize: prefs.fs(12) + 'px' }">
                    {{ hit.name }}
                  </span>
                  <span class="cs__file" :style="{ fontSize: prefs.fs(12) + 'px' }">{{ hit.file }}</span>
                  <span v-if="hit.line > 0" class="cs__line" :style="{ fontSize: prefs.fs(11) + 'px' }">
                    {{ hit.line }}
                  </span>
                  <span v-else class="cs__line cs__line--name">文件名</span>
                </div>
                <div
                  v-if="hit.line > 0 && hit.text"
                  class="cs__snip"
                  :style="{ fontSize: prefs.fs(12) + 'px' }"
                >
                  <template v-for="(seg, si) in highlightSegments(hit.text, query)" :key="si">
                    <span v-if="seg.hit" class="cs__hit">{{ seg.text }}</span>
                    <template v-else>{{ seg.text }}</template>
                  </template>
                </div>
              </div>
            </div>
            <div class="cs__warbar">
              <WarScrollBar :target="listEl" />
            </div>
          </div>

          <div
            v-else-if="error"
            class="cs__hint cs__hint--err"
            :style="{ fontSize: prefs.fs(12) + 'px' }"
          >
            {{ error }}
          </div>

          <div
            v-else-if="done && !searching"
            class="cs__hint"
            :style="{ fontSize: prefs.fs(12) + 'px' }"
          >
            {{
              isIface && ifaceUsingCodegraph && !query.trim()
                ? '输入接口名开始搜索'
                : isIface
                  ? '未找到匹配的接口'
                  : '未找到匹配内容'
            }}
          </div>

          <div class="cs__foot" :style="{ fontSize: prefs.fs(11) + 'px' }">
            ↑/↓ 选择 · Enter 预览 · Esc 关闭
          </div>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.cs-mask {
  position: fixed;
  inset: 0;
  z-index: 85;
  background: #00000090;
}

.cs {
  position: absolute;
  left: 50%;
  top: 7vh;
  transform: translateX(-50%);
  width: min(680px, 94vw);
  height: min(560px, 86vh);
}

.cs__frame {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 88px 100px 90px 100px;
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.cs__inner {
  position: absolute;
  inset: 88px 100px 90px 100px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  background: var(--war-glass);
  padding: 10px 12px;
  box-sizing: border-box;
}

.cs__title {
  flex: none;
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-weight: bold;
  text-align: center;
  white-space: nowrap;
  user-select: none;
}

.cs__bar {
  flex: none;
  display: flex;
  align-items: center;
  gap: 10px;
}

.cs__input {
  flex: 1;
  min-width: 0;
  height: 28px;
  padding: 0 8px;
  box-sizing: border-box;
  border: 1px solid #2a3344;
  outline: none;
  background: #0b0d12;
  color: var(--war-text);
  font-family: SimSun, serif;
}

.cs__input:focus {
  border-color: var(--war-gold);
}

.cs__meta {
  flex: none;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  white-space: nowrap;
}

.cs__opts {
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.cs__opt-label {
  flex: none;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
}

.cs__opt-btn {
  flex: none;
  padding: 1px 8px;
  border: 1px solid #2a3344;
  background: #0b0d12;
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  cursor: pointer;
  user-select: none;
}

.cs__opt-btn:hover {
  border-color: var(--war-gold);
  color: var(--war-text);
}

.cs__opt-btn.on {
  border-color: var(--war-gold);
  color: var(--war-gold);
  background: #3a4a6a44;
}

.cs__ext {
  width: 120px;
  height: 20px;
  padding: 0 6px;
  box-sizing: border-box;
  border: 1px solid #2a3344;
  outline: none;
  background: #0b0d12;
  color: var(--war-text);
  font-family: Consolas, monospace;
}

.cs__ext:focus {
  border-color: var(--war-gold);
}

.cs__scrollwrap {
  flex: 1;
  min-height: 0;
  position: relative;
  display: flex;
}

.cs__list {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  scrollbar-width: none; /* native bar hidden — the WC3 WarScrollBar replaces it */
  border: 1px solid #2a3344;
  background: #0b0d12cc;
  padding-right: 30px; /* room for the WC3 scrollbar */
  box-sizing: border-box;
}

.cs__warbar {
  position: absolute;
  top: 0;
  /* a touch inset from the list's right edge (native bars hug it) */
  right: 6px;
  bottom: 0;
  width: 22px;
}

.cs__row {
  padding: 4px 8px;
  cursor: pointer;
  border-bottom: 1px solid #1a2130;
}

.cs__row--sel {
  background: #3a4a6a55;
}

.cs__row-top {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

.cs__file {
  color: var(--war-gold);
  font-family: Consolas, monospace;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.cs__iface {
  flex: none;
  color: #80f0a0;
  font-family: Consolas, monospace;
  font-weight: bold;
  white-space: nowrap;
}

.cs__line {
  flex: none;
  color: var(--war-text-muted);
  font-family: Consolas, monospace;
}

.cs__line--name {
  color: #80f0a0;
}

.cs__snip {
  margin-top: 2px;
  color: var(--war-text);
  font-family: Consolas, monospace;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.cs__hit {
  color: #ffd479;
  background: #8a5a1a66;
}

.cs__hint {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--war-text-faint);
  font-family: SimSun, serif;
}

.cs__hint--err {
  color: #f08080;
}

.cs-card {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 0 16px;
  text-align: center;
}

.cs-card__title {
  color: var(--war-gold);
  font-family: SimSun, serif;
  font-weight: bold;
}

.cs-card__title--err {
  color: #f08080;
}

.cs-card__body {
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  max-width: 440px;
  line-height: 1.6;
}

.cs-card__row {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: center;
}

.cs-card__btn {
  padding: 4px 12px;
  border: 1px solid #2a3344;
  background: #0b0d12;
  color: var(--war-text);
  font-family: SimSun, serif;
  cursor: pointer;
  user-select: none;
}

.cs-card__btn:hover {
  border-color: var(--war-gold);
  color: var(--war-gold);
}

.cs-card__btn.on {
  border-color: var(--war-gold);
  color: var(--war-gold);
  background: #3a4a6a44;
}

.cs-card__link {
  color: var(--war-text-muted);
  font-family: SimSun, serif;
  cursor: pointer;
  text-decoration: underline;
  user-select: none;
}

.cs-card__link:hover {
  color: var(--war-gold-bright);
}

.cs__iface-tools {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
}

.cs__foot {
  flex: none;
  color: var(--war-text-faint);
  font-family: SimSun, serif;
  text-align: center;
  user-select: none;
}
</style>

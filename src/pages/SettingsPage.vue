<script setup lang="ts">
// Settings page (设置): app-level preferences split into
//   共性（两种风格通用）: 我的名字 / 界面字体缩放 / 任务完成通知 / 用户头像 / 界面背景
//   个性（随界面风格）: 界面风格 + 该风格专属设置项（themes.ts settings 注册表）
//     pure: 对话页透明度（下拉 + 手动输入）、页面颜色（预置色板）、背景亮度（下拉 + 手动输入）
//     war : 暂无（未来可加 铁轨/剑形光标 开关等）
// 所有项立即生效 + 落盘（user_prefs.json / localStorage 浏览器模式）。
// The Agent editor lives in Agent 配置 (ConfigPage) instead.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import PageShell from '../components/PageShell.vue';
import WarFrame from '../components/war/WarFrame.vue';
import WarButton from '../components/war/WarButton.vue';
import WarDropdown from '../components/war/WarDropdown.vue';
import { fileSrc, openPath } from '../lib/tauri';
import { useNavStore } from '../stores/nav';
import { CHAT_ALPHA_STEPS, FONT_SCALE_STEPS, PAGE_COLOR_OPTIONS, BG_BRIGHTNESS_STEPS, clampChatAlpha, clampBgBrightness, usePrefsStore } from '../stores/prefs';
import { usePluginsStore } from '../stores/plugins';
import { THEMES, themeOf } from '../lib/themes';

const nav = useNavStore();
const prefs = usePrefsStore();
const plugins = usePluginsStore();

const pageKeysOn = computed(() => nav.page === 'settings');

// Esc → back to main menu.
function onPageKey(e: KeyboardEvent): void {
  if (nav.page !== 'settings') return;
  if (e.key === 'Escape') void nav.goMain();
}
onMounted(() => window.addEventListener('keydown', onPageKey));
onBeforeUnmount(() => window.removeEventListener('keydown', onPageKey));

// ---- 共性：我的名字 ----
const userNameDraft = ref(prefs.userName);
function commitUserName(): void {
  void prefs.setUserName(userNameDraft.value);
  userNameDraft.value = prefs.userName; // show the trimmed/fallback value
}

// ---- 共性：界面字体缩放 (§8.3) ----
const scaleLabels = FONT_SCALE_STEPS.map((s) => `${Math.round(s * 100)}%`);
const scaleIndex = computed(() => {
  const i = FONT_SCALE_STEPS.findIndex((s) => Math.abs(s - prefs.fontScale) < 0.001);
  return i >= 0 ? i : 1;
});
function onScaleChange(i: number): void {
  void prefs.setFontScale(FONT_SCALE_STEPS[i]);
}

// ---- 共性：任务完成通知 ----
function onTaskDoneNotifyChange(e: Event): void {
  void prefs.setTaskDoneNotify((e.target as HTMLInputElement).checked);
}

// ---- 共性：用户头像 ----
const avatarSeq = ref(0);
const BUILTIN_USER_AVATAR = '/assets/ui/avatars/avatar_user_default.png';
const userAvatarUrl = computed(() =>
  prefs.userAvatarPath ? `${fileSrc(prefs.userAvatarPath)}?v=${avatarSeq.value}` : BUILTIN_USER_AVATAR,
);
async function uploadUserAvatar(): Promise<void> {
  const picked = await openFileDialog({
    multiple: false,
    filters: [{ name: '图片', extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp'] }],
  });
  if (typeof picked !== 'string' || !picked) return;
  const ok = await prefs.importUserAvatar(picked);
  avatarSeq.value += 1;
  statusMsg.value = ok ? '头像已更新' : '头像导入失败';
}
async function clearUserAvatar(): Promise<void> {
  await prefs.clearUserAvatar();
  avatarSeq.value += 1;
  statusMsg.value = '已恢复默认头像';
}

// ---- 共性：界面背景 (custom image/video upload) ----
const bgLabel = computed(() => {
  if (prefs.bgType === 'image') return '自定义图片';
  if (prefs.bgType === 'video') return '自定义视频';
  return '默认（内置视频）';
});
const bgThumbUrl = computed(() =>
  prefs.bgType === 'image' ? fileSrc(prefs.bgPath) : '/assets/ui/avatars/avatar_user_default.png',
);
async function uploadBgImage(): Promise<void> {
  const picked = await openFileDialog({
    multiple: false,
    filters: [{ name: '图片', extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp', 'gif'] }],
  });
  if (typeof picked !== 'string' || !picked) return;
  const ok = await prefs.uploadBackground(picked);
  statusMsg.value = ok ? '背景已更新' : '背景导入失败';
}
async function uploadBgVideo(): Promise<void> {
  const picked = await openFileDialog({
    multiple: false,
    filters: [{ name: '视频', extensions: ['mp4', 'webm', 'mov', 'mkv'] }],
  });
  if (typeof picked !== 'string' || !picked) return;
  const ok = await prefs.uploadBackground(picked);
  statusMsg.value = ok ? '背景已更新' : '背景导入失败';
}
async function clearBackground(): Promise<void> {
  await prefs.clearBackground();
  statusMsg.value = '已恢复默认背景';
}

// ---- 个性：界面风格（§8.4, themes.ts）----
const theme = computed(() => themeOf(prefs.uiStyle));
const styleLabels = THEMES.map((t) => t.label);
const styleIndex = computed(() => Math.max(0, THEMES.findIndex((t) => t.id === prefs.uiStyle)));
function onUiStyleChange(i: number): void {
  const t = THEMES[i];
  if (t) void prefs.setUiStyle(t.id);
}

// ---- 个性 · pure：对话页透明度（下拉 6 档 + 手动输入 0.5~1.0）----
const CHAT_ALPHA_CUSTOM = 6; // 档位之外的“自定义”行
const chatAlphaLabels = [...CHAT_ALPHA_STEPS.map((a) => `${Math.round(a * 100)}%`), '自定义'];
const chatAlphaIndex = computed(() => {
  const i = CHAT_ALPHA_STEPS.findIndex((a) => Math.abs(a - prefs.chatAlpha) < 0.001);
  return i >= 0 ? i : CHAT_ALPHA_CUSTOM;
});
function onChatAlphaChange(i: number): void {
  const a = CHAT_ALPHA_STEPS[i];
  if (a != null) void prefs.setChatAlpha(a);
  else alphaInputFocus.value = true; // 选“自定义”→ 聚焦手动输入框
}
const alphaInputFocus = ref(false);
const alphaInputEl = ref<HTMLInputElement | null>(null);
const alphaInputText = ref('');
watch(alphaInputFocus, async (f) => {
  if (f) {
    await nextTick();
    alphaInputEl.value?.focus();
    alphaInputEl.value?.select();
    alphaInputFocus.value = false;
  }
});
// 输入框同步显示当前值（百分数，如 0.77 → 77）
watchAlphaFromPrefs();
function watchAlphaFromPrefs(): void {
  alphaInputText.value = String(Math.round(prefs.chatAlpha * 100));
}
function commitChatAlphaInput(): void {
  const n = Number(alphaInputText.value);
  if (!Number.isFinite(n)) {
    watchAlphaFromPrefs();
    return;
  }
  const v = clampChatAlpha(n / 100);
  void prefs.setChatAlpha(v);
  alphaInputText.value = String(Math.round(v * 100));
}

// ---- 个性 · pure：页面颜色（预置色板）----
const pageColorIndex = computed(() =>
  Math.max(0, PAGE_COLOR_OPTIONS.findIndex((c) => c.hex === prefs.pageColor)),
);
function onPickPageColor(i: number): void {
  const c = PAGE_COLOR_OPTIONS[i];
  if (c) void prefs.setPageColor(c.hex);
}

// ---- 个性 · pure：背景亮度（下拉 5 档 + 手动输入 0.5~1.5，作用于背景图/视频）----
const BG_BRIGHTNESS_CUSTOM = 5; // 档位之外的“自定义”行
const bgBrightnessLabels = [...BG_BRIGHTNESS_STEPS.map((v) => `${Math.round(v * 100)}%`), '自定义'];
const bgBrightnessIndex = computed(() => {
  const i = BG_BRIGHTNESS_STEPS.findIndex((v) => Math.abs(v - prefs.bgBrightness) < 0.001);
  return i >= 0 ? i : BG_BRIGHTNESS_CUSTOM;
});
function onBgBrightnessChange(i: number): void {
  const v = BG_BRIGHTNESS_STEPS[i];
  if (v != null) void prefs.setBgBrightness(v);
  else bgBrightnessFocus.value = true; // 选“自定义”→ 聚焦手动输入框
}
const bgBrightnessFocus = ref(false);
const bgBrightnessEl = ref<HTMLInputElement | null>(null);
const bgBrightnessText = ref('');
watch(bgBrightnessFocus, async (f) => {
  if (f) {
    await nextTick();
    bgBrightnessEl.value?.focus();
    bgBrightnessEl.value?.select();
    bgBrightnessFocus.value = false;
  }
});
// 输入框同步显示当前值（百分数，如 0.75 → 75）
function watchBgBrightnessFromPrefs(): void {
  bgBrightnessText.value = String(Math.round(prefs.bgBrightness * 100));
}
watchBgBrightnessFromPrefs();
function commitBgBrightnessInput(): void {
  const n = Number(bgBrightnessText.value);
  if (!Number.isFinite(n)) {
    watchBgBrightnessFromPrefs();
    return;
  }
  const v = clampBgBrightness(n / 100);
  void prefs.setBgBrightness(v);
  bgBrightnessText.value = String(Math.round(v * 100));
}

// ---- 状态提示 ----
const statusMsg = ref('');

// ---- 插件（插件化改造 P0）：列表/开关/删除/扫描/生效 ----
onMounted(() => {
  if (!plugins.loaded) void plugins.load();
});
function kindLabel(kind: string): string {
  if (kind === 'ui') return '界面';
  if (kind === 'tool+ui') return '工具+界面';
  return '工具';
}
async function onPluginToggle(p: { id: string }, e: Event): Promise<void> {
  await plugins.toggle(p.id, (e.target as HTMLInputElement).checked);
}
async function onPluginDelete(id: string): Promise<void> {
  try {
    await plugins.remove(id);
    statusMsg.value = `已删除插件 ${id}`;
  } catch (e) {
    statusMsg.value = String(e);
  }
}
async function onRescan(): Promise<void> {
  await plugins.rescan();
  statusMsg.value = `扫描完成，共 ${plugins.list.length} 个插件`;
}
async function onOpenDir(): Promise<void> {
  const dir = await plugins.rootDir();
  if (dir) openPath(dir);
}
async function onApply(): Promise<void> {
  const r = await plugins.apply();
  await plugins.load(); // UI 面板随生效结果刷新
  statusMsg.value = r.skipped > 0
    ? `已重启 ${r.restarted} 个会话，${r.skipped} 个忙碌会话已跳过（稍后可再点生效）`
    : `已应用到 ${r.restarted} 个运行中的会话`;
}
</script>

<template>
  <PageShell :embed="52">
    <div class="set">
      <WarFrame
        class="set__panel"
        src="/assets/ui/frames/frame_iron_panel.png"
        :slice="[96, 110, 69, 108]"
        :hole="[56, 25, 21, 24]"
      >
        <div class="set__scroll">
          <div class="set__head">
            <div class="set__title war-font-title war-outline-black" :style="{ fontSize: prefs.fs(18) + 'px' }">设置</div>
            <div class="set__sub" :style="{ fontSize: prefs.fs(11) + 'px' }">所有更改立即生效 + 落盘；Agent 的 Provider/模型等在「Agent 配置」里编辑</div>
          </div>

          <!-- ===== 共性：两种风格通用 ===== -->
          <div class="cfg__section-title" :style="{ fontSize: prefs.fs(15) + 'px' }">共性</div>

          <div class="cfg__field">
            <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">我的名字</span>
            <input
              v-model="userNameDraft"
              class="war-input cfg__input"
              placeholder="阿尔萨斯"
              maxlength="24"
              :style="{ fontSize: prefs.fs(13) + 'px' }"
              @change="commitUserName"
              @keydown.enter.prevent="commitUserName"
            />
          </div>
          <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
            对话页用户气泡显示此名字，留空则默认「阿尔萨斯」
          </div>

          <div class="cfg__field">
            <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">界面字体缩放</span>
            <WarDropdown class="cfg__dropdown" :options="scaleLabels" :model-value="scaleIndex" @activated="onScaleChange" />
          </div>
          <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
            立即生效，作用于聊天气泡、输入框、会话列表等主要阅读区
          </div>

          <div class="cfg__field">
            <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">任务完成通知</span>
            <label class="cfg__check-row">
              <input type="checkbox" :checked="prefs.taskDoneNotify" @change="onTaskDoneNotifyChange" />
              <span class="cfg__check-text" :style="{ fontSize: prefs.fs(13) + 'px' }">后台任务完成时弹系统通知</span>
            </label>
          </div>
          <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
            如战场监控的步兵跑完任务；应用窗口聚焦时不打扰，仅在你没看界面时提醒
          </div>

          <div class="cfg__avatar-row">
            <img class="cfg__avatar" :src="userAvatarUrl" draggable="false" />
            <div class="cfg__avatar-side">
              <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
                用户头像：对话页用户气泡使用此头像\n未上传时使用默认金发肖像
              </div>
              <div class="cfg__btn-row">
                <WarButton skin="dialog" :width="130" :art-aspect="5.34" text="上传…" @activated="uploadUserAvatar" />
                <WarButton skin="dialog" :width="130" :art-aspect="5.34" text="恢复默认" @activated="clearUserAvatar" />
              </div>
            </div>
          </div>

          <div class="cfg__avatar-row">
            <img class="cfg__avatar" :src="bgThumbUrl" draggable="false" />
            <div class="cfg__avatar-side">
              <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
                界面背景：{{ bgLabel }}（自定义文件复制到数据目录）\n支持 jpg/png/webp/gif 与 mp4/webm 视频；位于最底层，页面颜色/半透明表面在它之上
              </div>
              <div class="cfg__btn-row">
                <WarButton skin="dialog" :width="130" :art-aspect="5.34" text="上传图片…" @activated="uploadBgImage" />
                <WarButton skin="dialog" :width="130" :art-aspect="5.34" text="上传视频…" @activated="uploadBgVideo" />
                <WarButton skin="dialog" :width="130" :art-aspect="5.34" text="恢复默认" :enabled="prefs.hasCustomBackground" @activated="clearBackground" />
              </div>
            </div>
          </div>

          <div class="cfg__divider"></div>

          <!-- ===== 个性：随界面风格（themes.ts settings 注册表） ===== -->
          <div class="cfg__section-title" :style="{ fontSize: prefs.fs(15) + 'px' }">个性</div>

          <div class="cfg__field">
            <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">界面风格</span>
            <WarDropdown class="cfg__dropdown" :options="styleLabels" :model-value="styleIndex" @activated="onUiStyleChange" />
          </div>
          <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
            立即生效 + 落盘。纯净风格为浅色半透明 CSS 渲染（无铁轨/贴图/剑形光标）；背景默认纯白，仍可上传自定义背景视频或图片
          </div>

          <!-- 纯净风格专属：对话页透明度 -->
          <template v-if="theme.settings.includes('chatAlpha')">
            <div class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">对话页透明度</span>
              <WarDropdown
                class="cfg__dropdown"
                :options="chatAlphaLabels"
                :model-value="chatAlphaIndex"
                @activated="onChatAlphaChange"
              />
              <input
                v-model="alphaInputText"
                ref="alphaInputEl"
                type="number"
                min="50"
                max="100"
                step="1"
                class="war-input set__alpha-input"
                :style="{ fontSize: prefs.fs(13) + 'px' }"
                @change="commitChatAlphaInput"
              />
              <span class="set__alpha-unit" :style="{ fontSize: prefs.fs(13) + 'px' }">%</span>
            </div>
            <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
              纯净风格下对话页内容（主卡/气泡/输入区）的半透明程度：50% 最透、100% 不透明。数值越低越能看到背景图/视频；魔兽风格不受影响
            </div>
          </template>

          <!-- 纯净风格专属：页面颜色（预置色板） -->
          <template v-if="theme.settings.includes('pageColor')">
            <div class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">页面颜色</span>
              <div class="set__swatches">
                <button
                  v-for="(c, i) in PAGE_COLOR_OPTIONS"
                  :key="c.hex"
                  type="button"
                  class="set__swatch"
                  :class="{ selected: i === pageColorIndex }"
                  :style="{ background: c.hex }"
                  :title="c.label"
                  @click="onPickPageColor(i)"
                />
              </div>
              <span class="set__swatch-label" :style="{ fontSize: prefs.fs(12) + 'px' }">
                {{ PAGE_COLOR_OPTIONS[pageColorIndex]?.label ?? '' }}
              </span>
            </div>
            <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
              纯净风格下对话页卡片、菜单铁框、监控面板等表面层的底色；背景图/视频在最底层，半透明表面会透出它
            </div>
          </template>

          <!-- 纯净风格专属：背景亮度（下拉 + 手动输入，作用于背景图/视频） -->
          <template v-if="theme.settings.includes('bgBrightness')">
            <div class="cfg__field">
              <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">背景亮度</span>
              <WarDropdown
                class="cfg__dropdown"
                :options="bgBrightnessLabels"
                :model-value="bgBrightnessIndex"
                @activated="onBgBrightnessChange"
              />
              <input
                v-model="bgBrightnessText"
                ref="bgBrightnessEl"
                type="number"
                min="50"
                max="150"
                step="1"
                class="war-input set__alpha-input"
                :style="{ fontSize: prefs.fs(13) + 'px' }"
                @change="commitBgBrightnessInput"
              />
              <span class="set__alpha-unit" :style="{ fontSize: prefs.fs(13) + 'px' }">%</span>
            </div>
            <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
              纯净风格下背景图/视频的明暗：50% 最暗、150% 最亮。表面层半透明时尤其明显，可让背景更衬文字；魔兽风格不受影响
            </div>
          </template>

          <!-- 魔兽风格暂无专属项 -->
          <template v-if="theme.settings.length === 0">
            <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
              魔兽风格暂无可配置的专属项（贴图风保持原汁原味）；后续可扩展铁轨/剑形光标开关等
            </div>
          </template>

          <div class="cfg__divider"></div>

          <!-- ===== 插件（工具 + 界面面板） ===== -->
          <div class="cfg__section-title" :style="{ fontSize: prefs.fs(15) + 'px' }">插件</div>
          <div class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
            工具类插件注入给 AI（pi extension），界面类插件在对话页右侧栏新增面板。\n也可以直接在对话里让 AI 帮你新建/修改插件；改动后点「生效」应用
          </div>
          <div v-for="p in plugins.list" :key="p.id" class="cfg__field plug-row">
            <span class="cfg__label" :style="{ fontSize: prefs.fs(13) + 'px' }">{{ p.name }}</span>
            <span class="plug-row__meta" :style="{ fontSize: prefs.fs(11) + 'px' }">
              {{ kindLabel(p.kind) }} · v{{ p.version }}<template v-if="p.builtin"> · 内置</template>
            </span>
            <label class="cfg__check-row plug-row__toggle">
              <input
                type="checkbox"
                :checked="p.enabled"
                :disabled="p.id === 'plugins'"
                @change="onPluginToggle(p, $event)"
              />
              <span class="cfg__check-text" :style="{ fontSize: prefs.fs(12) + 'px' }">启用</span>
            </label>
            <WarButton
              v-if="!p.builtin"
              skin="dialog"
              :width="86"
              :art-aspect="5.34"
              text="删除"
              @activated="onPluginDelete(p.id)"
            />
          </div>
          <div v-if="plugins.list.length === 0" class="cfg__hint" :style="{ fontSize: prefs.fs(11) + 'px' }">
            尚未发现任何插件，点「扫描」试试
          </div>
          <div class="cfg__btn-row set__actions--inline">
            <WarButton skin="dialog" :width="130" :art-aspect="5.34" text="扫描" @activated="onRescan" />
            <WarButton skin="dialog" :width="130" :art-aspect="5.34" text="打开目录…" @activated="onOpenDir" />
            <WarButton
              skin="blue"
              :width="130"
              text="生效"
              :enabled="!plugins.applying"
              @activated="onApply"
            />
          </div>

          <div class="cfg__divider"></div>

          <div v-if="statusMsg" class="set__status" :style="{ fontSize: prefs.fs(11) + 'px' }">{{ statusMsg }}</div>

          <div class="set__actions">
            <WarButton :width="276" text="返回(B)" shortcut-key="B" :shortcut-active="pageKeysOn" @activated="nav.goMain()" />
          </div>
        </div>
      </WarFrame>
    </div>
  </PageShell>
</template>

<style scoped>
.set {
  display: grid;
  grid-template-columns: 1fr;
  grid-template-rows: 1fr;
  height: 100%;
  padding-top: 4px;
  padding-bottom: 8px;
  box-sizing: border-box;
}

.set__panel {
  grid-row: 1;
  grid-column: 1;
  min-height: 0;
}

.set__scroll {
  height: 100%;
  overflow-y: auto;
  scrollbar-width: none;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding-right: 4px;
}

.set__head {
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.set__title {
  color: var(--war-gold);
  font-weight: bold;
}

.set__sub {
  color: var(--war-text-dim);
}

.set__status {
  flex: none;
  color: var(--war-gold);
}

.set__actions {
  flex: none;
  display: flex;
  justify-content: center;
  gap: 12px;
  padding-top: 4px;
}

.set__alpha-input {
  flex: none;
  width: 64px;
  text-align: right;
}

.set__alpha-unit {
  flex: none;
  color: var(--war-text-dim);
}

.set__swatches {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 8px;
}

.set__swatch {
  flex: none;
  width: 30px;
  height: 30px;
  border-radius: 6px;
  border: 1px solid var(--war-panel-border, #d4dae2);
  box-sizing: border-box;
  cursor: pointer;
}

.set__swatch.selected {
  border: 2px solid var(--war-gold, #a9882f);
  box-shadow: 0 0 0 1px var(--war-panel-border, #d4dae2);
}

.set__swatch-label {
  flex: none;
  color: var(--war-text-dim);
}

/* ---- 复用的配置行样式（与 ConfigPage 一致） ---- */
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

.cfg__label {
  flex: none;
  color: var(--war-text);
}

.cfg__hint {
  flex: none;
  color: var(--war-text-dim);
  line-height: 1.5;
}

.cfg__dropdown {
  flex: none;
}

.cfg__input {
  flex: 1;
  min-width: 0;
}

.cfg__check-row {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 8px;
}

.cfg__check-text {
  color: var(--war-text);
}

.cfg__avatar-row {
  flex: none;
  display: flex;
  align-items: center;
  gap: 12px;
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
  gap: 6px;
}

.cfg__btn-row {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

/* 插件列表行：名称 + 元信息 + 启用开关 + 删除 */
.plug-row {
  align-items: center;
}

.plug-row__meta {
  flex: 1;
  min-width: 0;
  color: var(--war-text-dim);
}

.plug-row__toggle {
  flex: none;
  margin-right: 4px;
}

.set__actions--inline {
  padding: 6px 0 2px;
}
</style>

<script setup lang="ts">
// WC3-style folder browser ("打开项目", FolderBrowserDialog.qml).
// Composition: title plate + path bar with drive dropdown + folder list in
// the frame_popup nine-slice + dialog-skin button row. Keyboard navigation:
// ↑/↓ select, Enter enter/choose, Backspace go up, Esc close.
//
// DATA SOURCE: the Rust folder-browser commands (store/browse.rs —
// FolderBrowserModel equivalent): folder_drives / folder_list (directories
// only, one level) / folder_create (inline 新建文件夹).
import { computed, nextTick, ref, watch } from 'vue';
import WarDropdown from './war/WarDropdown.vue';
import WarButton from './war/WarButton.vue';
import { cmd } from '../lib/tauri';

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{
  (e: 'update:open', v: boolean): void;
  (e: 'folderChosen', path: string): void;
}>();

// ---- folder browser backend (folder_* commands) ----
interface FolderEntry {
  name: string;
  path: string;
}

const drives = ref<string[]>(['C:\\']);

/** parent_of (browse.rs): C:\a\b → C:\a, C:\a → C:\, root → null. */
function parentOf(dir: string): string | null {
  const d = dir.trim().replace(/[\\/]+$/, '');
  if (!/^[A-Za-z]:/.test(d) || d.length < 3) return null;
  const rest = d.slice(2);
  if (!rest) return null;
  const i = rest.lastIndexOf('\\');
  return i < 0 ? d.slice(0, 1) + ':\\' : d.slice(0, 1) + ':' + rest.slice(0, i);
}

// ---- state ----
const currentDrive = ref('C:\\');
const currentPath = ref('C:\\');
const entries = ref<FolderEntry[]>([]);
const selectedRow = ref(-1);
const creating = ref(false);
const createError = ref('');
const newName = ref('新建文件夹');
const listEl = ref<HTMLElement | null>(null);
const createInput = ref<HTMLInputElement | null>(null);

const canGoUp = computed(() => parentOf(currentPath.value) !== null);

async function refresh(): Promise<void> {
  entries.value = await cmd<FolderEntry[]>('folder_list', { dir: currentPath.value }, []);
  selectedRow.value = -1;
  creating.value = false;
  createError.value = '';
}

function setDrive(d: string): void {
  currentDrive.value = d;
  currentPath.value = d;
  void refresh();
}

function enter(row: number): void {
  const e = entries.value[row];
  if (!e) return;
  selectedRow.value = -1;
  currentPath.value = e.path;
  void refresh();
}

function goUp(): void {
  const p = parentOf(currentPath.value);
  if (!p) return;
  selectedRow.value = -1;
  currentPath.value = p;
  void refresh();
}

// ---- inline folder creation ----
function startCreate(): void {
  createError.value = '';
  creating.value = true;
  newName.value = '新建文件夹';
  void nextTick(() => {
    createInput.value?.focus();
    createInput.value?.select();
  });
}

function cancelCreate(): void {
  creating.value = false;
  createError.value = '';
  listEl.value?.focus();
}

async function confirmCreate(): Promise<void> {
  try {
    const r = await cmd<FolderEntry>('folder_create', { dir: currentPath.value, name: newName.value });
    creating.value = false;
    createError.value = '';
    await refresh();
    selectedRow.value = entries.value.findIndex((e) => e.path === r.path);
    listEl.value?.focus();
  } catch (e) {
    createError.value = String(e);
    createInput.value?.focus();
  }
}

function currentTarget(): string {
  if (selectedRow.value >= 0) {
    const p = entries.value[selectedRow.value]?.path;
    if (p) return p;
  }
  return currentPath.value;
}

function choose(): void {
  const p = currentTarget();
  if (!p) return;
  emit('update:open', false);
  emit('folderChosen', p);
}

function close(): void {
  emit('update:open', false);
}

// ---- keyboard navigation ----
function onListKey(e: KeyboardEvent): void {
  if (e.key === 'ArrowDown') {
    selectedRow.value = Math.min(entries.value.length - 1, selectedRow.value + 1);
    e.preventDefault();
  } else if (e.key === 'ArrowUp') {
    selectedRow.value = Math.max(0, selectedRow.value - 1);
    e.preventDefault();
  } else if (e.key === 'Enter') {
    if (selectedRow.value >= 0) enter(selectedRow.value);
    else choose();
    e.preventDefault();
  } else if (e.key === 'Backspace') {
    goUp();
    e.preventDefault();
  }
}

function onDialogKey(e: KeyboardEvent): void {
  if (e.key === 'Escape' && !creating.value) close();
}

watch(
  () => props.open,
  (v) => {
    if (!v) return;
    void (async () => {
      drives.value = await cmd<string[]>('folder_drives', undefined, drives.value);
      // Resync the drive dropdown with the remembered path.
      const driveOfPath = /^[A-Za-z]:/.test(currentPath.value)
        ? currentPath.value.slice(0, 1).toUpperCase() + ':\\'
        : '';
      if (driveOfPath && drives.value.includes(driveOfPath)) {
        currentDrive.value = driveOfPath;
      } else if (!drives.value.includes(currentDrive.value)) {
        currentDrive.value = drives.value[0] ?? 'C:\\';
        currentPath.value = currentDrive.value;
      }
      await refresh();
      void nextTick(() => listEl.value?.focus());
    })();
  },
);
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="fb-mask" @keydown="onDialogKey">
      <div class="fb">
        <!-- title plate -->
        <div class="fb__title-plate">
          <span class="fb__title war-outline-gold">打 开 项 目</span>
        </div>

        <!-- path bar + drive dropdown -->
        <div class="fb__pathbar">
          <div class="fb__pathbar-frame"></div>
          <span class="fb__path">{{ currentPath }}</span>
          <WarDropdown
            class="fb__drive"
            :options="drives"
            :model-value="drives.indexOf(currentDrive)"
            @activated="(i: number) => setDrive(drives[i])"
          />
        </div>

        <!-- create error row -->
        <div v-if="createError" class="fb__error">{{ createError }}</div>

        <!-- folder list -->
        <div class="fb__list-frame">
          <div class="fb__list-iron"></div>
          <div ref="listEl" class="fb__list" tabindex="0" @keydown="onListKey">
            <!-- inline new-folder row -->
            <div v-if="creating" class="fb__row fb__row--creating">
              <img src="/assets/wc3_extracted/ui/icon-folder.png" class="fb__icon" draggable="false" />
              <input
                ref="createInput"
                v-model="newName"
                class="war-inline-input fb__create-input"
                @input="createError = ''"
                @keydown.enter.prevent="confirmCreate"
                @keydown.esc.stop="cancelCreate"
                @click.stop
              />
            </div>

            <!-- ".." header row -->
            <div v-if="canGoUp" class="fb__row fb__row--up" @click="selectedRow = -1" @dblclick="goUp">
              <img src="/assets/wc3_extracted/ui/icon-folder-up.png" class="fb__icon" draggable="false" />
              <span class="fb__up-text">..（上一层）</span>
            </div>

            <div
              v-for="(e, i) in entries"
              :key="e.path"
              class="fb__row"
              :class="{ selected: selectedRow === i }"
              @click="selectedRow = i"
              @dblclick="enter(i)"
            >
              <span v-if="selectedRow === i" class="fb__row-glow"></span>
              <img src="/assets/wc3_extracted/ui/icon-folder.png" class="fb__icon" draggable="false" />
              <span class="fb__name">{{ e.name }}</span>
            </div>

            <div v-if="entries.length === 0 && !creating" class="fb__empty">（空目录）</div>
          </div>
        </div>

        <!-- buttons -->
        <div class="fb__buttons">
          <WarButton
            skin="dialog"
            :width="190"
            :art-aspect="5.34"
            text="新建文件夹"
            @activated="creating ? cancelCreate() : startCreate()"
          />
          <WarButton skin="dialog" :width="190" :art-aspect="5.34" text="打开此目录" @activated="choose" />
          <WarButton skin="dialog" :width="190" :art-aspect="5.34" text="取消" @activated="close" />
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.fb-mask {
  position: fixed;
  inset: 0;
  z-index: 100;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.fb {
  width: min(680px, 85vw);
  height: min(560px, 90vh);
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.fb__title-plate {
  align-self: center;
  height: 44px;
  padding: 0 36px;
  display: flex;
  align-items: center;
  border: 14px 16px 13px 20px solid transparent;
  border-image: url('/assets/ui/dropdown/dropdown_panel.png') 14 16 13 20 stretch;
  box-sizing: border-box;
}

.fb__title {
  color: var(--war-gold);
  font-size: 22px;
  font-weight: bold;
  font-family: SimSun, serif;
  letter-spacing: 4px;
}

.fb__pathbar {
  position: relative;
  height: 40px;
  flex: none;
  display: flex;
  align-items: center;
  padding: 0 12px 0 18px;
  gap: 8px;
  box-sizing: border-box;
}

.fb__pathbar-frame {
  position: absolute;
  inset: 0;
  border: 14px 16px 13px 20px solid transparent;
  border-image: url('/assets/ui/dropdown/dropdown_panel.png') 14 16 13 20 stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.fb__path {
  position: relative;
  flex: 1;
  color: var(--war-text);
  font-size: 14px;
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  direction: rtl;
  text-align: left;
}

.fb__drive {
  position: relative;
  width: 88px;
  height: 30px;
  flex: none;
}

.fb__error {
  align-self: center;
  color: var(--war-error);
  font-size: 12px;
  font-family: SimSun, serif;
  flex: none;
}

.fb__list-frame {
  position: relative;
  flex: 1;
  min-height: 0;
}

.fb__list-iron {
  position: absolute;
  inset: 0;
  border: 88px 100px 90px 100px solid transparent; /* T R B L (frame_popup) */
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 stretch;
  box-sizing: border-box;
  pointer-events: none;
}

.fb__list {
  position: absolute;
  /* gold rim inner edge L56 T40 R59 B42 + breathing gap */
  inset: 44px 64px 46px 60px;
  overflow-y: auto;
  scrollbar-width: none;
  outline: none;
}

.fb__row {
  position: relative;
  height: 34px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding-left: 8px;
  box-sizing: border-box;
}

.fb__row:hover {
  background: #32509640;
}

.fb__row--creating {
  background: #32509633;
}

.fb__row-glow {
  position: absolute;
  inset: 0;
  background: url('/assets/wc3_extracted/ui/GlueScreen-Button-KeyboardHighlight.png') 0 0 / 100% 100% no-repeat;
  mix-blend-mode: screen;
  pointer-events: none;
}

.fb__icon {
  width: 20px;
  height: 16px;
  flex: none;
}

.fb__name {
  color: var(--war-text);
  font-size: 15px;
  font-family: SimSun, serif;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.fb__row.selected .fb__name {
  color: var(--war-gold);
  font-weight: bold;
}

.fb__up-text {
  color: #9aa2b2;
  font-size: 15px;
  font-family: SimSun, serif;
}

.fb__create-input {
  flex: 1;
  height: 26px;
  font-size: 15px;
}

.fb__empty {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--war-text-faint);
  font-size: 14px;
  font-family: SimSun, serif;
  pointer-events: none;
}

.fb__buttons {
  flex: none;
  display: flex;
  justify-content: center;
  gap: 16px;
}
</style>

<script setup lang="ts">
// Full query workspace (opened from the chat dock drawer). A large modal over
// the chat page: connection tabs + schema tree + AI 生成 SQL + SQL editor +
// result grid. AI SQL goes through the current chat session's agent.
import { computed, onMounted, onBeforeUnmount, ref } from 'vue';
import { useDbStore } from '../../stores/db';
import WarFrame from '../war/WarFrame.vue';
import WarDialog from '../war/WarDialog.vue';
import WarButton from '../war/WarButton.vue';
import DbSchemaTree from './DbSchemaTree.vue';
import DbResultGrid from './DbResultGrid.vue';

const db = useDbStore();

const sqlEl = ref<HTMLElement | null>(null);
const gutterLines = ref<string[]>([]);
const writeOpen = ref(false);
const execOpen = ref(false);

const statusClass = computed(() =>
  db.statusKind === 'ok' ? 'db-q__status-ok' : db.statusKind === 'err' ? 'db-q__status-err' : '',
);

function onKey(e: KeyboardEvent): void {
  if (!db.dialogOpen) return;
  if (e.altKey && e.key === 'Enter') {
    e.preventDefault();
    void db.execute(false);
    return;
  }
  if (e.key === 'Escape') {
    if (writeOpen.value || execOpen.value) return;
    e.stopPropagation();
    db.dialogOpen = false;
  }
}
onMounted(() => window.addEventListener('keydown', onKey, true));
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true));

function onSqlInput(): void {
  gutterLines.value = db.sql.split('\n');
}

async function onModeClick(): Promise<void> {
  if (db.mode === 'ro') {
    writeOpen.value = true;
  } else {
    db.toggleMode();
    db.setStatus('已切回只读模式', 'ok');
  }
}

function confirmWrite(): void {
  writeOpen.value = false;
  db.toggleMode();
  db.setStatus('已开启写操作，写语句执行前需逐次确认', 'err');
}

async function onRun(): Promise<void> {
  const outcome = await db.execute(false);
  if (outcome === 'needConfirm') execOpen.value = true;
}

async function confirmExec(): Promise<void> {
  execOpen.value = false;
  await db.execute(true);
}

async function onAiSubmit(): Promise<void> {
  await db.requestAi();
}

function onAiKey(e: KeyboardEvent): void {
  if (e.key === 'Enter' && !e.ctrlKey) {
    e.preventDefault();
    void onAiSubmit();
  }
}

function selectConn(name: string): void {
  db.activeConn = name;
  db.tree = [];
  db.cols = {};
  void db.openConn(name).then(() => db.fetchTree());
}
</script>

<template>
  <Teleport to="body">
    <div v-if="db.dialogOpen" class="db-q__mask" @keydown.stop @mousedown.self="db.dialogOpen = false">
      <div class="db-q__pv">
        <div class="db-q__pv-frame"></div>
        <div class="db-q__inner">
          <div class="db-q">
          <!-- top bar -->
          <div class="db-q__bar">
            <span class="db-q__proj">◆ 数据库 · <span class="db-q__proj-name">{{ db.projectDir.split(/[\\/]/).filter(Boolean).pop() || db.projectDir }}</span></span>
            <div class="db-q__tabs">
              <div
                v-for="c in db.connections"
                :key="c.name"
                class="db-q__tab"
                :class="{ on: c.name === db.activeConn }"
                @click="selectConn(c.name)"
              >
                <span class="db-q__dot" :class="{ open: db.isOpen(c.name) }"></span>
                {{ c.name }}
                <span class="db-q__tab-x" @click.stop="db.closeConn(c.name)">×</span>
              </div>
              <button class="db-q__add" @click="db.connDialogOpen = true">＋ 新建连接</button>
            </div>
            <div
              class="db-q__mode"
              :class="db.mode"
              @click="onModeClick"
              :title="db.mode === 'ro' ? '当前只读，点击开启写操作' : '当前可写，点击切回只读'"
            >
              <span class="db-q__mode-dot"></span>{{ db.mode === 'ro' ? '只读' : '读写' }}
            </div>
            <button class="db-q__close" @click="db.dialogOpen = false">×</button>
          </div>

          <!-- body -->
          <div class="db-q__body">
            <WarFrame
              class="db-q__panel"
              src="/assets/ui/frames/frame_fat_bar.png"
              :slice="[23, 26, 22, 25]"
            >
              <DbSchemaTree />
            </WarFrame>

            <div class="db-q__right">
              <!-- AI row -->
              <div class="db-q__ai">
                <span class="db-q__ai-label">AI 生成 SQL</span>
                <input
                  v-model="db.aiPrompt"
                  class="db-q__ai-input"
                  placeholder="用自然语言描述需求，如：查测试环境订单金额前10的订单"
                  spellcheck="false"
                  @keydown="onAiKey"
                />
                <button class="db-q__btn" :disabled="db.aiBusy" @click="onAiSubmit">
                  {{ db.aiBusy ? '生成中…' : '生成' }}
                </button>
                <span class="db-q__ai-status">{{ db.aiStatus }}</span>
              </div>

              <!-- editor -->
              <WarFrame
                class="db-q__panel db-q__panel--grow"
                src="/assets/ui/frames/frame_fat_bar.png"
                :slice="[23, 26, 22, 25]"
              >
                <div class="db-q__ed">
                  <div class="db-q__ed-head">
                    <span class="db-q__ed-title">SQL 编辑器</span>
                    <span class="db-q__spacer"></span>
                    <button class="db-q__btn" @click="db.sql = db.sql.replace(/\b(select|from|where|limit|order by|join|on|and|or|as|group by)\b/gi, (m) => m.toUpperCase())">格式化</button>
                    <button class="db-q__btn db-q__btn--run" @click="onRun">执行 (Alt+Enter)</button>
                  </div>
                  <div class="db-q__ed-box">
                    <div class="db-q__gutter"><div v-for="(l, i) in gutterLines" :key="i">{{ i + 1 }}</div></div>
                    <textarea
                      ref="sqlEl"
                      v-model="db.sql"
                      class="db-q__sql"
                      spellcheck="false"
                      @input="onSqlInput"
                    ></textarea>
                  </div>
                  <div class="db-q__status">
                    <span class="db-q__status-mode" :class="db.mode">● {{ db.mode === 'ro' ? '只读模式' : '读写模式' }}</span>
                    <span :class="statusClass">{{ db.statusText }}</span>
                    <span class="db-q__spacer"></span>
                  </div>
                </div>
              </WarFrame>

              <!-- results -->
              <WarFrame
                class="db-q__panel db-q__panel--grow"
                src="/assets/ui/frames/frame_fat_bar.png"
                :slice="[23, 26, 22, 25]"
              >
                <div class="db-q__res">
                  <div class="db-q__res-tabs">
                    <div
                      v-for="(r, i) in db.results"
                      :key="i"
                      class="db-q__res-tab"
                      :class="{ on: i === db.activeRes }"
                      @click="db.activeRes = i"
                    >
                      结果 {{ i + 1 }}/{{ db.results.length }}
                    </div>
                    <span v-if="db.results.length === 0" class="db-q__res-hint">执行后在此显示结果</span>
                  </div>
                  <div class="db-q__res-grid">
                    <DbResultGrid v-if="db.results[db.activeRes]" :result="db.results[db.activeRes].res" />
                  </div>
                </div>
              </WarFrame>
            </div>
          </div>

          <!-- footer -->
          <div class="db-q__foot">
            <span class="db-q__foot-title">数据库查询</span>
            <span class="db-q__spacer"></span>
            <button class="db-q__btn" @click="db.dialogOpen = false">取消</button>
            <button class="db-q__btn db-q__btn--run" @click="db.dialogOpen = false">关闭</button>
          </div>
          </div>
        </div>
      </div>
    </div>

    <!-- write-mode confirm -->
    <WarDialog
      :open="writeOpen"
      title-text="开启写操作"
      :message-text="'写模式将对当前项目所有已打开连接生效。\nINSERT / UPDATE / DELETE / DDL 将可执行，每次写语句执行前仍需逐次确认。'"
      :dialog-width="600"
      @update:open="writeOpen = $event"
    >
      <WarButton skin="dialog" :width="190" text="取消" @activated="writeOpen = false" />
      <WarButton skin="dialog" :width="190" text="确认开启" @activated="confirmWrite" />
    </WarDialog>

    <!-- write execution confirm -->
    <WarDialog
      :open="execOpen"
      title-text="执行确认"
      message-text="批内包含写 / DDL 语句，将执行以上 SQL。"
      :dialog-width="640"
      @update:open="execOpen = $event"
    >
      <template #plate>
        <div class="db-q__exec-plate">
          <div v-for="(s, i) in db.execConfirm?.statements ?? []" :key="i" class="db-q__exec-sql">{{ s }}</div>
        </div>
      </template>
      <WarButton skin="dialog" :width="150" text="取消" @activated="execOpen = false" />
      <WarButton skin="dialog" :width="190" text="确认执行" @activated="confirmExec" />
    </WarDialog>
  </Teleport>
</template>

<style scoped>
.db-q__mask {
  position: fixed;
  inset: 0;
  z-index: 100;
  background: #000000c0;
  display: flex;
  align-items: center;
  justify-content: center;
}

/* popup material (frame_popup.png), same recipe as FilePreviewDialog's .pv */
.db-q__pv {
  position: relative;
  width: 94vw;
  height: 94vh;
}
.db-q__pv-frame {
  position: absolute;
  inset: 0;
  border-style: solid;
  border-color: transparent;
  border-width: 88px 100px 90px 100px; /* T R B L (slice 88/100/90/100) */
  border-image: url('/assets/ui/frames/frame_popup.png') 88 100 90 100 fill stretch;
  box-sizing: border-box;
  pointer-events: none;
}
.db-q__inner {
  position: absolute;
  inset: 88px 100px 90px 100px;
  display: flex;
  flex-direction: column;
  background: var(--war-glass);
  padding: 6px;
  box-sizing: border-box;
}

.db-q {
  display: flex;
  flex-direction: column;
  gap: 8px;
  height: 100%;
  box-sizing: border-box;
}

/* top bar */
.db-q__bar {
  flex: none;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 4px 8px;
}
.db-q__proj {
  color: var(--war-gold);
  font-size: 14px;
  font-weight: bold;
  text-shadow:
    -1px 0 var(--war-outline-brown), 1px 0 var(--war-outline-brown),
    0 -1px var(--war-outline-brown), 0 1px var(--war-outline-brown);
  white-space: nowrap;
}
.db-q__proj-name {
  color: var(--war-gold-bright);
}
.db-q__tabs {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  overflow-x: auto;
  scrollbar-width: none;
}
.db-q__tab {
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 10px;
  background: #10141fe6;
  border: 1px solid #2a3344;
  font-size: 12px;
  color: var(--war-text-dim);
  cursor: pointer;
}
.db-q__tab.on {
  border-color: var(--war-gold-input);
  color: var(--war-gold);
  background: #1a2033f2;
}
.db-q__dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #3a4456;
}
.db-q__dot.open {
  background: var(--war-green);
  box-shadow: 0 0 6px var(--war-green);
}
.db-q__tab-x {
  color: var(--war-text-faint);
  margin-left: 2px;
}
.db-q__tab-x:hover {
  color: var(--war-error);
}
.db-q__add {
  flex: none;
  background: #10141fe6;
  border: 1px dashed #2a3344;
  color: var(--war-text-muted);
  font-size: 12px;
  padding: 5px 10px;
  cursor: pointer;
}
.db-q__add:hover {
  color: var(--war-gold);
  border-color: var(--war-gold-input);
}
.db-q__mode {
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 10px;
  background: #10141fcc;
  border: 1px solid #2a3344;
  font-size: 12px;
  font-weight: bold;
  cursor: pointer;
}
.db-q__mode.ro .db-q__mode-dot {
  background: var(--war-green);
  box-shadow: 0 0 6px var(--war-green);
}
.db-q__mode.rw .db-q__mode-dot {
  background: var(--war-red);
  box-shadow: 0 0 6px var(--war-red);
}
.db-q__mode.ro {
  color: var(--war-green);
}
.db-q__mode.rw {
  color: var(--war-red);
}
.db-q__mode-dot {
  width: 9px;
  height: 9px;
  border-radius: 50%;
}
.db-q__close {
  flex: none;
  width: 26px;
  height: 26px;
  background: #10141fcc;
  border: 1px solid #2a3344;
  color: var(--war-text-muted);
  font-size: 15px;
  cursor: pointer;
}
.db-q__close:hover {
  color: var(--war-error);
  border-color: var(--war-error);
}

/* body */
.db-q__body {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: 240px 1fr;
  gap: 8px;
}
.db-q__panel {
  min-height: 0;
}
.db-q__right {
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

/* AI row */
.db-q__ai {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 2px 4px;
}
.db-q__ai-label {
  color: var(--war-gold);
  font-size: 12px;
  font-weight: bold;
  white-space: nowrap;
  text-shadow:
    -1px 0 var(--war-outline-brown), 1px 0 var(--war-outline-brown),
    0 -1px var(--war-outline-brown), 0 1px var(--war-outline-brown);
}
.db-q__ai-input {
  flex: 1;
  min-width: 0;
  background: #15192299;
  border: 1px solid #2a3344;
  color: var(--war-text);
  font-family: SimSun, serif;
  font-size: 12px;
  padding: 7px 10px;
  outline: none;
}
.db-q__ai-input:focus {
  border-color: var(--war-gold-input);
}
.db-q__ai-input::placeholder {
  color: var(--war-text-faint);
}
.db-q__ai-status {
  flex: none;
  color: var(--war-text-muted);
  font-size: 11px;
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* buttons */
.db-q__btn {
  flex: none;
  background: #15192299;
  border: 1px solid #2a3344;
  color: var(--war-text-dim);
  font-size: 12px;
  padding: 5px 12px;
  cursor: pointer;
}
.db-q__btn:hover {
  color: var(--war-gold);
  border-color: var(--war-gold-input);
}
.db-q__btn:disabled {
  opacity: 0.5;
  pointer-events: none;
}
.db-q__btn--run {
  color: var(--war-gold);
  border-color: var(--war-gold-input);
}

/* editor */
.db-q__ed {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: 6px 8px;
  box-sizing: border-box;
}
.db-q__ed-head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  padding-bottom: 6px;
}
.db-q__ed-title {
  color: var(--war-text-muted);
  font-size: 12px;
}
.db-q__spacer {
  flex: 1;
}
.db-q__ed-box {
  flex: 1;
  min-height: 0;
  display: flex;
  background: #0c101ccc;
  border: 1px solid #2a3344;
}
.db-q__gutter {
  flex: none;
  width: 42px;
  overflow: hidden;
  background: #10141faa;
  border-right: 1px solid #1c2333;
  padding-top: 6px;
  text-align: right;
  color: var(--war-text-faint);
  font: 12px/18px Consolas, monospace;
}
.db-q__sql {
  flex: 1;
  min-width: 0;
  background: transparent;
  border: none;
  outline: none;
  resize: none;
  color: var(--war-text);
  font: 12px/18px Consolas, monospace;
  padding: 6px 8px;
}
.db-q__status {
  flex: none;
  display: flex;
  align-items: center;
  gap: 12px;
  padding-top: 6px;
  color: var(--war-text-muted);
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
}
.db-q__status-mode.ro {
  color: var(--war-green);
}
.db-q__status-mode.rw {
  color: var(--war-red);
}
.db-q__status-ok {
  color: var(--war-green);
}
.db-q__status-err {
  color: var(--war-error);
}

/* results */
.db-q__res {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: 4px 8px;
  box-sizing: border-box;
}
.db-q__res-tabs {
  flex: none;
  display: flex;
  align-items: center;
  gap: 4px;
  padding-bottom: 6px;
  overflow-x: auto;
  scrollbar-width: none;
}
.db-q__res-tab {
  flex: none;
  padding: 3px 12px;
  background: #10141fcc;
  border: 1px solid #2a3344;
  color: var(--war-text-muted);
  font-size: 12px;
  cursor: pointer;
}
.db-q__res-tab.on {
  color: var(--war-gold);
  border-color: var(--war-gold-input);
  background: #1a2033f2;
}
.db-q__res-hint {
  color: var(--war-text-faint);
  font-size: 12px;
}
.db-q__res-grid {
  flex: 1;
  min-height: 0;
}

/* footer */
.db-q__foot {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 4px;
}
.db-q__foot-title {
  color: var(--war-gold);
  font-size: 13px;
  font-weight: bold;
  text-shadow:
    -1px 0 var(--war-outline-brown), 1px 0 var(--war-outline-brown),
    0 -1px var(--war-outline-brown), 0 1px var(--war-outline-brown);
}

/* exec plate */
.db-q__exec-plate {
  width: 100%;
  max-height: 120px;
  overflow-y: auto;
  text-align: left;
}
.db-q__exec-sql {
  font-family: Consolas, monospace;
  font-size: 12px;
  color: var(--war-text);
  background: #0c101ccc;
  border: 1px solid #2a3344;
  padding: 6px 8px;
  margin-bottom: 4px;
  white-space: pre-wrap;
  word-break: break-all;
}
</style>

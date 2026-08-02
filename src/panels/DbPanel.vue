<script setup lang="ts">
// 数据库 dock drawer panel (registry 'db'): project-bound connection list +
// schema tree + AI 生成 SQL mini input + "展开完整查询窗口" button.
import { computed, ref, watch } from 'vue';
import { useDbStore } from '../stores/db';
import { useChatStore } from '../stores/chat';
import WarScrollBar from '../components/war/WarScrollBar.vue';
import DbQueryDialog from '../components/db/DbQueryDialog.vue';
import DbConnectionDialog from '../components/db/DbConnectionDialog.vue';

const db = useDbStore();
const chat = useChatStore();

const projectDir = computed(() => chat.meta?.projectDir || chat.projectDir);

watch(
  projectDir,
  (dir) => {
    if (dir) void db.load(dir);
  },
  { immediate: true },
);

const listEl = ref<HTMLElement | null>(null);

const treeRows = computed(() => {
  const f = db.filter.trim().toLowerCase();
  const out: { schema: string; table: string; depth: number; kind: 'schema' | 'table' }[] = [];
  for (const g of db.tree) {
    const tables = f
      ? g.tables.filter(
          (t) =>
            t.name.toLowerCase().includes(f) ||
            t.comment.toLowerCase().includes(f) ||
            (db.aliases[`${g.schema}.${t.name}`] ?? '').toLowerCase().includes(f),
        )
      : g.tables;
    if (tables.length === 0 && f) continue;
    out.push({ schema: g.schema, table: '', depth: 0, kind: 'schema' });
    if (f || db.expanded[g.schema]) {
      for (const t of tables) {
        out.push({ schema: g.schema, table: t.name, depth: 1, kind: 'table' });
      }
    }
  }
  return out;
});

async function pickConn(name: string): Promise<void> {
  db.activeConn = name;
  if (!db.isOpen(name)) {
    const ok = await db.openConn(name);
    if (!ok) return;
  }
  await db.fetchTree();
}

function toggleSchema(schema: string): void {
  db.expanded = { ...db.expanded, [schema]: !db.expanded[schema] };
}

async function onAi(): Promise<void> {
  await db.requestAi();
}

function onAiKey(e: KeyboardEvent): void {
  if (e.key === 'Enter' && !e.ctrlKey) {
    e.preventDefault();
    void onAi();
  }
}
</script>

<template>
  <div class="db-panel">
    <div class="db-panel__title">数据库</div>
    <div v-if="db.connections.length === 0" class="db-panel__empty">
      本项目尚未配置数据库连接，点击下方「＋ 新建连接」。
    </div>

    <div v-else class="db-panel__conns">
      <div
        v-for="c in db.connections"
        :key="c.name"
        class="db-panel__conn"
        :class="{ on: c.name === db.activeConn }"
        @click="pickConn(c.name)"
      >
        <span class="db-panel__dot" :class="{ open: db.isOpen(c.name) }"></span>
        <span class="db-panel__conn-name">{{ c.name }}</span>
      </div>
    </div>

    <button class="db-panel__add" @click="db.connDialogOpen = true">＋ 新建连接</button>

    <div class="db-panel__filter">
      <input v-model="db.filter" placeholder="过滤表" spellcheck="false" />
    </div>

    <div class="db-panel__scroll" ref="listEl">
      <div
        v-for="(r, i) in treeRows"
        :key="r.kind + r.schema + r.table + i"
        class="db-panel__row"
        :class="'db-panel__' + r.kind"
        :style="{ paddingLeft: r.depth * 14 + 6 + 'px' }"
        @click="
          r.kind === 'schema'
            ? toggleSchema(r.schema)
            : db.insertSelect(r.table)
        "
      >
        <span class="db-panel__tw">
          {{ r.kind === 'schema' ? (db.expanded[r.schema] ? '▾' : '▸') : '·' }}
        </span>
        <template v-if="r.kind === 'schema'">
          <span class="db-panel__schema">{{ r.schema }}</span>
        </template>
        <template v-else>
          <span class="db-panel__table">{{ r.table }}</span>
        </template>
      </div>
      <div v-if="db.activeConn && treeRows.length === 0" class="db-panel__empty">没有匹配的表</div>
    </div>
    <WarScrollBar :target="listEl" :scale="0.7" />

    <div class="db-panel__ai">
      <input
        v-model="db.aiPrompt"
        placeholder="用自然语言查数据…"
        spellcheck="false"
        @keydown="onAiKey"
      />
      <button :disabled="db.aiBusy" @click="onAi">{{ db.aiBusy ? '生成中…' : 'AI 生成 SQL' }}</button>
      <span class="db-panel__ai-status">{{ db.aiStatus }}</span>
    </div>

    <button class="db-panel__expand" @click="db.dialogOpen = true">展开完整查询窗口 →</button>

    <DbQueryDialog />
    <DbConnectionDialog />
  </div>
</template>

<style scoped>
.db-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: 8px 6px;
  box-sizing: border-box;
  gap: 6px;
}

.db-panel__title {
  color: var(--war-gold);
  font-size: 13px;
  font-weight: bold;
  text-shadow:
    -1px 0 var(--war-outline-brown), 1px 0 var(--war-outline-brown),
    0 -1px var(--war-outline-brown), 0 1px var(--war-outline-brown);
}

.db-panel__empty {
  color: var(--war-text-faint);
  font-size: 11px;
  padding: 6px 2px;
}

.db-panel__conns {
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.db-panel__conn {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 8px;
  background: #10141fe6;
  border: 1px solid #2a3344;
  font-size: 12px;
  color: var(--war-text-dim);
  cursor: pointer;
}
.db-panel__conn.on {
  border-color: var(--war-gold-input);
  color: var(--war-gold);
}
.db-panel__dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #3a4456;
  flex: none;
}
.db-panel__dot.open {
  background: var(--war-green);
  box-shadow: 0 0 6px var(--war-green);
}

.db-panel__add {
  background: #10141fe6;
  border: 1px dashed #2a3344;
  color: var(--war-text-muted);
  font-size: 11px;
  padding: 4px 8px;
  cursor: pointer;
}
.db-panel__add:hover {
  color: var(--war-gold);
  border-color: var(--war-gold-input);
}

.db-panel__filter {
  display: flex;
  background: #15192299;
  border: 1px solid #2a3344;
}
.db-panel__filter input {
  flex: 1;
  min-width: 0;
  background: transparent;
  border: none;
  outline: none;
  color: var(--war-text);
  font-family: SimSun, serif;
  font-size: 11px;
  padding: 4px 6px;
}
.db-panel__filter input::placeholder {
  color: var(--war-text-faint);
}

.db-panel__scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
}
.db-panel__row {
  line-height: 18px;
  font-size: 11px;
  color: var(--war-text-dim);
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.db-panel__row:hover {
  color: var(--war-gold-bright);
  background: #1a2033aa;
}
.db-panel__tw {
  display: inline-block;
  width: 12px;
  text-align: center;
  color: var(--war-gold-dim);
}
.db-panel__schema {
  color: var(--war-gold);
  font-weight: bold;
}
.db-panel__table {
  color: var(--war-text);
}

.db-panel__ai {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.db-panel__ai input {
  width: 100%;
  background: #15192299;
  border: 1px solid #2a3344;
  color: var(--war-text);
  font-family: SimSun, serif;
  font-size: 11px;
  padding: 5px 7px;
  outline: none;
  box-sizing: border-box;
}
.db-panel__ai input:focus {
  border-color: var(--war-gold-input);
}
.db-panel__ai input::placeholder {
  color: var(--war-text-faint);
}
.db-panel__ai button {
  background: #15192299;
  border: 1px solid var(--war-gold-input);
  color: var(--war-gold);
  font-size: 11px;
  padding: 4px 8px;
  cursor: pointer;
}
.db-panel__ai button:disabled {
  opacity: 0.5;
  pointer-events: none;
}
.db-panel__ai-status {
  color: var(--war-text-muted);
  font-size: 10px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.db-panel__expand {
  background: #15192299;
  border: 1px solid var(--war-gold-input);
  color: var(--war-gold);
  font-size: 12px;
  padding: 5px 8px;
  cursor: pointer;
  flex: none;
}
.db-panel__expand:hover {
  color: var(--war-gold-bright);
}
</style>

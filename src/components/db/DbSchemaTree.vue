<script setup lang="ts">
// Schema tree (PRD M2): schema → table → column, lazy column loading,
// filter box (name/comment/alias), right-click table menu.
import { computed, onMounted, onBeforeUnmount, ref } from 'vue';
import { useDbStore, type ColumnMeta } from '../../stores/db';
import WarScrollBar from '../war/WarScrollBar.vue';

const db = useDbStore();

const listEl = ref<HTMLElement | null>(null);

const rows = computed(() => {
  const f = db.filter.trim().toLowerCase();
  const out: { schema: string; table: string; comment: string; col?: ColumnMeta; depth: number; kind: 'schema' | 'table' | 'col' }[] = [];
  for (const g of db.tree) {
    let tables = g.tables;
    if (f) {
      tables = g.tables.filter(
        (t) =>
          t.name.toLowerCase().includes(f) ||
          t.comment.toLowerCase().includes(f) ||
          (db.aliases[`${g.schema}.${t.name}`] ?? '').toLowerCase().includes(f),
      );
      if (tables.length === 0) continue;
    }
    out.push({ schema: g.schema, table: '', comment: '', depth: 0, kind: 'schema' });
    if (f || db.expanded[g.schema]) {
      for (const t of tables) {
        out.push({ schema: g.schema, table: t.name, comment: t.comment, depth: 1, kind: 'table' });
        const cols = db.cols[`${g.schema}.${t.name}`];
        if ((f || db.expanded[`${g.schema}.${t.name}`]) && cols) {
          for (const c of cols) {
            out.push({ schema: g.schema, table: t.name, comment: '', col: c, depth: 2, kind: 'col' });
          }
        }
      }
    }
  }
  return out;
});

function toggleSchema(schema: string): void {
  db.expanded = { ...db.expanded, [schema]: !db.expanded[schema] };
}

async function toggleTable(schema: string, table: string): Promise<void> {
  const key = `${schema}.${table}`;
  const next = !db.expanded[key];
  db.expanded = { ...db.expanded, [key]: next };
  if (next && !db.cols[key]) {
    await db.fetchColumns(key);
  }
}

function aliasKey(schema: string, table: string, col?: string): string {
  return col ? `${schema}.${table}.${col}` : `${schema}.${table}`;
}

function fullName(schema: string, table: string): string {
  return `${schema}.${table}`;
}

function onTableClick(schema: string, table: string): void {
  db.insertSelect(table);
}

function onColClick(key: string): void {
  navigator.clipboard?.writeText(key).catch(() => {});
  db.setStatus(`已复制列名：${key}`, 'ok');
}

// ---- context menu (table) ----
const ctx = ref<{ visible: boolean; x: number; y: number; schema: string; table: string }>({
  visible: false,
  x: 0,
  y: 0,
  schema: '',
  table: '',
});

function onTableContext(e: MouseEvent, schema: string, table: string): void {
  e.preventDefault();
  ctx.value = { visible: true, x: e.clientX, y: e.clientY, schema, table };
}
onMounted(() => document.addEventListener('click', closeCtx));
onBeforeUnmount(() => document.removeEventListener('click', closeCtx));
function closeCtx(): void {
  ctx.value.visible = false;
}

function ctxSelect(): void {
  const { table } = ctx.value;
  db.insertSelect(table);
  closeCtx();
}
function ctxCopy(): void {
  navigator.clipboard?.writeText(fullName(ctx.value.schema, ctx.value.table)).catch(() => {});
  db.setStatus('已复制：' + fullName(ctx.value.schema, ctx.value.table), 'ok');
  closeCtx();
}
function ctxAlias(): void {
  const { schema, table } = ctx.value;
  const key = `${schema}.${table}`;
  const cur = db.aliases[key] ?? '';
  const name = prompt('设置别名（留空清除）：', cur);
  if (name === null) return;
  void db.setAlias(key, name.trim());
  closeCtx();
}

onMounted(() => {
  if (!db.treeStatus && db.activeConn) void db.fetchTree();
});
</script>

<template>
  <div class="db-tree">
    <div class="db-tree__filter">
      <span>⌕</span>
      <input
        v-model="db.filter"
        placeholder="过滤表名 / 注释 / 别名"
        spellcheck="false"
      />
    </div>

    <div class="db-tree__scroll" ref="listEl">
      <div
        v-for="(r, i) in rows"
        :key="r.kind === 'schema' ? 's' + r.schema : r.kind === 'table' ? 't' + r.schema + r.table : 'c' + r.schema + r.table + (r.col?.name ?? i)"
        class="db-tree__row"
        :class="'db-tree__' + r.kind"
        :style="{ paddingLeft: r.depth * 16 + 8 + 'px' }"
        @click.stop="
          r.kind === 'schema'
            ? toggleSchema(r.schema)
            : r.kind === 'table'
              ? onTableClick(r.schema, r.table)
              : r.col && onColClick(aliasKey(r.schema, r.table, r.col.name))
        "
        @contextmenu.stop="r.kind === 'table' && onTableContext($event, r.schema, r.table)"
      >
        <span class="db-tree__tw">
          {{
            r.kind === 'schema'
              ? db.expanded[r.schema]
                ? '▾'
                : '▸'
              : r.kind === 'table'
                ? db.expanded[r.schema + '.' + r.table]
                  ? '▾'
                  : '▸'
                : '·'
          }}
        </span>
        <template v-if="r.kind === 'schema'">
          <span class="db-tree__schema-name">{{ r.schema }}</span>
        </template>
        <template v-else-if="r.kind === 'table'">
          <span class="db-tree__table-name">{{ r.table }}</span>
          <span v-if="db.aliases[fullName(r.schema, r.table)]" class="db-tree__alias">
            （别名：{{ db.aliases[fullName(r.schema, r.table)] }}）
          </span>
          <span v-if="r.comment" class="db-tree__comment">{{ r.comment }}</span>
        </template>
        <template v-else>
          <span class="db-tree__col-name">{{ r.col?.name }}</span>
          <span class="db-tree__col-type">{{ r.col?.type_name }}{{ r.col?.not_null ? ' · NN' : '' }}</span>
        </template>
      </div>

      <div v-if="db.activeConn && rows.length === 0" class="db-tree__empty">没有匹配的表</div>
    </div>

    <WarScrollBar :target="listEl" :scale="0.8" />

    <!-- table context menu -->
    <div
      v-if="ctx.visible"
      class="db-tree__ctx"
      :style="{ left: ctx.x + 'px', top: ctx.y + 'px' }"
    >
      <div @click="ctxSelect">查看前 100 行 → 填入编辑器</div>
      <div @click="ctxCopy">复制全限定名</div>
      <div class="db-tree__ctx-sep"></div>
      <div @click="ctxAlias">设置别名…</div>
    </div>
  </div>
</template>

<style scoped>
.db-tree {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}

.db-tree__filter {
  flex: none;
  display: flex;
  align-items: center;
  background: #15192299;
  border: 1px solid #2a3344;
  margin-bottom: 6px;
}
.db-tree__filter span {
  padding: 0 8px;
  color: var(--war-text-faint);
}
.db-tree__filter input {
  flex: 1;
  min-width: 0;
  background: transparent;
  border: none;
  outline: none;
  color: var(--war-text);
  font-family: SimSun, serif;
  font-size: 12px;
  padding: 5px 4px;
}
.db-tree__filter input::placeholder {
  color: var(--war-text-faint);
}

.db-tree__scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
}

.db-tree__row {
  line-height: 20px;
  font-size: 12px;
  color: var(--war-text-dim);
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  border-radius: 2px;
}
.db-tree__row:hover {
  color: var(--war-gold-bright);
  background: #1a2033aa;
}
.db-tree__tw {
  display: inline-block;
  width: 12px;
  text-align: center;
  color: var(--war-gold-dim);
}
.db-tree__schema-name {
  color: var(--war-gold);
  font-weight: bold;
  text-shadow:
    -1px 0 var(--war-outline-brown), 1px 0 var(--war-outline-brown),
    0 -1px var(--war-outline-brown), 0 1px var(--war-outline-brown);
}
.db-tree__table-name {
  color: var(--war-text);
}
.db-tree__comment {
  color: var(--war-text-faint);
  font-size: 11px;
  margin-left: 4px;
}
.db-tree__alias {
  color: var(--war-user-blue);
  font-size: 11px;
  margin-left: 4px;
}
.db-tree__col-name {
  color: var(--war-text-muted);
}
.db-tree__col-type {
  color: var(--war-text-faint);
  font-size: 11px;
  margin-left: 4px;
}
.db-tree__empty {
  color: var(--war-text-faint);
  font-size: 12px;
  padding: 14px;
  text-align: center;
}

.db-tree__ctx {
  position: fixed;
  z-index: 90;
  min-width: 180px;
  background: #0b0d12f5;
  border: 1px solid var(--war-gold-input);
  padding: 4px 0;
}
.db-tree__ctx div {
  padding: 6px 14px;
  font-size: 12px;
  color: var(--war-text-dim);
  cursor: pointer;
}
.db-tree__ctx div:hover {
  color: var(--war-gold-bright);
  background: #1a2033cc;
}
.db-tree__ctx-sep {
  height: 1px;
  margin: 3px 8px;
  background: #2a3344;
  padding: 0 !important;
}
</style>

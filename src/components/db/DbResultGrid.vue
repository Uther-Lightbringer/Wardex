<script setup lang="ts">
// Result grid (PRD M5.4): type-aware cells — numbers right-aligned, booleans
// ✓/✗, NULL gray, dates formatted; zebra rows; double-click cell → preview.
import { ref } from 'vue';
import type { QueryResult } from '../../stores/db';
import WarScrollBar from '../war/WarScrollBar.vue';

const props = defineProps<{ result: QueryResult }>();

const wrapEl = ref<HTMLElement | null>(null);
const preview = ref<{ col: string; value: string } | null>(null);

function cellClass(type: string, value: string | null): string {
  if (value === null || value === undefined) return 'grid-null';
  const t = type.toLowerCase();
  if (t.includes('bool')) return 'grid-bool ' + (value === 't' || value === 'true' ? 'grid-true' : 'grid-false');
  if (['int2', 'int4', 'int8', 'float4', 'float8', 'numeric', 'money'].some((n) => t.includes(n) || t === n)) {
    return 'grid-num';
  }
  if (t.includes('date') || t.includes('time')) return 'grid-datetime';
  return '';
}

function display(value: string | null, type: string): string {
  if (value === null || value === undefined) return '<NULL>';
  const t = type.toLowerCase();
  if (t.includes('bool')) return value === 't' || value === 'true' ? '✓' : '✗';
  return value;
}

function onCellDbl(e: MouseEvent, col: string, value: string | null): void {
  preview.value = { col, value: value ?? 'NULL' };
  e.stopPropagation();
}
</script>

<template>
  <div class="db-grid">
    <div class="db-grid__scroll" ref="wrapEl">
      <table class="db-grid__table">
        <thead>
          <tr>
            <th v-for="c in props.result.columns" :key="c.name">{{ c.name }}<span v-if="c.type_name" class="db-grid__type"> {{ c.type_name }}</span></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(row, i) in props.result.rows" :key="i">
            <td
              v-for="(cell, j) in row"
              :key="j"
              :class="[cellClass(props.result.columns[j]?.type_name ?? '', cell), 'db-grid__cell']"
              :title="cell ?? 'NULL'"
              @dblclick="onCellDbl($event, props.result.columns[j]?.name ?? '', cell)"
            >
              {{ display(cell, props.result.columns[j]?.type_name ?? '') }}
            </td>
          </tr>
        </tbody>
      </table>
      <div v-if="props.result.rows.length === 0" class="db-grid__empty">（空结果集）</div>
    </div>
    <WarScrollBar :target="wrapEl" :scale="0.8" />

    <!-- cell preview -->
    <div v-if="preview" class="db-grid__mask" @click.self="preview = null" @keydown.esc="preview = null">
      <div class="db-grid__pv">
        <div class="db-grid__pv-title">{{ preview.col }}</div>
        <div class="db-grid__pv-val">{{ preview.value }}</div>
        <button class="db-grid__pv-btn" @click="preview = null">关闭</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.db-grid {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}

.db-grid__scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  scrollbar-width: none;
  background: #0c101ccc;
  border: 1px solid #2a3344;
}

.db-grid__table {
  border-collapse: collapse;
  min-width: 100%;
  font: 12px/18px Consolas, monospace;
}

.db-grid__table th {
  position: sticky;
  top: 0;
  background: #161b28;
  color: var(--war-gold);
  font-weight: bold;
  padding: 5px 10px;
  text-align: left;
  border-bottom: 1px solid #2a3344;
  white-space: nowrap;
  z-index: 2;
}

.db-grid__type {
  color: var(--war-text-faint);
  font-weight: normal;
  font-size: 10px;
}

.db-grid__cell {
  padding: 3px 10px;
  white-space: nowrap;
  border-bottom: 1px solid #141a28;
  color: var(--war-text);
  cursor: cell;
}
.db-grid__table tbody tr:nth-child(odd) td {
  background: #0e131f66;
}
.db-grid__cell:hover {
  background: #1a2033aa;
}
.grid-num {
  text-align: right;
  color: var(--war-user-blue);
}
.grid-bool {
  text-align: center;
}
.grid-true {
  color: var(--war-green);
}
.grid-false {
  color: var(--war-error);
}
.grid-null {
  color: var(--war-text-faint);
  font-style: italic;
}
.grid-datetime {
  color: var(--war-text-dim);
}
.db-grid__empty {
  padding: 18px;
  text-align: center;
  color: var(--war-text-faint);
}

.db-grid__mask {
  position: fixed;
  inset: 0;
  z-index: 110;
  background: #000000b0;
  display: flex;
  align-items: center;
  justify-content: center;
}
.db-grid__pv {
  width: 480px;
  max-width: 90vw;
  background: #0b0d12;
  border: 1px solid var(--war-gold-input);
  padding: 16px;
}
.db-grid__pv-title {
  color: var(--war-gold);
  font-size: 14px;
  font-weight: bold;
  margin-bottom: 8px;
}
.db-grid__pv-val {
  font-family: Consolas, monospace;
  font-size: 13px;
  color: var(--war-text);
  word-break: break-all;
  max-height: 260px;
  overflow: auto;
  background: #10141f;
  border: 1px solid #2a3344;
  padding: 10px;
  margin-bottom: 12px;
}
.db-grid__pv-btn {
  background: #15192299;
  border: 1px solid var(--war-gold-input);
  color: var(--war-gold);
  padding: 4px 20px;
  cursor: pointer;
  float: right;
}
</style>

<script setup lang="ts">
// New-connection dialog (PRD FR-1.1/1.2): name + DSN, saved list at top.
import { ref, watch } from 'vue';
import { useDbStore } from '../../stores/db';
import WarDialog from '../war/WarDialog.vue';
import WarButton from '../war/WarButton.vue';

const db = useDbStore();

const name = ref('');
const dsn = ref('');

watch(
  () => db.connDialogOpen,
  (v) => {
    if (v) {
      name.value = '';
      dsn.value = '';
    }
  },
);

async function onSave(): Promise<void> {
  const n = name.value.trim();
  const d = dsn.value.trim();
  if (!n || !d) return;
  const next = [...db.connections.filter((c) => c.name !== n), { name: n, dsn: d }];
  await db.saveConns(next);
  db.connDialogOpen = false;
  await db.openConn(n, d);
}
</script>

<template>
  <WarDialog
    :open="db.connDialogOpen"
    title-text="新建数据库连接"
    :dialog-width="620"
    @update:open="db.connDialogOpen = $event"
  >
    <template #plate>
      <div class="db-conn__plate">
        <div v-if="db.connections.length" class="db-conn__saved-title">已保存（点击回填）：</div>
        <div v-for="c in db.connections" :key="c.name" class="db-conn__saved" @click="name = c.name; dsn = c.dsn">
          <span class="db-conn__saved-name">{{ c.name }}</span>
          <span class="db-conn__saved-dsn">{{ c.dsn }}</span>
        </div>
      </div>
    </template>
    <WarButton skin="dialog" :width="190" text="取消" @activated="db.connDialogOpen = false" />
    <WarButton skin="dialog" :width="190" text="保存并连接" @activated="onSave" />
  </WarDialog>
</template>

<style scoped>
.db-conn__plate {
  width: 100%;
  max-height: 120px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.db-conn__saved-title {
  color: var(--war-text-muted);
  font-size: 12px;
}
.db-conn__saved {
  display: flex;
  gap: 10px;
  font-size: 12px;
  padding: 3px 8px;
  background: #10141fcc;
  border: 1px solid #1a2230;
  cursor: pointer;
}
.db-conn__saved:hover {
  border-color: var(--war-gold-input);
}
.db-conn__saved-name {
  color: var(--war-gold);
  flex: none;
}
.db-conn__saved-dsn {
  color: var(--war-text-faint);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.db-conn__field {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 8px;
  width: 100%;
}
.db-conn__field label {
  color: var(--war-text-muted);
  font-size: 12px;
  flex: none;
  width: 74px;
  text-align: right;
}
.db-conn__field input {
  flex: 1;
  min-width: 0;
  background: #15192299;
  border: 1px solid #2a3344;
  color: var(--war-text);
  font-family: Consolas, monospace;
  font-size: 12px;
  padding: 5px 8px;
  outline: none;
}
.db-conn__field input:focus {
  border-color: var(--war-gold-input);
}
</style>

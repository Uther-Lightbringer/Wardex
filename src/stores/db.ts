// Database feature render state (project-bound, follows the current chat
// session's project). The Rust side owns connections/drivers; this store
// holds the UI model: saved connections, open list, active connection, the
// schema tree, the editor text and the per-statement results.
//
// AI 生成 SQL goes through the chat agent (see requestAi): the prompt is
// sent into the current chat session (which has the --mcp-db metadata tools
// injected), and when the assistant turn finishes the first ```sql block is
// extracted back into the editor — never auto-executed.

import { defineStore } from 'pinia';
import { cmd, isTauri } from '../lib/tauri';
import { useChatStore } from './chat';

export interface NamedConn {
  name: string;
  dsn: string;
}

export interface TableMeta {
  schema: string;
  name: string;
  comment: string;
}

export interface ColumnMeta {
  name: string;
  type_name: string;
  not_null: boolean;
  comment: string;
}

export interface Col {
  name: string;
  type_name: string;
}

export interface QueryResult {
  columns: Col[];
  rows: (string | null)[][];
  truncated: boolean;
  affected: number | null;
}

export interface ExecuteOutcome {
  need_confirm: boolean;
  has_write: boolean;
  statements: string[];
  results: QueryResult[] | null;
}

interface SchemaGroup {
  schema: string;
  tables: TableMeta[];
}

interface StmtResult {
  stmt: string;
  res: QueryResult;
}

/** Port of gate::extract_sql — first ```sql block of an AI reply. */
export function extractSql(text: string): string | null {
  const lower = text.toLowerCase();
  const start = lower.indexOf('```sql');
  if (start < 0) return null;
  const after = text.slice(start + 6);
  const end = after.indexOf('```');
  const sql = (end < 0 ? after : after.slice(0, end)).trim();
  return sql.length > 0 ? sql : null;
}

export const useDbStore = defineStore('db', {
  state: () => ({
    projectDir: '',
    connections: [] as NamedConn[],
    aliases: {} as Record<string, string>,
    open: [] as string[],
    activeConn: '',
    mode: 'ro' as 'ro' | 'rw',
    // schema tree
    tree: [] as SchemaGroup[],
    cols: {} as Record<string, ColumnMeta[]>,
    expanded: {} as Record<string, boolean>,
    filter: '',
    treeStatus: '',
    // editor
    sql: '',
    // AI
    aiPrompt: '',
    aiBusy: false,
    aiStatus: '',
    // results
    results: [] as StmtResult[],
    activeRes: 0,
    // status line
    statusText: '就绪',
    statusKind: '' as '' | 'ok' | 'err',
    // dialogs
    dialogOpen: false,
    connDialogOpen: false,
    execConfirm: null as { statements: string[] } | null,
  }),

  getters: {
    activeConnInfo(state): NamedConn | null {
      return state.connections.find((c) => c.name === state.activeConn) ?? null;
    },
    isOpen(state): (name: string) => boolean {
      return (name: string) => state.open.includes(name);
    },
  },

  actions: {
    /** Load saved connections/aliases/open list for a project. */
    async load(projectDir: string): Promise<void> {
      this.projectDir = projectDir;
      if (!isTauri) return;
      try {
        const v = await cmd<{
          connections: NamedConn[];
          aliases: Record<string, string>;
          open: string[];
        }>('db_conns', { projectDir }, { connections: [], aliases: {}, open: [] });
        this.connections = v.connections ?? [];
        this.aliases = v.aliases ?? {};
        this.open = v.open ?? [];
        if (!this.activeConn || !this.connections.some((c) => c.name === this.activeConn)) {
          this.activeConn = this.open[0] ?? this.connections[0]?.name ?? '';
        }
      } catch (e) {
        console.warn('[db] db_conns failed', e);
      }
    },

    async saveConns(conns: NamedConn[]): Promise<void> {
      if (!isTauri) return;
      try {
        await cmd('db_save_conns', { projectDir: this.projectDir, connections: conns });
        this.connections = conns;
      } catch (e) {
        this.setStatus(String(e), 'err');
      }
    },

    async setAlias(key: string, alias: string): Promise<void> {
      if (!isTauri) return;
      try {
        await cmd('db_set_alias', { projectDir: this.projectDir, key, alias });
        this.aliases = { ...this.aliases, [key]: alias };
        if (!alias) delete this.aliases[key];
      } catch (e) {
        console.warn('[db] db_set_alias failed', e);
      }
    },

    /** Open a named connection (uses its stored DSN if dsn omitted). */
    async openConn(name: string, dsn?: string): Promise<boolean> {
      const info = this.connections.find((c) => c.name === name);
      const d = dsn ?? info?.dsn ?? '';
      if (!d) {
        this.setStatus(`连接「${name}」缺少 DSN`, 'err');
        return false;
      }
      if (!isTauri) {
        this.open = [...this.open.filter((n) => n !== name), name];
        this.activeConn = name;
        return true;
      }
      try {
        await cmd('db_open', { projectDir: this.projectDir, name, dsn: d });
        if (!this.open.includes(name)) this.open = [...this.open, name];
        this.activeConn = name;
        this.setStatus(`已连接 ${name}`, 'ok');
        return true;
      } catch (e) {
        this.setStatus(String(e), 'err');
        return false;
      }
    },

    async closeConn(name: string): Promise<void> {
      this.open = this.open.filter((n) => n !== name);
      if (this.activeConn === name) {
        this.activeConn = this.open[0] ?? '';
        this.tree = [];
        this.cols = {};
      }
      if (isTauri) {
        try {
          await cmd('db_close', { projectDir: this.projectDir, name });
        } catch (e) {
          console.warn('[db] db_close failed', e);
        }
      }
    },

    /** Load the schema tree (schema → tables, comments + aliases). */
    async fetchTree(): Promise<void> {
      const name = this.activeConn;
      if (!name) return;
      try {
        const tables = await cmd<TableMeta[]>('db_tables', {
          projectDir: this.projectDir,
          name,
          schema: null,
          keyword: null,
        }, []);
        const groups: SchemaGroup[] = [];
        for (const t of tables) {
          let g = groups.find((x) => x.schema === t.schema);
          if (!g) {
            g = { schema: t.schema, tables: [] };
            groups.push(g);
          }
          g.tables.push(t);
        }
        this.tree = groups;
        this.treeStatus = '';
      } catch (e) {
        this.setStatus(String(e), 'err');
      }
    },

    /** Lazy-load columns of `qualified` (schema.table). */
    async fetchColumns(qualified: string): Promise<ColumnMeta[]> {
      const name = this.activeConn;
      if (!name) return [];
      if (this.cols[qualified]) return this.cols[qualified];
      try {
        const cols = await cmd<ColumnMeta[]>('db_columns', {
          projectDir: this.projectDir,
          name,
          qualified,
        }, []);
        this.cols = { ...this.cols, [qualified]: cols };
        return cols;
      } catch (e) {
        this.setStatus(String(e), 'err');
        return [];
      }
    },

    /** Fill the editor with a SELECT for the table (never auto-runs). */
    insertSelect(table: string): void {
      const g = this.tree.find((x) => x.tables.some((t) => t.name === table));
      const fq = g ? `${g.schema}.${table}` : table;
      this.sql = `SELECT * FROM ${fq}\nLIMIT 100;`;
      this.setStatus(`已填入 SELECT ${fq}，请人工核对后点击执行`, 'ok');
    },

    /** Execute the editor SQL through the gate. Returns the outcome kind. */
    async execute(confirmed = false): Promise<'needConfirm' | 'ok' | 'error'> {
      const name = this.activeConn;
      if (!name) return 'error';
      if (!this.sql.trim()) {
        this.setStatus('SQL 为空', 'err');
        return 'error';
      }
      if (!isTauri) {
        // Browser preview: no backend — just a fake row so the grid renders.
        this.results = [{ stmt: this.sql, res: fakeResult(this.sql) }];
        this.activeRes = 0;
        this.setStatus('预览模式：未执行真实查询', 'ok');
        return 'ok';
      }
      try {
        const out = await cmd<ExecuteOutcome>('db_execute', {
          projectDir: this.projectDir,
          name,
          sql: this.sql,
          allowWrite: this.mode === 'rw',
          confirmed,
        });
        if (out.need_confirm) {
          this.execConfirm = { statements: out.statements ?? [] };
          return 'needConfirm';
        }
        const results = (out.results ?? []).map((res, i) => ({
          stmt: out.statements[i] ?? '',
          res,
        }));
        this.results = results;
        this.activeRes = Math.max(0, results.length - 1);
        const last = results[this.activeRes]?.res;
        if (last) {
          if (last.affected != null) {
            this.setStatus(
              `第 ${results.length} 条执行完成，影响行数：${last.affected}`,
              'ok',
            );
          } else if (last.columns.length === 0) {
            this.setStatus('执行完成（无结果集）', 'ok');
          } else if (last.truncated) {
            this.setStatus(`共 ${last.rows.length}+ 行，仅显示前 100 行（已被截断）`, 'ok');
          } else {
            this.setStatus(`共 ${last.rows.length} 行`, 'ok');
          }
        }
        return 'ok';
      } catch (e) {
        this.setStatus(String(e), 'err');
        return 'error';
      }
    },

    /** Send the AI prompt into the current chat session and wait for SQL. */
    async requestAi(): Promise<boolean> {
      const prompt = this.aiPrompt.trim();
      if (!prompt) return false;
      const chat = useChatStore();
      if (!chat.sessionId) {
        this.aiStatus = '没有打开的聊天会话';
        return false;
      }
      const before = chat.rows.length;
      const ok = await chat.send(prompt, []);
      if (!ok) {
        this.aiBusy = false;
        this.aiStatus = chat.status.lastError || '发送失败';
        return false;
      }
      this.aiBusy = true;
      this.aiStatus = '已发送到会话，等待 Agent 生成 SQL…';
      const t0 = Date.now();
      return new Promise<boolean>((resolve) => {
        const timer = setInterval(() => {
          const rows = chat.rows;
          const last = rows.length > before ? rows[rows.length - 1] : null;
          const done =
            last &&
            last.role === 'assistant' &&
            (last.status === 'done' || last.status === 'error' || last.status === 'interrupted') &&
            Date.now() - t0 > 800;
          if (done) {
            clearInterval(timer);
            this.aiBusy = false;
            const sql = extractSql(last.content);
            if (sql) {
              this.sql = sql;
              this.aiStatus = '已生成 SQL，请人工核对后点击执行';
              this.setStatus('AI 已生成 SQL（只填入编辑器，未执行）', 'ok');
              resolve(true);
            } else {
              this.aiStatus = 'Agent 未返回 SQL，可到会话中查看';
              resolve(false);
            }
          } else if (Date.now() - t0 > 180_000) {
            clearInterval(timer);
            this.aiBusy = false;
            this.aiStatus = '等待超时（3 分钟）';
            resolve(false);
          }
        }, 300);
      });
    },

    toggleMode(): 'ro' | 'rw' {
      this.mode = this.mode === 'ro' ? 'rw' : 'ro';
      return this.mode;
    },

    setStatus(text: string, kind: '' | 'ok' | 'err' = ''): void {
      this.statusText = text;
      this.statusKind = kind;
    },
  },
});

/** Browser-preview fake result (no Tauri runtime). */
function fakeResult(sql: string): QueryResult {
  const matches = sql.match(/from\s+([a-z_.]+)/i);
  const table = matches?.[1]?.split('.').pop() ?? '';
  const cols = ['id', 'name', 'value'];
  const rows = [
    [1, table ? `${table} 行一` : 'a', 10.5],
    [2, table ? `${table} 行二` : 'b', null],
  ];
  return {
    columns: cols.map((c) => ({ name: c, type_name: c === 'value' ? 'numeric' : 'text' })),
    rows: rows.map((r) => r.map((v) => (v === null ? null : String(v)))),
    truncated: false,
    affected: null,
  };
}

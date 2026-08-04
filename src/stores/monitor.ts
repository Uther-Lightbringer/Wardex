// 战场监控页（MonitorPage）聚合 store：部署布局读写（prefs.monitorLayout）、
// 会话派生（sessions.all + runtime_states）、权限等待 payload、小窗/审批弹窗
// 状态。权威数据仍在 Rust stores；每个变更都走 Tauri 命令 + 重拉。
//
// 不 import chat store 的运行时（只用其 PermissionRequest 类型），事件监听
// 独立于 chat.init——监控页可能在聊天页之前打开。

import { defineStore } from 'pinia';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { cmd, isTauri } from '../lib/tauri';
import { useSessionsStore, type SessionIndexRow } from './sessions';
import { usePrefsStore } from './prefs';
import type { PermissionRequest } from './chat';

let listenersReady = false;
const unlisteners: UnlistenFn[] = [];

/** 迷你会话窗实例（多开）：x/y 是 left/top 定位（不持久化），z 是叠放序
 * （70 起步递增，永远低于审批弹窗的 75）。 */
export interface ChatWin {
  sessionId: string;
  x: number;
  y: number;
  z: number;
}

export const useMonitorStore = defineStore('monitor', {
  state: () => ({
    /** 待部署的项目目录（'' = 未在部署；ghost 跟随鼠标）。 */
    deploying: '',
    /** 部署落盘 + 搁置重拉进行中的项目（期间不渲染步兵，
     *  避免旧会话在栏位里闪一下再被搁置消失）。 */
    deploySettling: [] as string[],
    /** 每个会话待审批的权限 payload（acp://permission 实时 + 补拉）。 */
    permPayloads: {} as Record<string, PermissionRequest>,
    /** 迷你会话窗（可多开；同一会话只开一个）。 */
    chatWins: [] as ChatWin[],
    /** 权限审批弹窗当前会话（'' = 关闭）。 */
    permDialogSessionId: '',
    /** 小窗已看过的会话：本地抑制 NEW 角标（后端 unread 只在 open_session
     * 时清除，监控页不切 active；chat://unread 再触发时自动恢复）。 */
    readLocal: [] as string[],
  }),
  getters: {
    /** 步兵金叹号：事件标记 ∪ 后端快照（重启后快照兜底）。 */
    isPermPending(): (id: string) => boolean {
      return (id: string) => {
        const sessions = useSessionsStore();
        return (
          sessions.permPending.includes(id) ||
          sessions.runtimeStates[id]?.permPending === true
        );
      };
    },
    /** 等待审批会话数（主菜单角标 / 左 rail 计数）。 */
    permPendingCount(): number {
      const sessions = useSessionsStore();
      const set = new Set<string>(sessions.permPending);
      for (const [id, st] of Object.entries(sessions.runtimeStates)) {
        if (st.permPending) set.add(id);
      }
      return set.size;
    },
    /** 某项目的在营会话（未搁置，createdAt 升序 → 栏位稳定）。 */
    sessionsOf(): (dir: string) => SessionIndexRow[] {
      return (dir: string) => {
        const sessions = useSessionsStore();
        return sessions.all
          .filter((s) => s.projectDir === dir && !s.shelved)
          .slice()
          .sort((a, b) => a.createdAt - b.createdAt);
      };
    },
    shelvedOf(): (dir: string) => SessionIndexRow[] {
      return (dir: string) => {
        const sessions = useSessionsStore();
        return sessions.all
          .filter((s) => s.projectDir === dir && s.shelved)
          .slice()
          .sort((a, b) => a.createdAt - b.createdAt);
      };
    },
  },
  actions: {
    /** 一次性事件 wiring（MonitorPage onMounted；页面常驻不卸载）。 */
    async initListeners(): Promise<void> {
      if (listenersReady || !isTauri) return;
      listenersReady = true;
      const sessions = useSessionsStore();
      unlisteners.push(
        await listen('store://sessions', () => void sessions.reloadAll()),
        // busy 状态点：合并进对应会话的 runtimeStates 条目（不整拉）。
        await listen<{ sessionId: string; busy: boolean; queueLength: number }>(
          'chat://status',
          (e) => {
            const { sessionId, busy, queueLength } = e.payload;
            const cur = sessions.runtimeStates[sessionId];
            if (cur) {
              sessions.runtimeStates = {
                ...sessions.runtimeStates,
                [sessionId]: { ...cur, busy, queueLength },
              };
            } else {
              void sessions.refreshRuntimeStates();
            }
          },
        ),
        await listen<PermissionRequest>('acp://permission', (e) => {
          const p = e.payload;
          sessions.markPermPending(p.sessionId, true);
          this.permPayloads = { ...this.permPayloads, [p.sessionId]: p };
          const rt = sessions.runtimeStates[p.sessionId];
          if (rt) {
            sessions.runtimeStates = {
              ...sessions.runtimeStates,
              [p.sessionId]: { ...rt, permPending: true },
            };
          }
        }),
        await listen<{ sessionId: string }>('acp://permissionCleared', (e) => {
          const id = e.payload.sessionId;
          sessions.markPermPending(id, false);
          const rt = sessions.runtimeStates[id];
          if (rt) {
            sessions.runtimeStates = {
              ...sessions.runtimeStates,
              [id]: { ...rt, permPending: false },
            };
          }
          if (id in this.permPayloads) {
            const rest = { ...this.permPayloads };
            delete rest[id];
            this.permPayloads = rest;
          }
          if (this.permDialogSessionId === id) this.permDialogSessionId = '';
        }),
        // 后台会话 turn 完成 → NEW 角标（清掉本地已读抑制）。
        await listen<{ sessionId: string }>('chat://unread', (e) => {
          const id = e.payload.sessionId;
          sessions.markUnread(id);
          if (this.readLocal.includes(id)) {
            this.readLocal = this.readLocal.filter((s) => s !== id);
          }
        }),
      );
    },

    /** 页面进入时整拉：索引 + runtime 快照。 */
    async refresh(): Promise<void> {
      const sessions = useSessionsStore();
      await Promise.all([sessions.reloadAll(), sessions.refreshRuntimeStates()]);
    },

    // ---- 部署 ----

    startDeploy(dir: string): void {
      this.deploying = dir;
    },
    cancelDeploy(): void {
      this.deploying = '';
    },
    /** 落点（0..1 比例坐标）→ 持久化。新部署的兵营从空地开始：
     *  后端在同一个命令里把该项目既有会话全部搁置（会话保留，可从兵营菜单「已搁置」恢复）。 */
    async deploy(dir: string, x: number, y: number): Promise<void> {
      this.deploying = '';
      this.deploySettling = [...this.deploySettling, dir];
      try {
        const prefs = usePrefsStore();
        await prefs.setMonitorLayout(dir, { x, y });
        await useSessionsStore().reloadAll();
      } finally {
        this.deploySettling = this.deploySettling.filter((d) => d !== dir);
      }
    },
    /** 销毁兵营：后端在同一个命令里把该项目未搁置会话全部搁置
     * （与部署一致，历史会话进「已搁置」可逐个恢复）。 */
    async raze(dir: string): Promise<void> {
      const prefs = usePrefsStore();
      await prefs.setMonitorLayout(dir, null);
      await useSessionsStore().reloadAll();
    },

    // ---- 会话 ----

    /** 兵营右键新会话：指定 Agent + 权限模式；返回新会话 id（'' = 失败）。 */
    async newSession(dir: string, agentId: string, permMode: string): Promise<string> {
      if (!isTauri) return '';
      try {
        const id = await cmd<string>('create_session', {
          projectDir: dir,
          groupId: '',
          agentId: agentId || undefined,
          permMode: permMode || undefined,
        });
        await useSessionsStore().reloadAll();
        return id;
      } catch (e) {
        console.warn('[monitor] create_session failed', e);
        return '';
      }
    },

    async rename(id: string, title: string): Promise<void> {
      const sessions = useSessionsStore();
      await sessions.rename(id, title);
      await sessions.reloadAll();
    },

    async setShelved(id: string, shelved: boolean): Promise<void> {
      if (!isTauri) return;
      try {
        await cmd('set_session_shelved', { sessionId: id, shelved });
      } catch (e) {
        console.warn('[monitor] set_session_shelved failed', e);
      }
      await useSessionsStore().reloadAll();
    },

    async remove(id: string): Promise<void> {
      if (!isTauri) return;
      try {
        await cmd('delete_session', { sessionId: id });
      } catch (e) {
        console.warn('[monitor] delete_session failed', e);
      }
      await useSessionsStore().reloadAll();
    },

    // ---- 权限审批 ----

    /** 步兵左键（perm 态）：确保有 payload 再开弹窗；后台会话先补拉。 */
    async openPermDialog(sessionId: string): Promise<void> {
      this.permDialogSessionId = sessionId;
      if (this.permPayloads[sessionId] || !isTauri) return;
      try {
        await cmd('ensure_runtime', { sessionId });
        const p = await cmd<PermissionRequest | null>(
          'pending_permission',
          { sessionId },
          null,
        );
        if (p) this.permPayloads = { ...this.permPayloads, [sessionId]: p };
      } catch (e) {
        console.warn('[monitor] pending_permission failed', e);
      }
    },
    closePermDialog(): void {
      this.permDialogSessionId = '';
    },
    async answerPermission(sessionId: string, optionId: string, cancelled: boolean): Promise<void> {
      this.permDialogSessionId = '';
      if (sessionId in this.permPayloads) {
        const rest = { ...this.permPayloads };
        delete rest[sessionId];
        this.permPayloads = rest;
      }
      try {
        await cmd('answer_permission', { sessionId, optionId, cancelled });
      } catch (e) {
        console.warn('[monitor] answer_permission failed', e);
      }
    },

    async setPermMode(sessionId: string, mode: string | null): Promise<void> {
      if (!isTauri) return;
      try {
        await cmd('set_session_perm_mode', { sessionId, mode });
      } catch (e) {
        console.warn('[monitor] set_session_perm_mode failed', e);
      }
      await useSessionsStore().reloadAll();
    },

    /** 小窗切换 Agent：先确保 runtime 再切（照 chat.switchAgent；busy 时后端
     * 自行处理，前端不拦）。成功返回 true，失败由调用方 toast。 */
    async switchAgent(sessionId: string, agentId: string): Promise<boolean> {
      if (!isTauri) return false;
      try {
        await cmd('ensure_runtime', { sessionId });
        await cmd('switch_agent', { sessionId, agentId });
      } catch (e) {
        console.warn('[monitor] switch_agent failed', e);
        return false;
      }
      await useSessionsStore().reloadAll();
      return true;
    },

    // ---- 迷你会话窗 ----

    /** 开小窗：已开则置前；未开则新增（级联偏移初始位置，避免完全重叠）。 */
    openChatWin(id: string): void {
      const cur = this.chatWins.find((w) => w.sessionId === id);
      if (!cur) {
        const i = this.chatWins.length;
        this.chatWins = [...this.chatWins, { sessionId: id, x: 60 + i * 32, y: 80 + i * 32, z: 0 }];
      }
      this.raiseChatWin(id);
      if (!this.readLocal.includes(id)) this.readLocal = [...this.readLocal, id];
    },
    closeChatWin(id: string): void {
      this.chatWins = this.chatWins
        .filter((w) => w.sessionId !== id)
        .map((w, i) => ({ ...w, z: 70 + i })); // 保持叠放序紧凑、相对不变
    },
    /** 置前：z 重排为 70..70+n（永远低于审批弹窗的 75）。 */
    raiseChatWin(id: string): void {
      const cur = this.chatWins.find((w) => w.sessionId === id);
      if (!cur) return;
      const rest = this.chatWins.filter((w) => w.sessionId !== id);
      this.chatWins = [...rest, cur].map((w, i) => ({ ...w, z: 70 + i }));
    },
    /** 标题栏拖动改位置（不持久化）。 */
    moveChatWin(id: string, x: number, y: number): void {
      this.chatWins = this.chatWins.map((w) => (w.sessionId === id ? { ...w, x, y } : w));
    },
    /** Esc：关最上层小窗。 */
    closeTopChatWin(): void {
      if (this.chatWins.length === 0) return;
      const top = this.chatWins.reduce((a, b) => (b.z > a.z ? b : a));
      this.closeChatWin(top.sessionId);
    },

    /** 小窗发送：后台会话先确保有 runtime，再发 prompt。 */
    async sendTo(sessionId: string, text: string): Promise<boolean> {
      if (!isTauri) return false;
      try {
        await cmd('ensure_runtime', { sessionId });
        await cmd<'sent' | 'enqueued'>('send_prompt', { sessionId, text, attachments: [] });
        return true;
      } catch (e) {
        console.warn('[monitor] send_prompt failed', e);
        return false;
      }
    },
  },
});

/**
 * WarDex session reminders for the embedded Pi agent.
 *
 * Replaces the ACP `--mcp-reminder` MCP server: same todos.json rows
 * (scope=session, notifyMode=push) so the WarDex runtime still wakes the
 * session when due. Context comes from env (WARDEX_SESSION_ID /
 * WARDEX_TODOS_PATH) injected at spawn.
 */

import * as fs from "node:fs";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

interface TodoRow {
	id: string;
	title: string;
	done: boolean;
	createdAt: number;
	doneAt: number;
	scope: string;
	sessionId: string;
	projectDir: string;
	dueAtMs: number;
	notifiedAtMs: number;
	notifyMode: string;
}

function env(name: string): string {
	return (process.env[name] ?? "").trim();
}

function loadTodos(path: string): TodoRow[] {
	try {
		const raw = JSON.parse(fs.readFileSync(path, "utf8")) as { todos?: TodoRow[] };
		return Array.isArray(raw.todos) ? raw.todos : [];
	} catch {
		return [];
	}
}

function saveTodos(path: string, todos: TodoRow[]): void {
	const tmp = `${path}.tmp`;
	fs.writeFileSync(tmp, `${JSON.stringify({ todos }, null, 2)}\n`);
	try {
		fs.renameSync(tmp, path);
	} catch {
		try {
			fs.unlinkSync(path);
		} catch {
			/* ignore */
		}
		fs.renameSync(tmp, path);
	}
}

function nowMs(): number {
	return Date.now();
}

function newRow(title: string, sessionId: string, dueAtMs: number): TodoRow {
	return {
		id: crypto.randomUUID(),
		title,
		done: false,
		createdAt: nowMs(),
		doneAt: 0,
		scope: "session",
		sessionId,
		projectDir: "",
		dueAtMs,
		notifiedAtMs: 0,
		notifyMode: "push",
	};
}

function requireCtx(): { sessionId: string; path: string } {
	const sessionId = env("WARDEX_SESSION_ID");
	const path = env("WARDEX_TODOS_PATH");
	if (!sessionId || !path) {
		throw new Error("WARDEX_SESSION_ID / WARDEX_TODOS_PATH 未设置，无法使用提醒工具");
	}
	return { sessionId, path };
}

export default function (pi: ExtensionAPI) {
	pi.registerTool({
		name: "set_reminder",
		label: "Set Reminder",
		description:
			"Set a one-shot reminder for this chat session. When the time comes, WarDex posts the reminder content back into this chat as a new prompt.",
		promptSnippet: "Set a timed reminder that comes back as a chat prompt",
		promptGuidelines: [
			"Use set_reminder when the user asks to be reminded later, or when you started background work and need to wake yourself (e.g. 1 minute).",
		],
		parameters: Type.Object({
			minutes: Type.Number({ description: "Minutes from now until the reminder fires (must be > 0)" }),
			content: Type.String({ description: "What to remind about" }),
		}),
		async execute(_id, params) {
			try {
				const { sessionId, path } = requireCtx();
				const minutes = params.minutes;
				const content = (params.content ?? "").trim();
				if (!(minutes > 0) || !content) {
					return {
						content: [{ type: "text" as const, text: "invalid arguments: minutes must be > 0 and content non-empty" }],
					};
				}
				const row = newRow(content, sessionId, nowMs() + Math.round(minutes * 60_000));
				const rows = loadTodos(path);
				rows.push(row);
				saveTodos(path, rows);
				return { content: [{ type: "text" as const, text: JSON.stringify(row) }] };
			} catch (e) {
				return { content: [{ type: "text" as const, text: e instanceof Error ? e.message : String(e) }] };
			}
		},
	});

	pi.registerTool({
		name: "cancel_reminder",
		label: "Cancel Reminder",
		description: "Cancel a pending reminder by its id (from list_reminders).",
		parameters: Type.Object({
			id: Type.String({ description: "Reminder id to cancel" }),
		}),
		async execute(_id, params) {
			try {
				const { path } = requireCtx();
				const id = (params.id ?? "").trim();
				if (!id) {
					return { content: [{ type: "text" as const, text: "invalid arguments: id is required" }] };
				}
				const rows = loadTodos(path);
				const next = rows.filter((r) => r.id !== id);
				if (next.length === rows.length) {
					return { content: [{ type: "text" as const, text: `reminder not found: ${id}` }] };
				}
				saveTodos(path, next);
				return { content: [{ type: "text" as const, text: `cancelled ${id}` }] };
			} catch (e) {
				return { content: [{ type: "text" as const, text: e instanceof Error ? e.message : String(e) }] };
			}
		},
	});

	pi.registerTool({
		name: "list_reminders",
		label: "List Reminders",
		description: "List all pending reminders of this chat session.",
		parameters: Type.Object({}),
		async execute() {
			try {
				const { sessionId, path } = requireCtx();
				const rows = loadTodos(path).filter(
					(r) => r.scope === "session" && r.sessionId === sessionId && r.notifyMode === "push" && !r.done,
				);
				return { content: [{ type: "text" as const, text: JSON.stringify(rows) }] };
			} catch (e) {
				return { content: [{ type: "text" as const, text: e instanceof Error ? e.message : String(e) }] };
			}
		},
	});
}

/**
 * WarDex session search tools for the embedded Pi agent.
 *
 * Searches WarDex's OWN local session store — no network, no credentials:
 * <data root>/sessions/<uuid>/meta.json + messages.jsonl (same files the
 * Rust SessionStore indexes for the desktop search UI). The data root is
 * located via env WARDEX_SESSIONS_DIR (injected at spawn) → derived from
 * WARDEX_TODOS_PATH's parent → %APPDATA%/WarDex(-tauri-dev)/sessions.
 */

import * as fs from "node:fs";
import * as path from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

const SNIPPET_CONTEXT = 40;
const MAX_HITS_PER_SESSION = 3;
const PLACEHOLDER = "…";

function env(name: string): string {
	return (process.env[name] ?? "").trim();
}

function textResult(text: string) {
	return { content: [{ type: "text" as const, text }] };
}

/** Directory holding <uuid>/{meta.json,messages.jsonl} session dirs. */
function sessionsDir(): string {
	const direct = env("WARDEX_SESSIONS_DIR");
	if (direct) return direct;
	const todos = env("WARDEX_TODOS_PATH");
	if (todos) return path.join(path.dirname(todos), "sessions");
	const appdata = env("APPDATA");
	if (appdata) {
		for (const name of ["WarDex", "WarDex-tauri-dev"]) {
			const dir = path.join(appdata, name, "sessions");
			if (fs.existsSync(dir)) return dir;
		}
		return path.join(appdata, "WarDex", "sessions");
	}
	return "";
}

interface Meta {
	id?: string;
	title?: string;
	agentName?: string;
	provider?: string;
	status?: string;
	projectDir?: string;
	messageCount?: number;
	createdAt?: number;
	updatedAt?: number;
	shelved?: boolean;
	pinned?: boolean;
	lastMessage?: string;
	workshop?: boolean;
	parentId?: string;
}

function readMeta(sessionDir: string): Meta | null {
	try {
		const raw = fs.readFileSync(path.join(sessionDir, "meta.json"), "utf8");
		const m = JSON.parse(raw) as Meta;
		if (!m.id) m.id = path.basename(sessionDir);
		return m;
	} catch {
		return null;
	}
}

/** All session metas, newest first (same ordering as the Rust index). */
function listMetas(dir: string): Meta[] {
	let entries: string[];
	try {
		entries = fs.readdirSync(dir);
	} catch {
		return [];
	}
	const metas: Meta[] = [];
	for (const e of entries) {
		const sd = path.join(dir, e);
		try {
			if (!fs.statSync(sd).isDirectory()) continue;
		} catch {
			continue;
		}
		const m = readMeta(sd);
		if (m) metas.push(m);
	}
	metas.sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0));
	return metas;
}

function fmtTs(ms: number | undefined): string {
	if (!ms) return "?";
	return new Date(ms).toISOString().replace("T", " ").slice(0, 19);
}

function fmtRow(m: Meta, selfId: string): string {
	const parts = [
		m.id ?? "?",
		m.title || "(无标题)",
		`${m.agentName || m.provider || "?"}`,
		`msgs=${m.messageCount ?? "?"}`,
		`updated=${fmtTs(m.updatedAt)}`,
	];
	if (m.pinned) parts.push("pinned");
	if (m.shelved) parts.push("shelved");
	if (m.parentId) parts.push(`parent=${m.parentId}`);
	if (m.projectDir) parts.push(`project=${m.projectDir}`);
	if (selfId && m.id === selfId) parts.push("(本会话)");
	return `- ${parts.join(" | ")}`;
}

function findInsensitive(haystack: string, needle: string): number {
	return haystack.toLowerCase().indexOf(needle.toLowerCase());
}

/** Snippet of `context` chars on each side of the match (char-safe for CJK). */
function snippetAround(content: string, matchStart: number, matchEnd: number, context: number): string {
	const from = Math.max(0, matchStart - context);
	const to = Math.min(content.length, matchEnd + context);
	let out = content.slice(from, to).replace(/\s+/g, " ").trim();
	if (from > 0) out = `…${out}`;
	if (to < content.length) out = `${out}…`;
	return out;
}

interface MsgRow {
	role?: string;
	content?: string;
	createdAt?: number;
	status?: string;
}

interface Hit {
	snippet: string;
	ts: number;
}

/** Message-body hits in one session's messages.jsonl, newest first. */
function messageHits(messagesPath: string, query: string): Hit[] {
	let bytes: Buffer;
	try {
		bytes = fs.readFileSync(messagesPath);
	} catch {
		return [];
	}
	const hits: Hit[] = [];
	for (const line of bytes.toString("utf8").split("\n")) {
		const raw = line.trim();
		if (!raw) continue;
		let o: MsgRow;
		try {
			o = JSON.parse(raw) as MsgRow;
		} catch {
			continue;
		}
		if (o.role !== "user" && o.role !== "assistant") continue;
		const content = (o.content ?? "").replace(/^…+/, "");
		if (!content || content === PLACEHOLDER) continue;
		const at = findInsensitive(content, query);
		if (at < 0) continue;
		hits.push({ snippet: snippetAround(content, at, at + query.length, SNIPPET_CONTEXT), ts: o.createdAt ?? 0 });
	}
	hits.reverse();
	return hits;
}

export default function (pi: ExtensionAPI) {
	pi.registerTool({
		name: "session_list",
		label: "Session List",
		description:
			"List WarDex chat sessions from the local session store, newest first. " +
			"Returns one line per session: id, title, agent, message count, updatedAt, flags, projectDir.",
		promptSnippet: "List recent WarDex sessions",
		promptGuidelines: [
			"Use session_list to enumerate WarDex sessions; use session_search to find sessions by title or message content.",
		],
		parameters: Type.Object({
			limit: Type.Optional(Type.Number({ description: "Max sessions, default 20" })),
			include_shelved: Type.Optional(
				Type.Boolean({ description: "Include shelved (monitor-hidden) sessions, default false" }),
			),
			project_only: Type.Optional(
				Type.Boolean({
					description: "Only sessions of the current project (WARDEX_PROJECT_DIR), default false",
				}),
			),
		}),
		async execute(_id, params) {
			const dir = sessionsDir();
			if (!dir || !fs.existsSync(dir)) {
				return textResult("找不到 WarDex 会话存储目录（WARDEX_SESSIONS_DIR 未注入且无法推断）。");
			}
			const selfId = env("WARDEX_SESSION_ID");
			const proj = env("WARDEX_PROJECT_DIR");
			const limit = Math.min(Math.max(Math.trunc(params.limit ?? 20) || 20, 1), 200);
			const rows = listMetas(dir).filter(
				(m) =>
					(params.include_shelved || !m.shelved) &&
					(!params.project_only || !proj || m.projectDir === proj),
			);
			if (rows.length === 0) return textResult("没有匹配的会话。");
			return textResult(rows.slice(0, limit).map((m) => fmtRow(m, selfId)).join("\n"));
		},
	});

	pi.registerTool({
		name: "session_search",
		label: "Session Search",
		description:
			"Full-text search across WarDex sessions: matches the query against session titles AND " +
			"user/assistant message bodies in messages.jsonl (local disk, no network). " +
			"Returns matching sessions with snippets and hit counts.",
		promptSnippet: "Search WarDex session titles and message content",
		promptGuidelines: [
			"Use session_search when the user asks to find an earlier chat/conversation by keyword; snippets show the match context and the session id for opening it in WarDex.",
		],
		parameters: Type.Object({
			query: Type.String({ description: "Case-insensitive keyword to match in titles and message text" }),
			limit: Type.Optional(Type.Number({ description: "Max result entries, default 10" })),
			include_shelved: Type.Optional(
				Type.Boolean({ description: "Include shelved sessions, default false" }),
			),
			project_only: Type.Optional(
				Type.Boolean({
					description: "Only sessions of the current project (WARDEX_PROJECT_DIR), default false",
				}),
			),
		}),
		async execute(_id, params) {
			const q = params.query.trim();
			if (!q) return textResult("query 不能为空。");
			const dir = sessionsDir();
			if (!dir || !fs.existsSync(dir)) {
				return textResult("找不到 WarDex 会话存储目录（WARDEX_SESSIONS_DIR 未注入且无法推断）。");
			}
			const selfId = env("WARDEX_SESSION_ID");
			const proj = env("WARDEX_PROJECT_DIR");
			const limit = Math.min(Math.max(Math.trunc(params.limit ?? 10) || 10, 1), 100);
			const metas = listMetas(dir).filter(
				(m) =>
					(params.include_shelved || !m.shelved) &&
					(!params.project_only || !proj || m.projectDir === proj),
			);

			const out: string[] = [];
			for (const m of metas) {
				if (out.length >= limit) break;
				const titleHit = findInsensitive(m.title ?? "", q) >= 0;
				const hits = messageHits(path.join(dir, m.id ?? "", "messages.jsonl"), q);
				if (!titleHit && hits.length === 0) continue;
				const head = fmtRow(m, selfId).slice(2); // strip "- "
				if (hits.length === 0) {
					out.push(`- ${head}\n    标题匹配`);
				} else {
					out.push(`- ${head}\n    命中 ${hits.length} 处：`);
					for (const h of hits.slice(0, MAX_HITS_PER_SESSION)) {
						out.push(`    [${fmtTs(h.ts)}] ${h.snippet}`);
					}
				}
			}
			if (out.length === 0) return textResult(`没有匹配「${q}」的会话。`);
			return textResult(out.join("\n"));
		},
	});
}

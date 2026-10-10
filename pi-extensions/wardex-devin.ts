/**
 * WarDex Devin session tools for the embedded Pi agent.
 *
 * Same capability as Devin's builtin devin_session_search: list/search Devin
 * sessions of the user's organization via the public REST API
 * (GET /v1/sessions, GET /v1/session/{id}). Auth comes from env
 * DEVIN_API_KEY (a Devin API key, https://app.devin.ai/settings/api-keys);
 * optional WARDEX_DEVIN_API_BASE overrides the API origin for enterprise
 * deployments.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

const RESULT_LIMIT = 12000;

function env(name: string): string {
	return (process.env[name] ?? "").trim();
}

function apiBase(): string {
	return (env("WARDEX_DEVIN_API_BASE") || "https://api.devin.ai").replace(/\/+$/, "");
}

function textResult(text: string) {
	return { content: [{ type: "text" as const, text }] };
}

function noKeyResult() {
	return textResult(
		"未配置 DEVIN_API_KEY：Devin 会话工具需要一个 Devin API key。\n" +
			"在 https://app.devin.ai/settings/api-keys 生成 key，然后把它设成环境变量 DEVIN_API_KEY " +
			"再启动 WarDex（或写进启动脚本）。企业部署可用 WARDEX_DEVIN_API_BASE 覆盖 API 域名。",
	);
}

interface DevinApiResult {
	ok: boolean;
	status: number;
	body: string;
	json?: unknown;
}

type FetchOutcome = DevinApiResult | ReturnType<typeof noKeyResult>;

async function devinFetch(path: string): Promise<FetchOutcome> {
	const key = env("DEVIN_API_KEY");
	if (!key) return noKeyResult();
	try {
		const res = await fetch(`${apiBase()}${path}`, {
			headers: { Authorization: `Bearer ${key}`, Accept: "application/json" },
		});
		const body = await res.text();
		if (!res.ok) {
			const hint =
				res.status === 401 || res.status === 403
					? "（DEVIN_API_KEY 无效或权限不足）"
					: "";
			return { ok: false, status: res.status, body: `HTTP ${res.status}${hint}：${body.slice(0, 500)}` };
		}
		try {
			return { ok: true, status: res.status, body, json: JSON.parse(body) };
		} catch {
			return { ok: false, status: res.status, body: `返回不是 JSON：${body.slice(0, 500)}` };
		}
	} catch (e) {
		return { ok: false, status: 0, body: `请求失败：${e instanceof Error ? e.message : String(e)}` };
	}
}

function truncate(text: string): string {
	return text.length > RESULT_LIMIT ? `${text.slice(0, RESULT_LIMIT)}\n…（截断）` : text;
}

interface SessionRow {
	session_id?: string;
	title?: string | null;
	status?: string;
	status_enum?: string | null;
	created_at?: string;
	updated_at?: string;
	tags?: string[] | null;
	requesting_user_email?: string | null;
	pull_request?: { url?: string } | null;
}

function formatRow(s: SessionRow): string {
	const parts = [
		s.session_id ?? "?",
		s.title || "(无标题)",
		`status=${s.status ?? "?"}${s.status_enum ? `(${s.status_enum})` : ""}`,
	];
	if (s.created_at) parts.push(`created=${s.created_at}`);
	if (s.updated_at) parts.push(`updated=${s.updated_at}`);
	if (s.tags?.length) parts.push(`tags=${s.tags.join(",")}`);
	if (s.requesting_user_email) parts.push(`user=${s.requesting_user_email}`);
	if (s.pull_request?.url) parts.push(`pr=${s.pull_request.url}`);
	return `- ${parts.join(" | ")}`;
}

export default function (pi: ExtensionAPI) {
	pi.registerTool({
		name: "devin_session_search",
		label: "Devin Session Search",
		description:
			"Search Devin sessions of this organization via the Devin REST API. " +
			"Returns one line per session: session_id, title, status, timestamps, tags, PR url. " +
			"Requires DEVIN_API_KEY in the environment.",
		promptSnippet: "Search/list Devin sessions by tags, creator email, or recency",
		promptGuidelines: [
			"Use devin_session_search to find Devin sessions (e.g. by tag or creator email), then devin_session_get on a session_id for its full details.",
			"If the tool reports a missing DEVIN_API_KEY, tell the user to set it as an environment variable before launching WarDex.",
		],
		parameters: Type.Object({
			tags: Type.Optional(
				Type.Array(Type.String(), { description: "Only sessions carrying all of these tags" }),
			),
			user_email: Type.Optional(
				Type.String({ description: "Filter by creator email address" }),
			),
			limit: Type.Optional(Type.Number({ description: "Max results, default 20, max 100" })),
			offset: Type.Optional(Type.Number({ description: "Pagination offset, default 0" })),
		}),
		async execute(_id, params) {
			const qs = new URLSearchParams();
			const limit = Math.min(Math.max(Math.trunc(params.limit ?? 20) || 20, 1), 100);
			qs.set("limit", String(limit));
			const offset = Math.max(Math.trunc(params.offset ?? 0) || 0, 0);
			if (offset > 0) qs.set("offset", String(offset));
			for (const t of params.tags ?? []) {
				if (t.trim()) qs.append("tags", t.trim());
			}
			if (params.user_email?.trim()) qs.set("user_email", params.user_email.trim());

			const r = await devinFetch(`/v1/sessions?${qs}`);
			if ("content" in r) return r;
			if (!r.ok) return textResult(`devin_session_search 失败：${r.body}`);

			const data = r.json as { sessions?: SessionRow[] };
			const rows = Array.isArray(data?.sessions) ? data.sessions : [];
			if (rows.length === 0) {
				return textResult("没有匹配的 Devin 会话。");
			}
			const lines = rows.map(formatRow);
			if (rows.length === limit) {
				lines.push(`（可能还有更多：用 offset=${offset + limit} 翻下一页）`);
			}
			return textResult(lines.join("\n"));
		},
	});

	pi.registerTool({
		name: "devin_session_get",
		label: "Devin Session Get",
		description:
			"Get details of one Devin session by session_id (with or without the devin- prefix). " +
			"Returns the raw session JSON (status, title, tags, PR, etc). Requires DEVIN_API_KEY.",
		promptSnippet: "Fetch full details of one Devin session",
		parameters: Type.Object({
			session_id: Type.String({ description: "Devin session id, e.g. devin-abc123 or abc123" }),
		}),
		async execute(_id, params) {
			const id = params.session_id.trim().replace(/^devin-/, "");
			if (!id) return textResult("session_id 不能为空");
			const r = await devinFetch(`/v1/session/${encodeURIComponent(id)}`);
			if ("content" in r) return r;
			if (!r.ok) return textResult(`devin_session_get 失败：${r.body}`);
			return textResult(truncate(JSON.stringify(r.json, null, 2)));
		},
	});
}

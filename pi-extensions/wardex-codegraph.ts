/**
 * WarDex codegraph tools for the embedded Pi agent.
 *
 * Replaces the ACP codegraph MCP server: shells out to the local `codegraph`
 * CLI against <project>/.codegraph/graph.db. Project dir comes from
 * WARDEX_PROJECT_DIR (spawn env) or ctx.cwd.
 */

import * as fs from "node:fs";
import * as path from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

function env(name: string): string {
	return (process.env[name] ?? "").trim();
}

function projectDir(ctx: ExtensionContext | undefined): string {
	const fromEnv = env("WARDEX_PROJECT_DIR");
	if (fromEnv) return fromEnv;
	return (ctx?.cwd ?? "").trim();
}

function dbPath(proj: string): string {
	return path.join(proj, ".codegraph", "graph.db");
}

function indexExists(proj: string): boolean {
	try {
		return fs.statSync(dbPath(proj)).isFile();
	} catch {
		return false;
	}
}

async function runCodegraph(
	ctx: ExtensionContext,
	args: string[],
	cwd: string,
): Promise<{ stdout: string; stderr: string; code: number }> {
	const isWin = process.platform === "win32";
	const result = isWin
		? await ctx.exec("cmd.exe", ["/d", "/s", "/c", "codegraph", ...args], { cwd, timeout: 120_000 })
		: await ctx.exec("codegraph", args, { cwd, timeout: 120_000 });
	return { stdout: result.stdout ?? "", stderr: result.stderr ?? "", code: result.code };
}

function textResult(text: string) {
	return { content: [{ type: "text" as const, text }] };
}

export default function (pi: ExtensionAPI) {
	pi.registerTool({
		name: "codegraph_status",
		label: "Codegraph Status",
		description: "Check whether the codegraph CLI is installed and whether this project has an index (.codegraph/graph.db).",
		promptSnippet: "Check codegraph install and index status for this project",
		parameters: Type.Object({}),
		async execute(_id, _params, _signal, _onUpdate, ctx) {
			const proj = projectDir(ctx);
			if (!proj) {
				return textResult("当前会话没有项目目录，无法使用 codegraph。");
			}
			const version = await runCodegraph(ctx, ["--version"], proj);
			const installed = version.code === 0;
			const indexed = indexExists(proj);
			const ver = (version.stdout || version.stderr).trim().split(/\r?\n/)[0] ?? "";
			const lines = [
				`installed: ${installed}${installed && ver ? ` (${ver})` : ""}`,
				`index: ${indexed ? dbPath(proj) : "missing"}`,
			];
			if (!installed) {
				lines.push("codegraph 未安装。需要 Node ≥ 22.6：npm install -g @optave/codegraph");
			} else if (!indexed) {
				lines.push("尚未构建索引。请在 WarDex 用 Ctrl+\\ 构建，或让用户运行：codegraph build <项目目录>");
			}
			return textResult(lines.join("\n"));
		},
	});

	pi.registerTool({
		name: "codegraph_query",
		label: "Codegraph Query",
		description:
			"Query the project's codegraph index for a symbol. Returns JSON hits (name, kind, file, line). Requires a built index.",
		promptSnippet: "Look up symbols, interfaces, or functions in the codegraph index",
		promptGuidelines: [
			"Use codegraph_query for code-structure questions (who implements X, where is Y defined) instead of grepping blindly when an index exists.",
			"If codegraph_status reports no index, tell the user to build it (WarDex Ctrl+\\ or `codegraph build`).",
		],
		parameters: Type.Object({
			name: Type.String({ description: "Symbol name to search (partial match ok)" }),
			kind: Type.Optional(Type.String({ description: "Optional kind filter, e.g. interface, function, method, struct" })),
			limit: Type.Optional(Type.Number({ description: "Max hits (default 40)" })),
		}),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			const proj = projectDir(ctx);
			if (!proj) {
				return textResult("当前会话没有项目目录，无法使用 codegraph。");
			}
			if (!indexExists(proj)) {
				return textResult("尚未构建索引。请先 codegraph_status，或请用户构建索引（Ctrl+\\ / `codegraph build`）。");
			}
			const name = (params.name ?? "").trim();
			if (!name) {
				return textResult("name 不能为空");
			}
			const limit = Math.max(1, Math.min(100, Math.floor(params.limit ?? 40)));
			const args = ["query", name, "--json", "-n", String(limit), "-d", dbPath(proj)];
			const kind = (params.kind ?? "").trim();
			if (kind) {
				args.push("--kind", kind);
			}
			const r = await runCodegraph(ctx, args, proj);
			if (r.code !== 0) {
				const err = (r.stderr || r.stdout).trim() || `codegraph query exited ${r.code}`;
				return textResult(err);
			}
			return textResult(r.stdout.trim() || "(no results)");
		},
	});

	pi.registerTool({
		name: "codegraph_where",
		label: "Codegraph Where",
		description: "Find where a symbol is defined and used (fast lookup). Requires a built index.",
		parameters: Type.Object({
			name: Type.String({ description: "Symbol name" }),
			limit: Type.Optional(Type.Number({ description: "Max hits (default 40)" })),
		}),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			const proj = projectDir(ctx);
			if (!proj) {
				return textResult("当前会话没有项目目录，无法使用 codegraph。");
			}
			if (!indexExists(proj)) {
				return textResult("尚未构建索引。请先 codegraph_status，或请用户构建索引。");
			}
			const name = (params.name ?? "").trim();
			if (!name) {
				return textResult("name 不能为空");
			}
			const limit = Math.max(1, Math.min(100, Math.floor(params.limit ?? 40)));
			const r = await runCodegraph(
				ctx,
				["where", name, "-j", "-n", String(limit), "-d", dbPath(proj)],
				proj,
			);
			if (r.code !== 0) {
				const err = (r.stderr || r.stdout).trim() || `codegraph where exited ${r.code}`;
				return textResult(err);
			}
			return textResult(r.stdout.trim() || "(no results)");
		},
	});
}

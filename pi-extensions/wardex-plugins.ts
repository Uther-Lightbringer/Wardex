/**
 * WarDex plugin manager (插件化改造 P1) — lets the model create / inspect /
 * edit / toggle WarDex plugins during a conversation.
 *
 * Sandbox: every file tool is rooted at WARDEX_PLUGINS_DIR (injected by the
 * WarDex runtime at spawn, <data root>/wardex-plugins). Paths escaping that
 * root are rejected. Built-in extensions (reminders/codegraph) cannot be
 * touched — they live outside the sandbox root.
 *
 * Changes only take effect for NEW spawns; the user applies them to live
 * sessions with 设置 → 插件 → 生效 (which restarts idle runtimes).
 */

import * as fs from "node:fs";
import * as path from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

function pluginsRoot(): string {
	return (process.env["WARDEX_PLUGINS_DIR"] ?? "").trim();
}

function textResult(text: string) {
	return { content: [{ type: "text" as const, text }] };
}

/** Resolve <root>/<id>/<rel> refusing traversal outside the root. */
function safeJoin(root: string, id: string, rel: string): string {
	const cleanId = path.basename(id.trim());
	if (!cleanId || cleanId.startsWith(".")) {
		throw new Error(`非法插件 id: ${id}`);
	}
	const target = path.resolve(root, cleanId, rel.trim());
	if (!target.startsWith(path.resolve(root) + path.sep)) {
		throw new Error(`路径越界: ${rel}`);
	}
	if (/(^|[\\/])\.\.([\\/]|$)/.test(rel)) {
		throw new Error(`路径越界: ${rel}`);
	}
	return target;
}

interface PluginManifest {
	name?: string;
	version?: string;
	entry?: string;
	ui?: string;
}

function readRegistry(root: string): Record<string, boolean> {
	try {
		const raw = JSON.parse(fs.readFileSync(path.join(root, "registry.json"), "utf8")) as {
			enabled?: Record<string, boolean>;
		};
		return raw.enabled ?? {};
	} catch {
		return {};
	}
}

function writeRegistry(root: string, enabled: Record<string, boolean>): void {
	fs.mkdirSync(root, { recursive: true });
	fs.writeFileSync(
		path.join(root, "registry.json"),
		JSON.stringify({ version: 1, enabled }, null, 2),
		"utf8",
	);
}

function listPlugins(root: string): string {
	if (!root) return "WARDEX_PLUGINS_DIR 未设置（旧版 WarDex？），插件管理不可用。";
	const reg = readRegistry(root);
	const lines: string[] = [];
	lines.push("内置插件：");
	for (const [id, name] of [
		["reminders", "会话提醒"],
		["codegraph", "Codegraph 代码索引"],
	] as const) {
		lines.push(`  - ${id} (${name}) enabled=${reg[id] ?? true}`);
	}
	lines.push("用户插件（<root>/<id>/plugin.json）：");
	let any = false;
	if (fs.existsSync(root)) {
		for (const dir of fs.readdirSync(root, { withFileTypes: true })) {
			if (!dir.isDirectory()) continue;
			const manifestPath = path.join(root, dir.name, "plugin.json");
			if (!fs.existsSync(manifestPath)) continue;
			any = true;
			let m: PluginManifest = {};
			try {
				m = JSON.parse(fs.readFileSync(manifestPath, "utf8")) as PluginManifest;
			} catch {
				lines.push(`  - ${dir.name}: plugin.json 解析失败！`);
				continue;
			}
			lines.push(
				`  - ${dir.name} "${m.name ?? dir.name}" v${m.version ?? "0.0.0"} ` +
					`entry=${m.entry ?? "-"} ui=${m.ui ?? "-"} enabled=${reg[dir.name] ?? true}`,
			);
		}
	}
	if (!any) lines.push("  （暂无用户插件）");
	lines.push("");
	lines.push('提示：修改后需请用户在 设置→插件 点击「生效」才会应用到运行中的会话。');
	return lines.join("\n");
}

export default function (pi: ExtensionAPI) {
	// Shared authoring rules injected into every tool's description context.
	// These encode WarDex host constraints so plugins work on the first try:
	// - panel.html is rendered via iframe srcdoc → it MUST be one
	//   self-contained file (inline CSS/JS, no <script src>/<link>/images by
	//   relative path — nothing else resolves).
	// - The ONLY host capabilities are the postMessage bridge (see below);
	//   no Tauri IPC, no Node, no fetch to local files.
	// - Transparent background: the panel sits on the app's theme surface, so
	//   prefer background: transparent and inherit colors, or provide your own
	//   full-bleed styling deliberately.
	const PANEL_GUIDELINES = [
		"panel.html 必须是单文件自包含：CSS/JS 全部内联，不要引用任何外部文件（<script src>、<link>、相对路径图片都不会加载）。",
		"与宿主通信只能用 postMessage 桥：发 { source:'wardex-plugin', type:'ready'|'notify'|'sendPrompt', text? }，收 { source:'wardex-host', type:'info', payload:{ sessionId, projectDir } }。没有其它宿主能力（无 Tauri IPC / 文件 / 网络）。",
	"宿主会自动捕获面板的 console.error/warn 和未捕获异常并记录到运行日志；调试面板问题时用 plugin_logs 工具读取，不要让面板自己实现日志上报。",
	"plugin.json 可用 \"surface\" 字段选择面板形态：\"drawer\"(默认，右侧抽屉) / \"dialog\"(侧栏按钮弹出独立浮窗，适合大画布/表单类) / \"both\"(默认抽屉，面板可通过桥发 {source:'wardex-plugin',type:'window',op:'open'|'close'} 在运行时切换到弹窗)。",
	"plugin.json 可用 \"data\":{\"scope\":...} 声明数据作用域：\"session\"(随会话内存) / \"project\"(默认，存 <项目>/.wardex/) / \"global\”(随 WarDex 全局)。面板通过桥发 {source:'wardex-plugin',type:'storage',reqId,op:'get'|'set'|'remove',key,value?} 读写；宿主会回 {source:'wardex-host',type:'storage',reqId,ok,value?,error?}。作用域由清单声明决定，面板运行时不可更改。",
		"面板背景建议透明（body{background:transparent}）以融入主题；需要自带配色时明确铺满整个 body。",
	].join("\n");
	pi.registerTool({
		name: "plugins_list",
		label: "List Plugins",
		description:
			"List all WarDex plugins (builtin + user), their entry files and enabled state.",
		promptSnippet: "list WarDex plugins",
		parameters: Type.Object({}),
		async execute() {
			return textResult(listPlugins(pluginsRoot()));
		},
	});

	pi.registerTool({
		name: "plugin_read",
		label: "Read Plugin File",
		description:
			"Read one file of a user plugin. Path is relative to the plugin directory, e.g. 'main.ts' or 'plugin.json'.",
		parameters: Type.Object({
			id: Type.String({ description: "Plugin id (= its directory name)" }),
			file: Type.String({ description: "Relative file path inside the plugin dir" }),
		}),
		async execute(_id, params) {
			const root = pluginsRoot();
			if (!root) return textResult("WARDEX_PLUGINS_DIR 未设置。");
			try {
				const p = safeJoin(root, params.id, params.file);
				if (!fs.existsSync(p)) return textResult(`文件不存在: ${params.id}/${params.file}`);
				return textResult(fs.readFileSync(p, "utf8").slice(0, 60_000));
			} catch (e) {
				return textResult(String(e));
			}
		},
	});

	pi.registerTool({
		name: "plugin_write",
		label: "Write Plugin Files",
		description:
			"Create or update a user plugin: writes files under wardex-plugins/<id>/ and ensures plugin.json exists. " +
			"An pi-extension entry must be a .ts module default-exporting an ExtensionAPI init function. " +
			"A UI panel is an html file referenced by plugin.json's 'ui' field.",
		promptGuidelines: [
			"When asked to add/change a WarDex tool or sidebar panel, use plugin_write to edit the plugin files, then remind the user to press 生效 in 设置→插件.",
			"Always keep plugin.json in sync: {\"name\", \"version\", \"entry\": \"main.ts\", \"ui\": \"panel.html\"?}. Bump version on every change.",
			`UI 面板编写规范（必须遵守）：\n${PANEL_GUIDELINES}`,
		],
		parameters: Type.Object({
			id: Type.String({ description: "Plugin id (= directory name, ascii/中文均可但不能含路径分隔符)" }),
			files: Type.Array(
				Type.Object({
					path: Type.String({ description: "Relative path, e.g. main.ts / plugin.json / panel.html" }),
					content: Type.String({ description: "Full new file content" }),
				}),
				{ description: "Files to write (whole-file overwrite)" },
			),
			enabled: Type.Optional(Type.Boolean({ description: "Set the plugin's enabled flag too (default: leave unchanged; new plugins default to enabled)" })),
		}),
		async execute(_id, params) {
			const root = pluginsRoot();
			if (!root) return textResult("WARDEX_PLUGINS_DIR 未设置。");
			const written: string[] = [];
			try {
				for (const f of params.files ?? []) {
					const p = safeJoin(root, params.id, f.path);
					fs.mkdirSync(path.dirname(p), { recursive: true });
					fs.writeFileSync(p, f.content, "utf8");
					written.push(f.path);
				}
				// Ensure a valid manifest even if the model forgot it.
				const dir = path.join(root, path.basename(params.id.trim()));
				const manifestPath = path.join(dir, "plugin.json");
				let m: PluginManifest = {};
				if (fs.existsSync(manifestPath)) {
					try {
						m = JSON.parse(fs.readFileSync(manifestPath, "utf8")) as PluginManifest;
					} catch {
						m = {};
					}
				}
				if (!m.entry && !m.ui) {
					m.name = m.name ?? params.id;
					m.version = m.version ?? "0.1.0";
					m.entry = written.find((w) => w.endsWith(".ts") && w !== "plugin.json") ?? "";
					m.ui = written.find((w) => w.endsWith(".html")) ?? "";
					fs.writeFileSync(manifestPath, JSON.stringify(m, null, 2), "utf8");
					written.push("plugin.json (自动补全)");
				}
				if (typeof params.enabled === "boolean") {
					const reg = readRegistry(root);
					reg[path.basename(params.id.trim())] = params.enabled;
					writeRegistry(root, reg);
					written.push(`enabled=${params.enabled}`);
				}
				// Lint UI panels: srcdoc rendering means external references
				// silently 404 — catch it at write time instead of a blank panel.
				const warnings: string[] = [];
				for (const f of params.files ?? []) {
					if (!f.path.endsWith(".html")) continue;
					const ext = /<(script[^>]+src|link[^>]+href)\s*=\s*["']([^"']+)["']/i.exec(f.content);
					if (ext) warnings.push(`${f.path} 引用了外部资源 ${ext[2]} —— 面板是 srcdoc 单文件渲染，外部引用不会加载，请内联。`);
				}
				return textResult(
					`已写入 ${params.id}: ${written.join(", ")}。\n` +
						(warnings.length ? "⚠ 规范警告：\n- " + warnings.join("\n- ") + "\n" : "") +
						"注意：需要用户在 设置→插件 点击「生效」后，运行中的会话才会加载新版本。",
				);
			} catch (e) {
				return textResult(`写入失败: ${String(e)}（已写入: ${written.join(", ") || "无"}）`);
			}
		},
	});

	pi.registerTool({
		name: "plugin_delete",
		label: "Delete Plugin",
		description: "Delete a user plugin directory entirely. Cannot touch builtins.",
		parameters: Type.Object({
			id: Type.String({ description: "Plugin id (= directory name)" }),
		}),
		async execute(_id, params) {
			const root = pluginsRoot();
			if (!root) return textResult("WARDEX_PLUGINS_DIR 未设置。");
			try {
				const dir = path.join(root, path.basename(params.id.trim()));
				if (!fs.existsSync(dir)) return textResult(`插件不存在: ${params.id}`);
				fs.rmSync(dir, { recursive: true });
				const reg = readRegistry(root);
				delete reg[path.basename(params.id.trim())];
				writeRegistry(root, reg);
				return textResult(`已删除 ${params.id}。设置页点「生效」后从运行中的会话卸载。`);
			} catch (e) {
				return textResult(String(e));
			}
		},
	});

	pi.registerTool({
		name: "plugin_set_enabled",
		label: "Toggle Plugin",
		description: "Enable/disable a plugin by editing registry.json (applies after 生效).",
		parameters: Type.Object({
			id: Type.String(),
			enabled: Type.Boolean(),
		}),
		async execute(_id, params) {
			const root = pluginsRoot();
			if (!root) return textResult("WARDEX_PLUGINS_DIR 未设置。");
			if (params.id === "plugins") return textResult("插件管理员不能停用。");
			try {
				const reg = readRegistry(root);
				reg[params.id] = params.enabled;
				writeRegistry(root, reg);
				return textResult(`${params.id} → enabled=${params.enabled}（生效按钮应用）。`);
			} catch (e) {
				return textResult(String(e));
			}
		},
	});

	pi.registerTool({
		name: "plugin_apply",
		label: "Apply Plugin Changes",
		description:
			"Ask the user to apply pending plugin changes NOW (restarts idle sessions' pi process; conversation context is preserved). " +
			"Call this after writing/toggling plugins when the user wants changes live immediately. Shows a confirmation dialog first.",
		promptGuidelines: [
			"After plugin_write/plugin_set_enabled/plugin_delete, call plugin_apply if the user asked for the change to take effect immediately; otherwise just remind them about the 生效 button.",
		],
		parameters: Type.Object({}),
		async execute(_id, _params, _signal, _onUpdate, ctx) {
			if (!ctx.hasUI) {
				return textResult("当前无 UI 环境，无法弹确认框；请用户在 设置→插件 手动点「生效」。",		);
			}
			// Title sentinel "[plugins.apply]" — WarDex intercepts this confirm,
			// shows its permission dialog, and on approval applies the change
			// host-side (restarts idle runtimes) right after answering.
			const ok = await ctx.ui.confirm(
				"[plugins.apply] 应用插件变更",
				"将重启空闲会话以加载最新插件（对话上下文保留，忙碌会话跳过）。允许吗？",
			);
			return ok
				? textResult("用户已允许：插件变更正在应用（空闲会话已重启加载新版本）。")
				: textResult("用户拒绝了本次生效；变更仍处于待生效状态，用户可稍后在 设置→插件 手动点「生效」。",		);
		},
	});

	// Runtime observability: UI panels' console output + uncaught errors are
	// captured by the host iframe bridge and persisted to .logs/<id>.log.
	// This tool closes the debug loop — the model can read what its panel
	// actually printed / threw and fix its own code.
	pi.registerTool({
		name: "plugin_logs",
		label: "Read Plugin Runtime Logs",
		description:
			"Read the runtime log of a UI plugin (console.warn/error + uncaught errors captured from its sidebar panel). " +
			"Without id: list plugins that have logs. Use this FIRST when a user reports a panel misbehaving or showing errors.",
		promptGuidelines: [
			"当用户报告面板空白/报错/行为异常时，先调用 plugin_logs 读取运行日志定位问题，再 plugin_read 看代码、plugin_write 修复。",
		],
		parameters: Type.Object({
			id: Type.Optional(Type.String({ description: "Plugin id (= its directory name)" })),
		}),
		async execute(_id, params) {
			const root = pluginsRoot();
			if (!root) return textResult("WARDEX_PLUGINS_DIR 未设置。");
			const logsDir = path.join(root, ".logs");
			try {
				if (!params.id || !params.id.trim()) {
					if (!fs.existsSync(logsDir)) return textResult("暂无任何插件运行日志。");
					const files = fs
						.readdirSync(logsDir)
						.filter((f) => f.endsWith(".log"))
						.map((f) => {
							const st = fs.statSync(path.join(logsDir, f));
							return `  ${f.replace(/\.log$/, "")}  (${st.size} bytes, ${st.mtime.toISOString()})`;
						});
					return textResult(
						files.length ? `有运行日志的插件：\n${files.join("\n")}` : "暂无任何插件运行日志。",
					);
				}
				const target = path.join(logsDir, `${path.basename(params.id.trim())}.log`);
				if (!target.startsWith(path.resolve(logsDir))) return textResult("非法插件 id。");
				if (!fs.existsSync(target)) return textResult(`插件 ${params.id} 暂无运行日志（面板可能从未报错或未打开过）。`);
				const stat = fs.statSync(target);
				let text = fs.readFileSync(target, "utf8");
				if (stat.size > 16_000) text = "…(仅末尾)\n" + text.slice(-16_000);
				return textResult(`[plugin_logs ${params.id}]\n${text}`);
			} catch (e) {
				return textResult(String(e));
			}
		},
	});
}

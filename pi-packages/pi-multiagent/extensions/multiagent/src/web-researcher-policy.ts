/** Package:web-researcher callable web-tool planning contract. */

import type { AgentDiagnostic } from "./types.ts";

const SEARCH_TOOLS = new Set(["exa_search"]);
const FETCH_TOOLS = new Set(["exa_fetch"]);

export function validateWebResearcherExtensionTools(ref: string, extensionTools: { name: string }[], diagnostics: AgentDiagnostic[], path: string): boolean {
	if (ref !== "package:web-researcher") return true;
	const names = new Set(extensionTools.map((tool) => tool.name));
	const hasSearch = [...SEARCH_TOOLS].some((name) => names.has(name));
	const hasFetch = [...FETCH_TOOLS].some((name) => names.has(name));
	if (hasSearch && hasFetch) return true;
	diagnostics.push({
		code: "web-researcher-extension-tools-required",
		message: "package:web-researcher requires explicit callable exa_search and exa_fetch extensionTools plus graph.authority.allowExtensionCode:true; copy exact provenance from agent_team catalog before start.",
		severity: "error",
		path,
	});
	return false;
}

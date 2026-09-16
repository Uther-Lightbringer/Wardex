/** Built-in child tool profile expansion and authority caps. */

import type { AgentDiagnostic, GraphAuthority } from "./types.ts";
import { builtinToolAllowedByAuthority } from "./authority-policy.ts";
import { MUTATION_CHILD_TOOL_NAMES, READONLY_CHILD_TOOL_NAMES, SHELL_CHILD_TOOL_NAMES } from "./types.ts";
import { validateBuiltinToolNames } from "./tool-policy.ts";

const READONLY_TOOLS = new Set<string>(READONLY_CHILD_TOOL_NAMES);
const SHELL_TOOLS = new Set<string>(SHELL_CHILD_TOOL_NAMES);
const MUTATION_TOOLS = new Set<string>(MUTATION_CHILD_TOOL_NAMES);

export function resolveBuiltinToolProfile(input: {
	tools: string[] | undefined;
	explicit: boolean;
	authority: GraphAuthority;
	label: string;
	path: string;
	diagnostics: AgentDiagnostic[];
}): string[] | undefined {
	const provided = input.tools ?? [];
	if (!validateBuiltinToolNames(provided, input.label, input.diagnostics, input.path)) return undefined;
	const requested = withMandatoryRead(provided);
	const expanded = expandBuiltinToolProfile(requested);
	if (!input.explicit) return capInheritedTools(expanded, input);
	let valid = true;
	if (expanded.some((tool) => READONLY_TOOLS.has(tool)) && !input.authority.allowFilesystemRead) {
		input.diagnostics.push(makeDiagnostic("filesystem-read-authority-required", `The filesystem read/discovery suite (${READONLY_CHILD_TOOL_NAMES.join(", ")}) requires graph.authority.allowFilesystemRead:true.`, input.path));
		valid = false;
	}
	if (expanded.some((tool) => SHELL_TOOLS.has(tool)) && !input.authority.allowShellTools) {
		input.diagnostics.push(makeDiagnostic("shell-authority-required", "bash requires graph.authority.allowShellTools:true.", input.path));
		valid = false;
	}
	if (expanded.some((tool) => MUTATION_TOOLS.has(tool)) && !input.authority.allowMutationTools) {
		input.diagnostics.push(makeDiagnostic("mutation-authority-required", "edit and write require graph.authority.allowMutationTools:true.", input.path));
		valid = false;
	}
	return valid ? expanded : undefined;
}

function withMandatoryRead(tools: string[]): string[] {
	return tools.some((tool) => READONLY_TOOLS.has(tool)) ? tools : ["read", ...tools];
}

function expandBuiltinToolProfile(tools: string[]): string[] {
	const expanded: string[] = [];
	if (tools.some((tool) => READONLY_TOOLS.has(tool))) expanded.push(...READONLY_CHILD_TOOL_NAMES);
	for (const tool of tools) if (!READONLY_TOOLS.has(tool) && !expanded.includes(tool)) expanded.push(tool);
	return expanded;
}

function capInheritedTools(expanded: string[], input: { authority: GraphAuthority; label: string; path: string; diagnostics: AgentDiagnostic[] }): string[] | undefined {
	const effective = expanded.filter((tool) => builtinToolAllowedByAuthority(tool, input.authority));
	const denied = expanded.filter((tool) => !effective.includes(tool));
	if (expanded.length > 0 && effective.length === 0) {
		input.diagnostics.push(makeDiagnostic("catalog-default-tools-denied", `${input.label} defaultTools were fully denied by graph authority; every child agent keeps at least the read/discovery suite, so grant graph.authority.allowFilesystemRead:true.`, input.path));
		return undefined;
	}
	if (denied.some((tool) => READONLY_TOOLS.has(tool))) {
		input.diagnostics.push(makeDiagnostic("filesystem-read-authority-required", `${input.label} defaultTools lost mandatory read/discovery during authority capping; every child requires graph.authority.allowFilesystemRead:true.`, input.path));
		return undefined;
	}
	if (denied.length > 0) input.diagnostics.push(makeWarning("catalog-default-tools-capped", `${input.label} defaultTools were capped by graph authority; denied=${denied.join(",")}; effectiveTools=${effective.join(",")}; grant allowFilesystemRead, allowShellTools, or allowMutationTools only when intended.`, input.path));
	return effective;
}

function makeWarning(code: string, message: string, path: string): AgentDiagnostic {
	return { code, message, path, severity: "warning" };
}

function makeDiagnostic(code: string, message: string, path: string): AgentDiagnostic {
	return { code, message, path, severity: "error" };
}

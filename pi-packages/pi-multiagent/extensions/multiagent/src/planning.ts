/** Detached graph validation and resolved-run materialization. */

import { createHash } from "node:crypto";
import { lstatSync, realpathSync, statSync } from "node:fs";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import type { GraphSpec } from "./schemas.ts";
import type { AgentConfig, AgentDiagnostic, CwdIdentity, GraphAuthority, LibrarySource, ParentSkillInventory, ResolvedAgent, ResolvedGraph, SubagentSkillMode, TeamStepSpec } from "./types.ts";
import { DEFAULT_GRAPH_LIBRARY_SOURCES, LIBRARY_SOURCE_VALUES, PUBLIC_ID_PATTERN, SOURCE_QUALIFIED_LIBRARY_REF_PATTERN } from "./types.ts";
import { createCallerSkillResolutionContext, resolveAgentCallerSkills } from "./caller-skills.ts";
import { normalizeAuthority } from "./authority-policy.ts";
import { normalizeLimits, normalizeStartOptions } from "./limits.ts";
import { resolveAgentToolAccess } from "./tool-policy.ts";
import { resolveBuiltinToolProfile } from "./builtin-tool-profile.ts";
import { DEFAULT_SUBAGENT_SKILL_MODE } from "./subagent-skills-config.ts";
import { findProjectSettingsFile } from "./project-settings.ts";
import { validateWebResearcherExtensionTools } from "./web-researcher-policy.ts";
import { normalizeStepAgentOverrides } from "./step-agent-overrides.ts";
import { normalizeStepOutputLimit } from "./step-output-limit.ts";

const PUBLIC_ID_REGEX = new RegExp(PUBLIC_ID_PATTERN);
const SOURCE_REF_REGEX = new RegExp(SOURCE_QUALIFIED_LIBRARY_REF_PATTERN);

export interface ResolveGraphContext {
	parentTools: import("./types.ts").ParentToolInventory;
	parentSkills?: ParentSkillInventory;
	subagentSkillMode?: SubagentSkillMode;
	cwd: string;
	invocationCwd: string;
}

export function resolveDetachedGraph(graph: GraphSpec, libraryAgents: AgentConfig[], baseDiagnostics: AgentDiagnostic[], context: ResolveGraphContext, rawOptions: Parameters<typeof normalizeStartOptions>[0]): ResolvedGraph {
	const diagnostics = [...baseDiagnostics];
	if (graph.objective.trim().length === 0) diagnostics.push(makeDiagnostic("graph-objective-required", "Graph objective must contain non-whitespace text.", "error", "/graph/objective"));
	const authority = normalizeAuthority(graph.authority);
	const library = { sources: normalizeGraphSources(graph.library?.sources, authority, diagnostics) };
	const limits = normalizeLimits(graph);
	const options = normalizeStartOptions(rawOptions);
	const invocationCwd = resolveInvocationCwd(context.invocationCwd, diagnostics);
	const skillContext = createCallerSkillResolutionContext(context.parentSkills, invocationCwd);
	const subagentSkillMode = context.subagentSkillMode ?? DEFAULT_SUBAGENT_SKILL_MODE;
	const steps = graph.steps.map((step, index) => resolveStep(step, index, authority, library, libraryAgents, diagnostics, { ...context, invocationCwd, subagentSkillMode }, skillContext)).filter((step): step is TeamStepSpec => step !== undefined);
	validateStepGraph(steps, diagnostics);
	return { objective: graph.objective.trim(), library, authority, steps, limits, options, graphHash: hashGraph(graph, authority, limits, options, subagentSkillMode), diagnostics };
}

export { validatePreflightShape } from "./preflight-shape.ts";

function resolveStep(step: GraphSpec["steps"][number], index: number, authority: GraphAuthority, library: { sources: LibrarySource[] }, libraryAgents: AgentConfig[], diagnostics: AgentDiagnostic[], ctx: ResolveGraphContext, skillContext: ReturnType<typeof createCallerSkillResolutionContext>): TeamStepSpec | undefined {
	const path = `/graph/steps/${index}`;
	if (!validatePublicId(step.id, `step id ${step.id || "<empty>"}`, diagnostics, `${path}/id`)) return undefined;
	if (!step.task.trim()) diagnostics.push(makeDiagnostic("step-task-required", `Step ${step.id} requires task.`, "error", `${path}/task`));
	const cwd = resolveStepCwd(ctx.invocationCwd, step.cwd, diagnostics, `${path}/cwd`);
	const agent = resolveStepAgent(step.id, step.agent, authority, library, libraryAgents, diagnostics, ctx, skillContext, `${path}/agent`);
	const overrides = normalizeStepAgentOverrides(step.agent, diagnostics, `${path}/agent`);
	const outputLimit = normalizeStepOutputLimit(step.outputLimit, diagnostics, `${path}/outputLimit`);
	const cwdSettingsValid = cwd && agent ? validateBashCwd(step.id, agent, cwd.path, diagnostics, `${path}/cwd`) : false;
	for (const [needIndex, need] of (step.needs ?? []).entries()) validatePublicId(need, `strict dependency ${need || "<empty>"}`, diagnostics, `${path}/needs/${needIndex}`);
	for (const [afterIndex, after] of (step.after ?? []).entries()) validatePublicId(after, `terminal dependency ${after || "<empty>"}`, diagnostics, `${path}/after/${afterIndex}`);
	if (!cwd || !agent || !overrides || !outputLimit || !cwdSettingsValid) return undefined;
	return { id: step.id, agent: { ...agent, model: overrides.model ?? agent.model, thinking: overrides.thinking ?? agent.thinking }, task: step.task, needs: dedupeRefs(step.needs ?? []), after: dedupeRefs(step.after ?? []), cwd: cwd.path, cwdIdentity: cwd.identity, outputLimit };
}

function resolveStepAgent(stepId: string, spec: GraphSpec["steps"][number]["agent"], authority: GraphAuthority, library: { sources: LibrarySource[] }, libraryAgents: AgentConfig[], diagnostics: AgentDiagnostic[], context: ResolveGraphContext, skillContext: ReturnType<typeof createCallerSkillResolutionContext>, path: string): ResolvedAgent | undefined {
	if ((spec.extensionTools?.length ?? 0) > 0 && !authority.allowExtensionCode) {
		diagnostics.push(makeDiagnostic("extension-code-authority-required", "extensionTools require graph.authority.allowExtensionCode:true.", "error", `${path}/extensionTools`));
		return undefined;
	}
	const ref = spec.ref;
	const system = spec.system;
	if (ref !== undefined && system !== undefined) {
		diagnostics.push(makeDiagnostic("step-agent-binding-exclusive", "Step agent must set exactly one of system or ref.", "error", path));
		return undefined;
	}
	if (ref !== undefined) return resolveLibraryAgent(stepId, { ...spec, ref }, authority, library, libraryAgents, diagnostics, context, skillContext, path);
	if (system !== undefined) {
		const systemPrompt = system.trim();
		if (systemPrompt.length === 0) {
			diagnostics.push(makeDiagnostic("inline-system-required", `Inline agent ${stepId} requires non-whitespace system prompt text.`, "error", `${path}/system`));
			return undefined;
		}
		const tools = resolveBuiltinToolProfile({ tools: spec.tools, explicit: true, authority, label: `inline agent ${stepId}`, path: `${path}/tools`, diagnostics });
		if (!tools) return undefined;
		const toolAccess = resolveToolAccess(stepId, spec, tools, authority, diagnostics, context, path);
		if (!toolAccess) return undefined;
		const skills = resolveAgentCallerSkills({ mode: context.subagentSkillMode ?? DEFAULT_SUBAGENT_SKILL_MODE, tools: toolAccess.tools, label: `step agent ${stepId}`, path: `${path}/skills`, diagnostics, context: skillContext });
		if (!skills) return undefined;
		return { id: stepId, ref: `inline:${stepId}`, name: stepId, kind: "inline", description: stepId, tools: toolAccess.tools, extensionTools: toolAccess.extensionTools, callerSkills: skills, systemPrompt, model: undefined, thinking: undefined, source: "inline", filePath: undefined, sha256: undefined };
	}
	diagnostics.push(makeDiagnostic("step-agent-invalid", "Step agent requires either system or ref.", "error", path));
	return undefined;
}

function resolveLibraryAgent(stepId: string, spec: GraphSpec["steps"][number]["agent"] & { ref: string }, authority: GraphAuthority, library: { sources: LibrarySource[] }, libraryAgents: AgentConfig[], diagnostics: AgentDiagnostic[], context: ResolveGraphContext, skillContext: ReturnType<typeof createCallerSkillResolutionContext>, path: string): ResolvedAgent | undefined {
	const ref = spec.ref;
	if (!SOURCE_REF_REGEX.test(ref)) {
		diagnostics.push(makeDiagnostic("library-agent-ref-invalid", `Invalid library agent ref: ${ref}.`, "error", `${path}/ref`));
		return undefined;
	}
	const [sourceText, name] = ref.split(":");
	const source = sourceText === "package" || sourceText === "user" || sourceText === "project" ? sourceText : undefined;
	if (!source) {
		diagnostics.push(makeDiagnostic("library-agent-ref-invalid", `Invalid library agent ref: ${ref}.`, "error", `${path}/ref`));
		return undefined;
	}
	if (!library.sources.includes(source)) {
		diagnostics.push(makeDiagnostic("library-source-not-enabled", `Library source ${source} is not enabled by graph.library.sources.`, "error", `${path}/ref`));
		return undefined;
	}
	const agent = libraryAgents.find((candidate) => candidate.source === source && candidate.name === name);
	if (!agent) {
		diagnostics.push(makeDiagnostic("library-agent-unknown", `Unknown library agent: ${ref}. Run catalog or adjust graph.library.sources/authority.`, "error", `${path}/ref`));
		return undefined;
	}
	const tools = resolveBuiltinToolProfile({ tools: spec.tools ?? agent.tools, explicit: spec.tools !== undefined, authority, label: `library agent ${agent.ref}`, path: `${path}/tools`, diagnostics });
	if (!tools) return undefined;
	const toolAccess = resolveToolAccess(stepId, spec, tools, authority, diagnostics, context, path);
	if (!toolAccess) return undefined;
	if (!validatePackageRoleCapabilities(agent.ref, toolAccess.tools, diagnostics, path)) return undefined;
	if (!validateWebResearcherExtensionTools(agent.ref, toolAccess.extensionTools, diagnostics, `${path}/extensionTools`)) return undefined;
	const skills = resolveAgentCallerSkills({ mode: context.subagentSkillMode ?? DEFAULT_SUBAGENT_SKILL_MODE, tools: toolAccess.tools, label: `step agent ${stepId}`, path: `${path}/skills`, diagnostics, context: skillContext });
	if (!skills) return undefined;
	return { id: stepId, ref: agent.ref, name: agent.name, kind: "library", description: agent.description, tools: toolAccess.tools, extensionTools: toolAccess.extensionTools, callerSkills: skills, systemPrompt: agent.systemPrompt, model: agent.model, thinking: agent.thinking, source: agent.source, filePath: agent.filePath, sha256: agent.sha256 };
}

function resolveToolAccess(stepId: string, spec: GraphSpec["steps"][number]["agent"], tools: string[], authority: GraphAuthority, diagnostics: AgentDiagnostic[], context: ResolveGraphContext, path: string): ReturnType<typeof resolveAgentToolAccess> {
	return resolveAgentToolAccess({ tools, extensionTools: spec.extensionTools, label: `step agent ${stepId}`, toolsPath: `${path}/tools`, extensionToolsPath: `${path}/extensionTools`, diagnostics, context: { parentTools: context.parentTools } });
}

function validatePackageRoleCapabilities(ref: string, tools: string[], diagnostics: AgentDiagnostic[], path: string): boolean {
	if (ref === "package:validator" && !tools.includes("bash")) {
		diagnostics.push(makeDiagnostic("validator-shell-capability-required", "package:validator requires effective bash access for command-backed validation; grant graph.authority.allowShellTools:true with default tools, or use package:reviewer for non-command review.", "error", `${path}/tools`));
		return false;
	}
	if (ref === "package:worker" && !tools.some((tool) => tool === "edit" || tool === "write")) {
		diagnostics.push(makeDiagnostic("worker-mutation-capability-required", "package:worker requires effective edit/write mutation access for implementation; grant graph.authority.allowMutationTools:true for authorized changes, or use package:planner/package:reviewer for non-mutating work.", "error", `${path}/tools`));
		return false;
	}
	return true;
}

function validateBashCwd(stepId: string, agent: ResolvedAgent, cwd: string, diagnostics: AgentDiagnostic[], path: string): boolean {
	if (!agent.tools.includes("bash")) return true;
	const settings = findProjectSettingsFile(cwd);
	if (!settings) return true;
	diagnostics.push(makeDiagnostic("bash-project-settings-denied", `Step ${stepId} requests bash inside project Pi settings at ${settings}; remove bash, change cwd, or run elsewhere.`, "error", path));
	return false;
}

function normalizeGraphSources(sources: LibrarySource[] | undefined, authority: GraphAuthority, diagnostics: AgentDiagnostic[]): LibrarySource[] {
	const selected = dedupeSources(sources && sources.length > 0 ? sources : DEFAULT_GRAPH_LIBRARY_SOURCES);
	return selected;
}

function validateStepGraph(steps: TeamStepSpec[], diagnostics: AgentDiagnostic[]): void {
	const ids = new Set<string>();
	for (const [index, step] of steps.entries()) {
		if (ids.has(step.id)) diagnostics.push(makeDiagnostic("step-id-duplicate", `Duplicate step id: ${step.id}.`, "error", `/graph/steps/${index}/id`));
		ids.add(step.id);
	}
	for (const [index, step] of steps.entries()) {
		for (const need of step.needs) if (!ids.has(need)) diagnostics.push(makeDiagnostic("dependency-unknown", `Step ${step.id} depends on unknown step ${need}.`, "error", `/graph/steps/${index}/needs`));
		for (const after of step.after) if (!ids.has(after)) diagnostics.push(makeDiagnostic("dependency-unknown", `Step ${step.id} waits after unknown step ${after}.`, "error", `/graph/steps/${index}/after`));
	}
	const visited = new Set<string>();
	const stack: { id: string; edgeFromParent?: "needs" | "after" }[] = [];
	const byId = new Map(steps.map((step) => [step.id, step]));
	const visit = (id: string, edgeFromParent?: "needs" | "after"): string | undefined => {
		if (visited.has(id)) return undefined;
		const existing = stack.findIndex((frame) => frame.id === id);
		if (existing !== -1) return formatDependencyCycle([...stack.slice(existing), { id, edgeFromParent }]);
		stack.push({ id, edgeFromParent });
		const step = byId.get(id);
		for (const need of step?.needs ?? []) {
			const cycle = byId.has(need) ? visit(need, "needs") : undefined;
			if (cycle) return cycle;
		}
		for (const after of step?.after ?? []) {
			const cycle = byId.has(after) ? visit(after, "after") : undefined;
			if (cycle) return cycle;
		}
		stack.pop();
		visited.add(id);
		return undefined;
	};
	for (const step of steps) {
		const cycle = visit(step.id);
		if (cycle) {
			diagnostics.push(makeDiagnostic("dependency-cycle", `Graph dependencies contain a cycle: ${cycle}.`, "error", "/graph/steps"));
			return;
		}
	}
}

function formatDependencyCycle(frames: { id: string; edgeFromParent?: "needs" | "after" }[]): string {
	if (frames.length === 0) return "unknown";
	const [first, ...rest] = frames;
	return rest.reduce((text, frame) => `${text} --${frame.edgeFromParent ?? "depends"}--> ${frame.id}`, first.id);
}

function resolveInvocationCwd(cwd: string, diagnostics: AgentDiagnostic[]): string {
	try {
		return realpathSync(cwd);
	} catch (error) {
		diagnostics.push(makeDiagnostic("cwd-invalid", `Could not resolve invocation cwd: ${errorMessage(error)}`, "error", "/"));
		return cwd;
	}
}

function resolveStepCwd(invocationCwd: string, cwd: string | undefined, diagnostics: AgentDiagnostic[], path: string): { path: string; identity: CwdIdentity } | undefined {
	const lexical = cwd ? (isAbsolute(cwd) ? cwd : resolve(invocationCwd, cwd)) : invocationCwd;
	if (!isContainedPath(invocationCwd, lexical)) {
		diagnostics.push(makeDiagnostic("cwd-path-escape-denied", "Step cwd must resolve inside the invocation cwd.", "error", path));
		return undefined;
	}
	const symlinkDiagnostic = findSymlinkPathDiagnostic(invocationCwd, lexical, path);
	if (symlinkDiagnostic) {
		diagnostics.push(symlinkDiagnostic);
		return undefined;
	}
	try {
		const lstat = lstatSync(lexical);
		if (lstat.isSymbolicLink()) {
			diagnostics.push(makeDiagnostic("cwd-symlink-denied", "Step cwd symlinks are denied.", "error", path));
			return undefined;
		}
		if (!statSync(lexical).isDirectory()) {
			diagnostics.push(makeDiagnostic("cwd-not-directory", `Step cwd is not a directory: ${lexical}`, "error", path));
			return undefined;
		}
		const real = realpathSync(lexical);
		if (!isContainedPath(invocationCwd, real)) {
			diagnostics.push(makeDiagnostic("cwd-path-escape-denied", "Step cwd must resolve inside the invocation cwd.", "error", path));
			return undefined;
		}
		const realStats = statSync(real);
		return { path: real, identity: { realpath: real, dev: realStats.dev, ino: realStats.ino } };
	} catch (error) {
		diagnostics.push(makeDiagnostic("cwd-unreadable", `Could not resolve step cwd: ${errorMessage(error)}`, "error", path));
		return undefined;
	}
}

function findSymlinkPathDiagnostic(root: string, lexicalPath: string, path: string): AgentDiagnostic | undefined {
	const relativePath = relative(root, lexicalPath);
	let current = root;
	for (const segment of relativePath.split(sep)) {
		if (!segment) continue;
		current = join(current, segment);
		try {
			if (lstatSync(current).isSymbolicLink()) return makeDiagnostic("cwd-symlink-denied", "Step cwd symlinks are denied.", "error", path);
		} catch {
			return undefined;
		}
	}
	return undefined;
}

function validatePublicId(value: string, label: string, diagnostics: AgentDiagnostic[], path: string): boolean {
	if (PUBLIC_ID_REGEX.test(value)) return true;
	diagnostics.push(makeDiagnostic("public-id-invalid", `${label} must match ${PUBLIC_ID_PATTERN}.`, "error", path));
	return false;
}

function hashGraph(graph: GraphSpec, authority: GraphAuthority, limits: unknown, options: unknown, subagentSkillMode: SubagentSkillMode): string {
	return createHash("sha256").update(JSON.stringify({ graph, authority, limits, options, subagentSkillMode })).digest("hex");
}

function dedupeRefs(values: string[]): string[] {
	return Array.from(new Set(values));
}

function dedupeSources(sources: LibrarySource[]): LibrarySource[] {
	const allowed = new Set<LibrarySource>(LIBRARY_SOURCE_VALUES);
	const seen = new Set<LibrarySource>();
	const result: LibrarySource[] = [];
	for (const source of sources) if (allowed.has(source) && !seen.has(source)) {
		seen.add(source);
		result.push(source);
	}
	return result;
}

function isContainedPath(parent: string, child: string): boolean {
	const normalizedParent = resolve(parent);
	const normalizedChild = resolve(child);
	return normalizedChild === normalizedParent || normalizedChild.startsWith(`${normalizedParent}${sep}`);
}

function errorMessage(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

function makeDiagnostic(code: string, message: string, severity: AgentDiagnostic["severity"], path?: string): AgentDiagnostic {
	return { code, message, severity, path };
}

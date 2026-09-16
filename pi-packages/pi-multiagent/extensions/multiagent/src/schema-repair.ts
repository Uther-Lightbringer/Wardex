import { AGENT_TEAM_ACTION_VALUES, BUILTIN_CHILD_TOOL_NAMES } from "./types.ts";

const BUILTIN_CHILD_TOOL_SET = new Set<string>(BUILTIN_CHILD_TOOL_NAMES);

export function schemaRepair(input: unknown, path: string): string {
	if (isRecord(input)) {
		const pathRepair = schemaRepairForPath(path);
		if (pathRepair) return pathRepair;
		const misplacedExtensionTool = findMisplacedExtensionToolName(input);
		if (misplacedExtensionTool) return `agent.tools accepts only built-in child tools (${BUILTIN_CHILD_TOOL_NAMES.join(", ")}). Move extension tool ${misplacedExtensionTool} to steps[].agent.extensionTools with catalog-copied {name, from:{source, scope?, origin?}} provenance and set graph.authority.allowExtensionCode:true; do not put extension tool names in agent.tools.`;
		if (findAgentSkillsField(input)) return "Remove steps[].agent.skills. Subagent skill availability is controlled only by the product config flag --agent-team-subagent-skills enabled|disabled; it is all-or-nothing, defaults to enabled, and is not graph-controlled.";
		if (path !== "/") return `Fix field ${path}: use the documented type, enum, pattern, and bounds for this exact field; do not apply unrelated control repairs to this diagnostic.`;
		if (input.authority !== undefined) return 'Move authority under graph: {"action":"start","graph":{"authority":{"allowFilesystemRead":true},"objective":"...","steps":[...]}}.';
		if (input.maxBytes !== undefined) return "maxBytes must be between 1 and 200000 and is valid only on run_status/step_result: it bounds assistant previews when preview:true and raw debug event previews when run_status debugEvents:true; catalog narrowing uses library.query.";
		if (input.preview !== undefined) return "preview must be true or false and is valid only for run_status and step_result; previews default to false.";
		if (input.debugEvents !== undefined) return "debugEvents must be true or false and is valid only for run_status; use maxBytes there only to bound raw debug event previews.";
		if (input.channel !== undefined) return 'Use channel:"steer" or channel:"follow_up" for message.';
		if (input.text !== undefined) return "Message text must be a non-empty string.";
	}
	return "Use the action-specific control set: catalog uses library; start uses graph/graphFile plus options; run_status/step_result use runId and preview controls; message uses runId, stepId, channel, text, and optional clientMessageId.";
}

function schemaRepairForPath(path: string): string | undefined {
	if (path === "/action") return `action must be one of ${AGENT_TEAM_ACTION_VALUES.join(", ")}; action:\"run\" is not supported.`;
	if (path === "/runId") return "runId must be the short retained detached run handle returned by start for this Pi session, shaped like r1; retained ids are process/session-local.";
	if (path === "/stepId") return "stepId must be an existing graph step id matching lowercase letters, numbers, and dashes; use run_status to list available step ids or step_result for one step.";
	if (path === "/cursor") return "cursor must be a non-empty cursor returned by a prior run_status call; omit cursor for a fresh snapshot.";
	if (path === "/waitSeconds") return "waitSeconds is valid only on run_status and must be an integer from 1 to 60.";
	if (path === "/maxBytes") return "maxBytes must be between 1 and 200000 and is valid only on run_status/step_result: it bounds assistant previews when preview:true and raw debug event previews when run_status debugEvents:true; catalog narrowing uses library.query.";
	if (path === "/preview") return "preview must be true or false and is valid only for run_status and step_result; previews default to false.";
	if (path === "/debugEvents") return "debugEvents must be true or false and is valid only for run_status; use maxBytes there only to bound raw debug event previews.";
	if (path === "/channel") return 'Use channel:"steer" or channel:"follow_up" for message.';
	if (path === "/text") return "Message text must be a non-empty string.";
	if (path === "/clientMessageId") return "clientMessageId must be a non-empty idempotency key up to 128 characters; reuse the same key only for the same message intent.";
	if (path === "/reason") return "reason must be a short cancel reason string; cancel only when stopping is explicit, unsafe, stuck, obsolete, or higher value than completion.";
	if (path === "/graph") return "start requires a graph object or graphFile; graph contains objective, authority, steps, optional library, and limits.";
	if (path === "/graphFile") return "graphFile must be a relative path to a trusted workspace-local pure detached graph JSON file; use it only with action:start.";
	if (path === "/library") return "catalog library controls sources/query; start graph library sources belong under graph.library.";
	if (path === "/options") return "options is valid only with start and controls retention, maxRunSeconds, and notice behavior.";
	return undefined;
}

function findMisplacedExtensionToolName(input: Record<string, unknown>): string | undefined {
	const graph = input.graph;
	if (!isRecord(graph) || !Array.isArray(graph.steps)) return undefined;
	for (const step of graph.steps) {
		if (!isRecord(step) || !isRecord(step.agent) || !Array.isArray(step.agent.tools)) continue;
		for (const tool of step.agent.tools) if (typeof tool === "string" && !BUILTIN_CHILD_TOOL_SET.has(tool)) return tool;
	}
	return undefined;
}

function findAgentSkillsField(input: Record<string, unknown>): boolean {
	const graph = input.graph;
	if (!isRecord(graph) || !Array.isArray(graph.steps)) return false;
	return graph.steps.some((step) => isRecord(step) && isRecord(step.agent) && step.agent.skills !== undefined);
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null;
}

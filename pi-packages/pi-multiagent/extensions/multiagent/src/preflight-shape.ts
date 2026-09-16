/** Action-specific shape preflight for agent_team inputs. */

import type { AgentDiagnostic, ExecutionAction } from "./types.ts";
import { AGENT_TEAM_ACTION_VALUES, MESSAGE_CHANNEL_VALUES } from "./types.ts";

type PreflightField =
	| "action"
	| "graph"
	| "graphFile"
	| "library"
	| "options"
	| "runId"
	| "cursor"
	| "stepId"
	| "waitSeconds"
	| "maxBytes"
	| "preview"
	| "debugEvents"
	| "kind"
	| "channel"
	| "text"
	| "clientMessageId"
	| "reason"
	| "objective"
	| "steps"
	| "limits"
	| "authority"
	| "extensionTools";

interface ActionRule {
	allowed: readonly PreflightField[];
	controlCode: string;
}

const ACTION_RULES: Record<ExecutionAction, ActionRule> = {
	catalog: { allowed: ["action", "library"], controlCode: "catalog-control-fields-denied" },
	start: { allowed: ["action", "graph", "graphFile", "options"], controlCode: "start-control-fields-denied" },
	run_status: { allowed: ["action", "runId", "cursor", "stepId", "waitSeconds", "maxBytes", "preview", "debugEvents"], controlCode: "run_status-control-fields-denied" },
	step_result: { allowed: ["action", "runId", "stepId", "maxBytes", "preview"], controlCode: "step_result-control-fields-denied" },
	message: { allowed: ["action", "runId", "stepId", "channel", "text", "clientMessageId"], controlCode: "message-control-fields-denied" },
	cancel: { allowed: ["action", "runId", "reason"], controlCode: "cancel-control-fields-denied" },
	cleanup: { allowed: ["action", "runId"], controlCode: "cleanup-control-fields-denied" },
};

const KNOWN_FIELDS: readonly PreflightField[] = [
	"action",
	"graph",
	"graphFile",
	"library",
	"options",
	"runId",
	"cursor",
	"stepId",
	"waitSeconds",
	"maxBytes",
	"preview",
	"debugEvents",
	"kind",
	"channel",
	"text",
	"clientMessageId",
	"reason",
	"objective",
	"steps",
	"limits",
	"authority",
	"extensionTools",
];

// Current graph body fields eligible for the start "move under graph" repair message.
// `library` is valid in GraphSchema, but it is intentionally handled by its
// dedicated top-level catalog/start repair branch below.
const START_GRAPH_BODY_REPAIR_FIELDS = new Set<PreflightField>(["objective", "steps", "limits", "authority"]);

/** Return fail-closed diagnostics for controls that are invalid for the selected action. */
export function validatePreflightShape(rawInput: unknown): AgentDiagnostic[] {
	const input = isRecord(rawInput) ? rawInput : {};
	const action = stringField(input, "action");
	if (!action) return [diagnostic("action-required", 'agent_team requires action:"catalog", "start", "run_status", "step_result", "message", "cancel", or "cleanup".', "/action", undefined, undefined, "Set action to one of catalog, start, run_status, step_result, message, cancel, or cleanup.")];
	if (!isExecutionAction(action)) return [diagnostic("action-invalid", `Unknown agent_team action: ${action}.`, "/action", undefined, undefined, 'Set action to one of catalog, start, run_status, step_result, message, cancel, or cleanup. Use action:"start" for detached graph execution.')];

	const diagnostics: AgentDiagnostic[] = [];
	const rule = ACTION_RULES[action];
	const misplaced = misplacedFields(input, rule.allowed);
	if (misplaced.length > 0) diagnostics.push(diagnostic(rule.controlCode, `Action ${action} rejects fields: ${misplaced.join(", ")}.`, "/", action, misplaced, repairFor(action, misplaced)));
	validateRequiredControls(action, input, diagnostics);
	return diagnostics;
}

function validateRequiredControls(action: ExecutionAction, input: Record<string, unknown>, diagnostics: AgentDiagnostic[]): void {
	if (action === "start") {
		const hasGraph = fieldPresent(input, "graph");
		const hasGraphFile = fieldPresent(input, "graphFile");
		if (hasGraph === hasGraphFile) diagnostics.push(diagnostic("start-graph-required", "Start requires exactly one of graph or graphFile.", "/", action, ["graph", "graphFile"], 'Pass a pure graph under graph, or pass graphFile for a trusted relative JSON graph file. Do not put graph body fields at the tool-input top level.'));
	}
	if (action === "run_status" && !stringField(input, "runId")) diagnostics.push(diagnostic("run-id-required", "run_status requires runId.", "/runId", action, ["runId"], "Use the runId returned by start."));
	if (action === "step_result") {
		if (!stringField(input, "runId")) diagnostics.push(diagnostic("run-id-required", "step_result requires runId.", "/runId", action, ["runId"], "Use the runId returned by start."));
		if (!stringField(input, "stepId")) diagnostics.push(diagnostic("step_result-step-required", "step_result requires exactly one stepId.", "/stepId", action, ["stepId"], "Use one concrete step id from the graph or run_status snapshot; use run_status for run-level or sink-only summaries."));
	}
	if (action === "message") {
		if (!stringField(input, "runId")) diagnostics.push(diagnostic("run-id-required", "message requires runId.", "/runId", action, ["runId"], "Use the runId returned by start."));
		if (!stringField(input, "stepId")) diagnostics.push(diagnostic("message-step-required", "Message requires stepId.", "/stepId", action, ["stepId"], "Messages target exactly one live step."));
		const channel = stringField(input, "channel");
		if (!channel) diagnostics.push(diagnostic("message-channel-required", "Message requires channel.", "/channel", action, ["channel"], 'Use channel:"steer" for live clarification/scope repair or channel:"follow_up" for a deferred live follow-up before terminalization.'));
		else if (!MESSAGE_CHANNEL_VALUES.includes(channel as (typeof MESSAGE_CHANNEL_VALUES)[number])) diagnostics.push(diagnostic("message-channel-invalid", `Unknown message channel: ${channel}.`, "/channel", action, ["channel"], 'Use channel:"steer" or channel:"follow_up".'));
		if (!nonEmptyStringField(input, "text")) diagnostics.push(diagnostic("message-text-required", "Message requires non-empty text.", "/text", action, ["text"], "Send one bounded clarification or scope repair. Do not request premature finals just because the parent is waiting."));
	}
	if (action === "cancel" && !stringField(input, "runId")) diagnostics.push(diagnostic("run-id-required", "cancel requires runId.", "/runId", action, ["runId"], "Use the runId returned by start."));
	if (action === "cleanup" && !stringField(input, "runId")) diagnostics.push(diagnostic("run-id-required", "cleanup requires runId.", "/runId", action, ["runId"], "Use cleanup only after terminal run_status/step_result evidence is preserved."));
}

function misplacedFields(input: Record<string, unknown>, allowedFields: readonly PreflightField[]): string[] {
	const allowed = new Set<PreflightField>(allowedFields);
	return KNOWN_FIELDS.filter((field) => field !== "action" && !allowed.has(field) && fieldPresent(input, field));
}

function repairFor(action: ExecutionAction, fields: string[]): string {
	const fieldSet = new Set(fields);
	if (action === "start" && fields.some((field) => START_GRAPH_BODY_REPAIR_FIELDS.has(field as PreflightField))) return 'Move graph body fields under graph: {"action":"start","graph":{"objective":"...","authority":{"allowFilesystemRead":true},"steps":[...]}}. Put start sources in graph.library; top-level library is catalog-only.';
	if (fieldSet.has("extensionTools")) return "Place extensionTools under steps[].agent.extensionTools with catalog-copied provenance; agent.tools accepts only built-in child tools.";
	if (action === "catalog" && fieldSet.has("maxBytes")) return "Remove maxBytes; use library.query to narrow catalog results. maxBytes is valid only on run_status/step_result: it bounds assistant previews when preview:true and raw debug events when debugEvents:true.";
	if (fieldSet.has("preview")) return "Use preview only on run_status or step_result; it defaults to false and opts into bounded assistant text previews. Use maxBytes there only to bound those previews or run_status debug events.";
	if (action === "message" && fieldSet.has("kind")) return 'Replace kind with channel:"steer" or channel:"follow_up"; accepted means accepted for delivery to live child Pi, not child compliance, output, completion, or terminal resurrection.';
	if (fieldSet.has("waitSeconds")) return "Use waitSeconds only on run_status for a bounded wait/read snapshot that wakes on material parent-visible events or timeout.";
	if (fieldSet.has("debugEvents")) return "Use debugEvents only on run_status when raw background events are needed; maxBytes may bound raw debug event previews there.";
	if (fieldSet.has("cursor")) return "Use cursor only on run_status for debug/backfill pagination.";
	if (fieldSet.has("library") && action === "start") return "Move start library source selection to graph.library; top-level library is catalog-only.";
	return 'Use action-specific controls: catalog {library}; start exactly one of {graph,graphFile} plus options; run_status {runId,cursor?,stepId?,waitSeconds?,preview?,maxBytes?,debugEvents?}; step_result {runId,stepId,preview?,maxBytes?}; message {runId,stepId,channel,text,clientMessageId?}; cancel {runId,reason?}; cleanup {runId}.';
}

function isExecutionAction(action: string): action is ExecutionAction {
	return AGENT_TEAM_ACTION_VALUES.includes(action as ExecutionAction);
}

function stringField(input: Record<string, unknown>, field: string): string | undefined {
	const value = input[field];
	return typeof value === "string" ? value : undefined;
}

function nonEmptyStringField(input: Record<string, unknown>, field: string): boolean {
	const value = stringField(input, field);
	return value !== undefined && value.trim().length > 0;
}

function fieldPresent(input: Record<string, unknown>, field: string): boolean {
	return input[field] !== undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null;
}

function diagnostic(code: string, message: string, path: string, action?: ExecutionAction, fields?: readonly string[], repair?: string): AgentDiagnostic {
	return { code, message, path, severity: "error", action, fields: fields ? [...fields] : undefined, repair };
}

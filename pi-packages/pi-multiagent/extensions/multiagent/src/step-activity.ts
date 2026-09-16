/** Compact activity summaries for supervision views. */

import type { BackgroundEvent } from "./types.ts";

export interface StepActivityTracker {
	record(event: BackgroundEvent): void;
	summary(stepId: string): string | undefined;
}

export function createStepActivityTracker(): StepActivityTracker {
	const summaries = new Map<string, string>();
	return {
		record(event) {
			if (!event.stepId || isNonActivityParentDenial(event)) return;
			summaries.set(event.stepId, summarizeStepActivity(event));
		},
		summary(stepId) {
			return summaries.get(stepId);
		},
	};
}

function isNonActivityParentDenial(event: BackgroundEvent): boolean {
	return event.type === "parent_message" && event.status === "error";
}

export function summarizeStepActivity(event: BackgroundEvent): string {
	if (event.type === "step") return summarizeStep(event);
	if (event.type === "tool") return summarizeTool(event);
	if (event.type === "rpc") return summarizeRpc(event);
	if (event.type === "assistant_delta") return "assistant writing";
	if (event.type === "assistant_final") return "assistant final";
	if (event.type === "parent_message") return summarizeParentMessage(event);
	if (event.type === "diagnostic") return summarizeDiagnostic(event);
	if (event.type === "ui") return `UI request ${event.status === "done" ? "suppressed" : "denied"}${event.label ? `: ${event.label}` : ""}`;
	if (event.type === "run") return summarizeRun(event);
	return event.preview ?? event.status ?? event.type;
}

function summarizeStep(event: BackgroundEvent): string {
	if (event.label === "start") return "step started";
	if (event.label === "finish") {
		const reason = event.status && event.status !== "succeeded" && event.preview ? `: ${compactPreview(event.preview)}` : "";
		return `step finished${event.status ? ` [${event.status}]` : ""}${reason}`;
	}
	return withStatus(`step ${event.label ?? "activity"}`, event.status);
}

function summarizeTool(event: BackgroundEvent): string {
	const name = event.label ?? "tool";
	if (event.status === "running") return `tool ${name} running`;
	if (event.status === "done") return `tool ${name} done`;
	if (event.status === "error") return `tool ${name} error`;
	return withStatus(`tool ${name}`, event.status);
}

function summarizeRpc(event: BackgroundEvent): string {
	if (event.label === "spawn") return "child spawned";
	if (event.label === "message_start" || event.label === "turn_start") return "model turn active (no output yet)";
	if (event.label === "message_end") return "assistant message ended";
	if (event.label === "agent_end") return "child terminal event";
	if (event.label === "heartbeat") return "child heartbeat";
	if (event.label === "terminalizing") return `terminalizing${event.preview ? ` [${event.preview}]` : ""}`;
	if (event.label === "prompt") return event.status === "done" ? "prompt accepted; waiting for child output" : "child prompt sent";
	if (event.label === "thinking") return event.status === "done" ? "reasoning ended" : "reasoning";
	if (event.label === "toolcall") return event.status === "done" ? "tool call ended" : "tool call streaming";
	if (event.label === "tool_use" || event.label === "message_update:tool_use") return "assistant tool activity";
	if (event.label === "response") return "child command response";
	return `child event: ${event.label ?? "rpc"}`;
}

function summarizeParentMessage(event: BackgroundEvent): string {
	const kind = event.label ?? "message";
	if (event.status === "running") return `parent ${kind} sending`;
	if (kind.endsWith("_queued")) return `parent ${kind.replace("_", " ")} queued`;
	if (kind.endsWith("_consumed")) return `parent ${kind.replace("_", " ")} consumed by Pi queue`;
	if (event.status === "done") return `parent ${kind} accepted for delivery`;
	if (event.status === "error") return `parent ${kind} denied`;
	return withStatus(`parent ${kind}`, event.status);
}

function summarizeDiagnostic(event: BackgroundEvent): string {
	const label = event.label ?? "diagnostic";
	if (event.status === "error") return `${label} error`;
	return withStatus(label, event.status);
}

function summarizeRun(event: BackgroundEvent): string {
	if (event.label === "terminal") return `terminal: ${event.preview ?? event.status ?? "done"}`;
	if (event.label === "start") return event.preview ?? "run started";
	if (event.label === "cleanup") return event.preview ?? "cleanup done";
	if (event.label === "cancel") return "run cancel requested";
	if (event.label === "expired") return "run expired";
	return withStatus(event.label ?? "run activity", event.status);
}

function compactPreview(text: string): string {
	return text.length > 180 ? `${text.slice(0, 177)}...` : text;
}

function withStatus(label: string, status: string | undefined): string {
	return status ? `${label} [${status}]` : label;
}

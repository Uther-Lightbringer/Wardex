/** Final-output helpers for detached runs. */

import type { AgentInvocationMetadata, ChildSessionMetadata, RunStatus, StepArtifactReference, StepOutput, StepOutputLimit, StepRetryRecord, StepStatus, TeamStepSpec } from "./types.ts";
import { formatStepOutputLimit } from "./step-output-limit.ts";

export const FINAL_INLINE_PREVIEW_CHARS = 6000;

interface StepFinalArtifactInput {
	runId: string;
	objective: string;
	step: TeamStepSpec;
	status: StepStatus;
	startedAt: string | undefined;
	endedAt: string;
	text: string;
	assistantFinals: string[];
	nonFinalText: string | undefined;
	stopReason: string | undefined;
	invocation: AgentInvocationMetadata;
	outputLimit: StepOutputLimit | undefined;
	upstreamArtifacts: StepArtifactReference[];
	childSession: ChildSessionMetadata | undefined;
	retryHistory: StepRetryRecord[];
}

export function buildStepFinalArtifact(input: StepFinalArtifactInput): string {
	return [
		"# agent_team step final",
		"",
		`runId: ${input.runId}`,
		`objective: ${input.objective}`,
		`stepId: ${input.step.id}`,
		`status: ${input.status}`,
		`stopReason: ${input.stopReason ?? input.status}`,
		`agentRef: ${input.step.agent.ref}`,
		`agentSource: ${input.step.agent.source}`,
		`model: ${input.invocation.model ?? "default"}`,
		`thinking: ${input.invocation.thinking ?? "default"}`,
		`effectiveTools: ${input.step.agent.tools.length > 0 ? input.step.agent.tools.join(", ") : "none"}`,
		`extensionTools: ${input.step.agent.extensionTools.length > 0 ? input.step.agent.extensionTools.map((tool) => tool.name).join(", ") : "none"}`,
		input.outputLimit ? `outputLimit: ${formatStepOutputLimit(input.outputLimit)}` : "",
		`cwd: ${input.step.cwd ?? "default"}`,
		`needs: ${input.step.needs.length > 0 ? input.step.needs.join(", ") : "none"}`,
		`after: ${input.step.after.length > 0 ? input.step.after.join(", ") : "none"}`,
		`startedAt: ${input.startedAt ?? ""}`,
		`endedAt: ${input.endedAt}`,
		"",
		"## Child session",
		formatChildSession(input.childSession),
		"",
		"## Retry history",
		formatRetryHistory(input.retryHistory),
		"",
		"## Upstream artifacts",
		formatUpstreamArtifacts(input.upstreamArtifacts),
		"",
		"## Task",
		"",
		input.step.task,
		"",
		formatStepFinalBody(input.text, input.assistantFinals, input.nonFinalText),
	].join("\n");
}

function formatUpstreamArtifacts(upstreamArtifacts: StepArtifactReference[]): string {
	if (upstreamArtifacts.length === 0) return "none";
	return upstreamArtifacts.map((artifact) => {
		const path = artifact.filePath ? JSON.stringify(artifact.filePath) : "none";
		const chars = artifact.chars ?? 0;
		return `- ${artifact.stepId} [${artifact.status}]: artifact=${path} chars=${chars}`;
	}).join("\n");
}

function formatStepFinalBody(text: string, assistantFinals: string[], nonFinalText: string | undefined): string {
	const sections: string[] = [];
	if (assistantFinals.length === 1) sections.push(assistantFinals[0] ?? "");
	else if (assistantFinals.length > 1) sections.push(formatAssistantFinalMessages(assistantFinals));
	if (nonFinalText !== undefined && nonFinalText.trim().length > 0) sections.push(formatNonFinalText(nonFinalText));
	if (sections.length > 0) return sections.join("\n\n");
	return fallbackFinalText(text);
}

export function formatAssistantFinalMessages(texts: string[]): string {
	return texts.map((text, index) => [`## Assistant final ${index + 1}`, "", text].join("\n")).join("\n\n");
}

export function formatNonFinalText(text: string): string {
	return ["## Non-final assistant evidence", "", "This text was captured outside a successful assistant final. It is retained as bounded failure/cancel/timeout or later-partial evidence, not as a completed child answer.", "", text].join("\n");
}

function formatChildSession(session: ChildSessionMetadata | undefined): string {
	if (!session) return "unavailable";
	return [
		`sessionId: ${session.sessionId ?? "unknown"}`,
		`sessionName: ${session.sessionName ?? "unknown"}`,
		`sessionFile: ${session.sessionFile ?? "unknown"}`,
		`sessionDir: ${session.sessionDir ?? "unknown"}`,
		`launchSessionDir: ${session.launchSessionDir ?? "default"}`,
		`source: ${session.stateSource}`,
	].join("\n");
}

function formatRetryHistory(history: StepRetryRecord[]): string {
	if (history.length === 0) return "none";
	return history.map((entry) => `- attempt ${entry.attempt}: ${entry.reason}${entry.childSession?.sessionFile ? ` childSessionFile=${JSON.stringify(entry.childSession.sessionFile)}` : ""}`).join("\n");
}

function fallbackFinalText(text: string): string {
	return ["## Final text", "", text].join("\n");
}

export function formatStepOutputText(text: string, assistantFinals: string[], nonFinalText: string | undefined): string {
	const sections: string[] = [];
	if (assistantFinals.length > 0) sections.push(assistantFinals.length === 1 ? assistantFinals[0] ?? "" : formatAssistantFinalMessages(assistantFinals));
	if (nonFinalText && nonFinalText.trim().length > 0) sections.push(formatNonFinalText(nonFinalText));
	if (sections.length > 0) return sections.join("\n\n");
	return text;
}

export function boundedFinalPreview(text: string): string {
	if (text.length <= FINAL_INLINE_PREVIEW_CHARS) return text;
	return `${text.slice(0, FINAL_INLINE_PREVIEW_CHARS)}\n\n[agent_team preview truncated; full final is in the artifact file.]`;
}

export function boundedTextPreview(text: string, maxBytes: number): string {
	if (Buffer.byteLength(text, "utf8") <= maxBytes) return text;
	const suffix = "\n\n[agent_team preview truncated by maxBytes.]";
	const budget = Math.max(1, maxBytes - Buffer.byteLength(suffix, "utf8"));
	return `${utf8Prefix(text, budget)}${suffix}`;
}

export function boundedStepOutput(output: StepOutput, maxBytes: number): StepOutput {
	if (output.text === undefined) return output;
	return { ...output, text: boundedTextPreview(output.text, maxBytes) };
}

function utf8Prefix(text: string, maxBytes: number): string {
	let bytes = 0;
	let end = 0;
	for (const char of text) {
		const next = Buffer.byteLength(char, "utf8");
		if (bytes + next > maxBytes) break;
		bytes += next;
		end += char.length;
	}
	return text.slice(0, end);
}

export function isTerminalStepStatus(status: StepStatus): boolean {
	return status === "succeeded" || status === "failed" || status === "blocked" || status === "timed_out" || status === "canceled";
}

export function isTerminalRunStatus(status: RunStatus): boolean {
	return status === "succeeded" || status === "mixed" || status === "failed" || status === "canceled" || status === "expired";
}

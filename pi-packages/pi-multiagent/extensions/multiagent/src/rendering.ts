/** Human TUI rendering for the detached `agent_team` tool. */

import type { AgentToolResult, MessageRenderOptions, Theme, ToolRenderResultOptions } from "@earendil-works/pi-coding-agent";
import { Text, type Component } from "@earendil-works/pi-tui";
import { artifactName, summarizeArtifacts, summarizeFullArtifactPaths, terminalArtifacts } from "./rendering-artifacts.ts";
import type { AgentTeamInput } from "./schemas.ts";
import {
	attentionLines,
	focusedActivitySummary,
	humanActivity,
	humanRunStatus,
	humanStepStatus,
	liveProgressSummary,
	objectiveTitle,
	pendingSummary,
	progressMeter,
	shortRunId,
	statusColor,
	stepsByStatus,
	truncate,
} from "./rendering-live.ts";
import type { AgentTeamDetails, CleanupReceipt, RunSnapshot } from "./types.ts";

export { AgentTeamLiveRunsWidget, renderAgentTeamLiveRunsWidget } from "./rendering-live.ts";

const OBJECTIVE_CHARS = 76;
const VALUE_CHARS = 88;
const MAX_IDS = 3;

interface CardOptions {
	expanded: boolean;
	includeAction: boolean;
}

interface RenderContext {
	lastComponent: Component | undefined;
}

export function renderAgentTeamCall(args: AgentTeamInput, theme: Theme, context?: RenderContext): Component {
	const text = reuseText(context?.lastComponent);
	text.setText(formatCall(args, theme));
	return text;
}

export function renderAgentTeamResult(result: AgentToolResult<AgentTeamDetails>, options: ToolRenderResultOptions, theme: Theme, context?: RenderContext): Component {
	const text = reuseText(context?.lastComponent);
	const details = result.details;
	if (!details) {
		text.setText(theme.fg("dim", "agent_team: no details"));
		return text;
	}
	text.setText(formatAgentTeamCard(details, theme, { expanded: options.expanded, includeAction: true }).join("\n"));
	return text;
}

export function renderAgentTeamNoticeMessage(details: AgentTeamDetails | undefined, _content: string | unknown, options: MessageRenderOptions, theme: Theme): Component {
	if (!details) {
		const fallback = typeof _content === "string" ? _content.trim() : "";
		return new Text(fallback.length > 0 ? fallback : theme.fg("warning", "agent_team notice: missing details"), 0, 0);
	}
	return new Text(formatAgentTeamCard(details, theme, { expanded: options.expanded, includeAction: false }).join("\n"), 0, 0);
}

export function formatAgentTeamNoticeText(details: AgentTeamDetails | undefined): string {
	if (!details) return "agent_team notice unavailable";
	return formatAgentTeamPlainCard(details).join("\n");
}

export function formatAgentTeamCard(details: AgentTeamDetails, theme: Theme, options: CardOptions): string[] {
	if (details.action === "catalog") return formatCatalog(details, theme);
	if (hasCleanupReceipt(details)) return formatCleanupReceipt(details, theme, options.includeAction);
	if (!details.run) return formatNoRun(details, theme, options.includeAction);
	const run = details.run;
	const lines = [formatResultHeader(details, run, theme, options.includeAction), `${progressMeter(run.counts, theme)} ${theme.fg("dim", liveProgressSummary(run.counts))}`];
	const attention = actionErrorLines(details, theme).concat(attentionLines(details, theme, 2));
	if (attention.length > 0) lines.push(theme.fg("warning", "needs attention"), ...attention.slice(0, 2));
	const working = focusedActivitySummary(details);
	if (working) lines.push(`${theme.fg("muted", "working now")} ${theme.fg("dim", working)}`);
	const pending = stepsByStatus(details, "pending");
	if (pending.length > 0) lines.push(`${theme.fg("muted", "queued next")} ${theme.fg("dim", pendingSummary(pending))}`);
	const tail = formatResultTail(details, theme);
	if (tail) lines.push(tail);
	if (options.expanded) lines.push(formatExpanded(details, theme));
	return lines.filter((line) => line.length > 0);
}

function formatCall(args: AgentTeamInput, theme: Theme): string {
	const title = theme.fg("toolTitle", theme.bold("agent_team"));
	const action = stringProperty(args, "action");
	const runId = stringProperty(args, "runId");
	const stepId = stringProperty(args, "stepId");
	const channel = stringProperty(args, "channel");
	if (action === "start") return `${title} ${theme.fg("accent", "launch")} ${theme.fg("dim", startTarget(args))}`;
	if (action === "run_status") return `${title} ${theme.fg("accent", typeof args.waitSeconds === "number" ? `wait ${args.waitSeconds}s` : args.debugEvents === true ? "status debug" : "status")} ${theme.fg("dim", shortRunId(runId))}`;
	if (action === "step_result") return `${title} ${theme.fg("accent", "inspect step")} ${theme.fg("dim", `${shortRunId(runId)} ${stepId ?? ""}`.trim())}`;
	if (action === "message") return `${title} ${theme.fg("accent", channel === "follow_up" ? "send follow-up" : "steer live step")} ${theme.fg("dim", `${stepId ?? "step"} ${channel ?? ""}`.trim())}`;
	if (action === "cancel") return `${title} ${theme.fg("accent", "stop run")} ${theme.fg("dim", shortRunId(runId))}`;
	if (action === "cleanup") return `${title} ${theme.fg("accent", "delete evidence")} ${theme.fg("dim", shortRunId(runId))}`;
	const query = catalogQuery(args);
	return `${title} ${theme.fg("accent", "catalog")}${query ? ` ${theme.fg("dim", query)}` : ""}`;
}

function formatCatalog(details: AgentTeamDetails, theme: Theme): string[] {
	const activeExtensions = details.extensionTools.filter((tool) => tool.active).length;
	return [
		`${theme.fg(details.ok ? "success" : "error", `catalog ${details.ok ? "ok" : "error"}`)} ${theme.fg("dim", `${details.catalog.length} agent(s), ${activeExtensions} active extension tool(s)`)}`,
	];
}

function formatAgentTeamPlainCard(details: AgentTeamDetails): string[] {
	if (details.action === "catalog") return [`agent_team catalog ${details.ok ? "ok" : "error"} ${details.catalog.length} agent(s)`];
	if (hasCleanupReceipt(details)) return [`agent_team evidence deleted ${shortRunId(details.cleanup.runId)}`, `evidence deleted ${details.cleanup.deletedPaths.length} retained path(s)`, "untrusted cleanup receipt; retained evidence was deleted"];
	if (!details.run) return [`agent_team ${details.action} ${details.ok ? "ok" : "error"}${details.error ? ` ${details.error.code}` : ""}`];
	const run = details.run;
	const state = humanResultState(details, run);
	const subject = runIdSubjectState(state) ? shortRunId(run.runId) : objectiveTitle(run.objective, OBJECTIVE_CHARS) || shortRunId(run.runId);
	const lines = [`agent_team ${state} ${subject}`, `runId=${run.runId}`, `${plainProgressMeter(run.counts)} ${liveProgressSummary(run.counts)}`];
	const attention = plainAttentionLine(details);
	if (attention) lines.push(`needs attention ${attention}`);
	const working = focusedActivitySummary(details);
	if (working) lines.push(`working now ${working}`);
	const pending = stepsByStatus(details, "pending");
	if (pending.length > 0) lines.push(`queued next ${pendingSummary(pending)}`);
	const tail = formatPlainResultTail(details);
	if (tail) lines.push(tail);
	lines.push(...formatPlainTerminalEvidence(details));
	lines.push("untrusted status evidence; run_status/step_result for artifacts");
	return lines.filter((line) => line.length > 0);
}

function plainAttentionLine(details: AgentTeamDetails): string | undefined {
	if (details.error) return `${details.error.code} ${truncate(details.error.message, VALUE_CHARS)}`;
	const diagnostic = details.diagnostics.find((item) => item.severity === "error");
	if (diagnostic) return `${diagnostic.code} ${truncate(diagnostic.message, VALUE_CHARS)}`;
	const step = details.steps.find((item) => item.status === "failed" || item.status === "blocked" || item.status === "timed_out" || item.status === "canceled");
	if (!step) return undefined;
	return `${step.id} ${humanStepStatus(step.status)}${step.errorMessage ? `: ${truncate(humanActivity(step.errorMessage), VALUE_CHARS)}` : ""}`;
}

function plainProgressMeter(counts: RunSnapshot["counts"]): string {
	const total = Math.max(1, Object.values(counts).reduce((sum, count) => sum + count, 0));
	const done = counts.succeeded + counts.failed + counts.blocked + counts.timed_out + counts.canceled;
	const filled = Math.min(14, Math.round((done / total) * 14));
	return `[${"=".repeat(filled)}${"-".repeat(14 - filled)}]`;
}

function formatPlainResultTail(details: AgentTeamDetails): string {
	if (details.message) return `message ${details.message.stepId} ${details.message.accepted ? "accepted for delivery" : details.message.undeliveredReason ?? "denied"}`;
	if (details.wait) return plainWaitSummary(details.wait);
	if (details.action === "cleanup" && details.error) return `cleanup ${details.error.code}`;
	if (details.cleanup) return `evidence deleted ${details.cleanup.deletedPaths.length} retained path(s)`;
	if (details.outputs.length > 0) return `final evidence ${summarizePlainOutputs(details.outputs)}`;
	return `last update ${truncate(humanActivity(details.run?.lastEvent ?? "state changed"), VALUE_CHARS)}`;
}

function formatPlainTerminalEvidence(details: AgentTeamDetails): string[] {
	if (!details.notice?.terminal) return [];
	const lines: string[] = [];
	const artifacts = terminalArtifacts(details);
	if (artifacts.length > 0) lines.push(`artifact paths ${summarizeFullArtifactPaths(artifacts)}`);
	if (details.run?.expiresAt) lines.push(`expiresAt=${details.run.expiresAt}`);
	return lines;
}

function plainWaitSummary(wait: NonNullable<AgentTeamDetails["wait"]>): string {
	const target = wait.stepId ? ` ${wait.stepId}` : "";
	return `wait ${waitOutcomeCopy(wait.outcome)}${target} ${waitCursorCopy(wait)}`;
}

function waitOutcomeCopy(outcome: NonNullable<AgentTeamDetails["wait"]>["outcome"]): string {
	if (outcome === "already-material") return "already material";
	return outcome;
}

function waitCursorCopy(wait: NonNullable<AgentTeamDetails["wait"]>): string {
	return `${wait.requestedSeconds}s cursor ${wait.cursorBefore}->${wait.cursorAfter}`;
}

function summarizePlainOutputs(outputs: AgentTeamDetails["outputs"]): string {
	const visible = outputs.slice(0, MAX_IDS).map((output) => `${output.stepId} ${humanStepStatus(output.status)} ${artifactName(output.filePath)}`).join(", ");
	return outputs.length > MAX_IDS ? `${visible}, +${outputs.length - MAX_IDS} more` : visible;
}

function actionErrorLines(details: AgentTeamDetails, theme: Theme): string[] {
	if (!details.error) return [];
	return [`  ${theme.fg("warning", "!")} ${theme.fg("text", details.error.code)} ${theme.fg("dim", truncate(details.error.message, VALUE_CHARS))}`];
}

function formatNoRun(details: AgentTeamDetails, theme: Theme, includeAction: boolean): string[] {
	const prefix = includeAction ? `${details.action} ` : "";
	const status = details.ok ? theme.fg("success", `${prefix}ok`) : theme.fg("error", `${prefix}error`);
	const reason = details.error ? ` ${details.error.code}` : " no run";
	return [`${status}${theme.fg("dim", reason)}`];
}

function formatCleanupReceipt(details: AgentTeamDetails, theme: Theme, includeAction: boolean): string[] {
	const cleanup = details.cleanup;
	if (!cleanup) return formatNoRun(details, theme, includeAction);
	const prefix = includeAction ? "agent_team " : "";
	return [`${theme.fg("success", `${prefix}evidence deleted`)} ${theme.fg("dim", shortRunId(cleanup.runId))}`, `${theme.fg("muted", "evidence deleted")} ${theme.fg("dim", `${cleanup.deletedPaths.length} retained path(s)`)}`];
}

function hasCleanupReceipt(details: AgentTeamDetails): details is AgentTeamDetails & { cleanup: CleanupReceipt } {
	return details.action === "cleanup" && details.ok && details.cleanup !== undefined;
}

function formatResultHeader(details: AgentTeamDetails, run: RunSnapshot, theme: Theme, includeAction: boolean): string {
	const title = includeAction ? theme.fg("toolTitle", theme.bold("agent_team")) : theme.fg("toolTitle", "agent_team");
	const state = humanResultState(details, run);
	const runLabel = shortRunId(run.runId);
	const subject = runIdSubjectState(state) ? runLabel : objectiveTitle(run.objective, OBJECTIVE_CHARS) || runLabel;
	return `${title} ${theme.fg(statusColor(run.status), state)} ${theme.fg("dim", subject)}`;
}

function runIdSubjectState(state: string): boolean {
	return state === "stop requested" || state === "message accepted" || state === "message denied" || state === "cleanup denied" || state === "cleanup failed" || state === "cleanup error" || state === "cleanup ok" || state === "evidence deleted";
}

function humanResultState(details: AgentTeamDetails, run: RunSnapshot): string {
	if (details.action === "cancel") return run.terminal ? humanRunStatus(run.status) : "stop requested";
	if (details.action === "message") return details.message?.accepted === false ? "message denied" : "message accepted";
	if (details.action === "cleanup") {
		if (details.cleanup) return "evidence deleted";
		if (details.error?.code === "cleanup-run-live") return "cleanup denied";
		if (details.error?.code === "cleanup-artifacts-failed") return "cleanup failed";
		if (!details.ok) return "cleanup error";
		return "cleanup ok";
	}
	if (run.terminal) return humanRunStatus(run.status);
	if (details.action === "start") return "started";
	if (details.action === "step_result") return "step snapshot";
	return humanRunStatus(run.status);
}

function formatResultTail(details: AgentTeamDetails, theme: Theme): string {
	if (details.message) return `${theme.fg("muted", "message")} ${details.message.stepId} ${details.message.accepted ? theme.fg("success", "accepted for delivery") : theme.fg("error", details.message.undeliveredReason ?? "denied")}`;
	if (details.wait) return `${theme.fg("muted", "wait")} ${theme.fg(details.wait.outcome === "timeout" ? "warning" : "success", waitOutcomeCopy(details.wait.outcome))} ${theme.fg("dim", `${details.wait.stepId ? `${details.wait.stepId} ` : ""}${waitCursorCopy(details.wait)}`)}`;
	if (details.action === "cleanup" && details.error) return `${theme.fg("muted", "cleanup")} ${theme.fg("error", details.error.code)}`;
	if (details.cleanup) return `${theme.fg("muted", "evidence deleted")} ${theme.fg("dim", `${details.cleanup.deletedPaths.length} retained path(s)`)}`;
	if (details.notice) return `${theme.fg("muted", details.notice.terminal ? "terminal notice" : "milestone notice")} ${theme.fg("accent", summarizeNotice(details))}${formatNoticeArtifactCopy(details, theme)}${formatNoticeExpiry(details, theme)}`;
	if (details.outputs.length === 1) {
		const output = details.outputs[0];
		if (output) return `${theme.fg("muted", output.status === "running" || output.status === "pending" ? "live output" : "final output")} ${theme.fg(statusColor(output.status), `${output.stepId} ${humanStepStatus(output.status)}`)} ${theme.fg("dim", artifactName(output.filePath))}`;
	}
	if (details.outputs.length > 1) return `${theme.fg("muted", "final outputs")} ${theme.fg("accent", `${details.outputs.length}`)} ${theme.fg("dim", summarizeOutputIds(details.outputs))}`;
	return `${theme.fg("muted", "last update")} ${theme.fg("dim", truncate(humanActivity(details.run?.lastEvent ?? "none"), VALUE_CHARS))}`;
}

function formatExpanded(details: AgentTeamDetails, theme: Theme): string {
	const artifactOutputs = details.notice?.terminal ? terminalArtifacts(details) : details.outputs;
	if (artifactOutputs.some((output) => output.filePath)) {
		const copy = details.notice?.terminal ? summarizeFullArtifactPaths(artifactOutputs) : summarizeArtifacts(artifactOutputs);
		return `${theme.fg("muted", "artifacts")} ${theme.fg("dim", copy)}`;
	}
	const errors = details.diagnostics.filter((diagnostic) => diagnostic.severity === "error").map((diagnostic) => diagnostic.code);
	if (errors.length > 0) return `${theme.fg("muted", "diagnostics")} ${theme.fg("error", errors.join(", "))}`;
	const tools = stepToolSummaries(details.steps);
	if (tools.length > 0) return `${theme.fg("muted", "tools")} ${theme.fg("dim", tools.join("; "))}`;
	return "";
}

function stepToolSummaries(steps: AgentTeamDetails["steps"]): string[] {
	return steps.slice(0, MAX_IDS).map((step) => `${step.id}=${step.effectiveTools.join(",") || "none"}`);
}

function summarizeOutputIds(outputs: AgentTeamDetails["outputs"]): string {
	const visible = outputs.slice(0, MAX_IDS).map((output) => `${output.stepId} ${humanStepStatus(output.status)}`).join(", ");
	return outputs.length > MAX_IDS ? `${visible}, +${outputs.length - MAX_IDS} more` : visible;
}

function summarizeNotice(details: AgentTeamDetails): string {
	const reasons = details.notice?.reasons.slice(0, MAX_IDS).join(",") || details.run?.lastEvent || "state changed";
	return truncate(reasons, VALUE_CHARS);
}

function startTarget(args: AgentTeamInput): string {
	const graphFile = stringProperty(args, "graphFile");
	if (graphFile) return graphFile;
	const graph = recordProperty(args, "graph");
	const steps = graph ? graph["steps"] : undefined;
	return `${Array.isArray(steps) ? steps.length : 0} step(s)`;
}

function catalogQuery(args: AgentTeamInput): string | undefined {
	const library = recordProperty(args, "library");
	return library ? stringProperty(library, "query") : undefined;
}

function recordProperty(value: unknown, key: string): Record<string, unknown> | undefined {
	if (!isRecord(value)) return undefined;
	const child = value[key];
	return isRecord(child) ? child : undefined;
}

function stringProperty(value: unknown, key: string): string | undefined {
	if (!isRecord(value)) return undefined;
	const child = value[key];
	return typeof child === "string" ? child : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null;
}

function formatNoticeArtifactCopy(details: AgentTeamDetails, theme: Theme): string {
	const artifacts = details.notice?.terminal ? terminalArtifacts(details) : details.outputs;
	if (artifacts.length === 0) return "";
	const artifactCopy = details.notice?.terminal ? `artifactPaths=${summarizeFullArtifactPaths(artifacts)}` : summarizeArtifacts(artifacts);
	return ` ${theme.fg("dim", artifactCopy)}`;
}

function formatNoticeExpiry(details: AgentTeamDetails, theme: Theme): string {
	return details.notice?.terminal && details.run?.expiresAt ? ` ${theme.fg("dim", `expiresAt=${details.run.expiresAt}`)}` : "";
}

function reuseText(component: Component | undefined): Text {
	return component instanceof Text ? component : new Text("", 0, 0);
}

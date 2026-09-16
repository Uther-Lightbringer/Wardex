/** Live human HUD helpers for the detached `agent_team` tool. */

import type { Theme } from "@earendil-works/pi-coding-agent";
import { truncateToWidth, type Component } from "@earendil-works/pi-tui";
import type { AgentTeamDetails, RunSnapshot, StepSnapshot, StepStatus } from "./types.ts";

const MAX_IDS = 3;
const MAX_LIVE_RUN_ROWS = 3;
const MAX_SINGLE_ACTIVE_ROWS = 5;
const MAX_SINGLE_ATTENTION_ROWS = 2;
const PROGRESS_BAR_WIDTH = 14;

export function renderAgentTeamLiveRunsWidget(details: AgentTeamDetails[], theme: Theme): Component {
	return new AgentTeamLiveRunsWidget(details, theme);
}

/** Live-only human HUD for interactive Pi widgets. */
export class AgentTeamLiveRunsWidget implements Component {
	private details: AgentTeamDetails[];
	private readonly theme: Theme;
	private readonly requestRender: (() => void) | undefined;
	private cachedWidth: number | undefined;
	private cachedLines: string[] | undefined;

	constructor(details: AgentTeamDetails[], theme: Theme, requestRender?: () => void) {
		this.details = details;
		this.theme = theme;
		this.requestRender = requestRender;
	}

	setDetails(details: AgentTeamDetails[]): void {
		this.details = details;
		this.invalidate();
		this.requestRender?.();
	}

	render(width: number): string[] {
		if (this.cachedWidth === width && this.cachedLines) return this.cachedLines;
		const safeWidth = Math.max(1, width);
		const lines = formatHumanLiveRunsWidget(this.details, this.theme).map((line) => fitLine(line, safeWidth));
		this.cachedWidth = width;
		this.cachedLines = lines;
		return lines;
	}

	invalidate(): void {
		this.cachedWidth = undefined;
		this.cachedLines = undefined;
	}
}

function formatHumanLiveRunsWidget(details: AgentTeamDetails[], theme: Theme): string[] {
	const live = liveRunDetails(details);
	if (live.length === 0) return [];
	if (live.length === 1) return formatSingleLiveRun(live[0], theme);
	return formatLiveRunBoard(live, theme);
}

function liveRunDetails(details: AgentTeamDetails[]): AgentTeamDetails[] {
	return details.filter((item) => item.run !== undefined && !item.run.terminal);
}

function formatSingleLiveRun(details: AgentTeamDetails, theme: Theme): string[] {
	const run = details.run;
	if (!run) return [];
	const attention = attentionLines(details, theme, MAX_SINGLE_ATTENTION_ROWS);
	const stateColor = attention.length > 0 ? "warning" : statusColor(run.status);
	const title = attention.length > 0 ? "" : ` ${theme.fg("dim", objectiveTitle(run.objective, 58))}`;
	const lines = [
		`${theme.fg("toolTitle", theme.bold("agent_team"))} ${theme.fg(stateColor, attention.length > 0 ? "attention" : humanRunStatus(run.status))}${title}`,
		`${progressMeter(run.counts, theme)} ${theme.fg("dim", liveProgressSummary(run.counts))}`,
	];
	if (attention.length > 0) lines.push(theme.fg("warning", "needs attention"), ...attention);
	const running = stepsByStatus(details, "running");
	if (running.length > 0) {
		lines.push(theme.fg("muted", "working now"), ...stepRows(running, theme, MAX_SINGLE_ACTIVE_ROWS));
	} else if (attention.length === 0) {
		lines.push(`${theme.fg("muted", "last update")} ${theme.fg("dim", humanActivity(run.lastEvent ?? "starting"))}`);
	}
	const pending = stepsByStatus(details, "pending");
	if (pending.length > 0) lines.push(`${theme.fg("muted", "queued next")} ${theme.fg("dim", pendingSummary(pending))}`);
	return lines;
}

function formatLiveRunBoard(details: AgentTeamDetails[], theme: Theme): string[] {
	const ordered = [...details].sort((left, right) => attentionRank(right) - attentionRank(left));
	const issues = ordered.filter(runNeedsAttention).length;
	const header = issues > 0 ? `${ordered.length} runs  ${issues} need attention` : `${ordered.length} runs`;
	const rows = ordered.slice(0, MAX_LIVE_RUN_ROWS).map((item) => formatLiveRunBoardRow(item, theme));
	if (ordered.length > MAX_LIVE_RUN_ROWS) rows.push(theme.fg("dim", `+${ordered.length - MAX_LIVE_RUN_ROWS} more runs`));
	return [`${theme.fg("toolTitle", theme.bold("agent_team"))} ${theme.fg(issues > 0 ? "warning" : "accent", header)}`, ...rows];
}

function formatLiveRunBoardRow(details: AgentTeamDetails, theme: Theme): string {
	const run = details.run;
	if (!run) return "";
	const attention = attentionSummary(details);
	const marker = attention ? "!" : run.status === "canceling" ? "~" : ">";
	const title = objectiveTitle(run.objective, 34) || shortRunId(run.runId);
	const tail = attention ?? focusedActivitySummary(details) ?? humanActivity(run.lastEvent ?? "starting");
	return `${theme.fg(attention ? "warning" : statusColor(run.status), marker)} ${theme.fg("text", title)} ${theme.fg("dim", liveProgressSummary(run.counts))} ${theme.fg("muted", tail)}`;
}

function runNeedsAttention(details: AgentTeamDetails): boolean {
	return attentionRank(details) > 0;
}

function attentionRank(details: AgentTeamDetails): number {
	const run = details.run;
	if (details.diagnostics.some((diagnostic) => diagnostic.severity === "error")) return 3;
	if (attentionSteps(details).length > 0) return 2;
	if (run?.status === "canceling") return 1;
	return 0;
}

function attentionSummary(details: AgentTeamDetails): string | undefined {
	const diagnostic = details.diagnostics.find((item) => item.severity === "error");
	if (diagnostic) return `diagnostic ${diagnostic.code}`;
	const step = attentionSteps(details)[0];
	if (step) return `${humanStepLabel(step)} ${humanStepStatus(step.status)}${step.errorMessage ? `: ${humanActivity(step.errorMessage)}` : ""}`;
	if (details.run?.status === "canceling") return "canceling live work";
	return undefined;
}

function attentionSteps(details: AgentTeamDetails): StepSnapshot[] {
	return details.steps.filter((step) => step.status === "failed" || step.status === "blocked" || step.status === "timed_out" || step.status === "canceled");
}

export function attentionLines(details: AgentTeamDetails, theme: Theme, maxRows: number): string[] {
	const rows = details.diagnostics.filter((diagnostic) => diagnostic.severity === "error").map((diagnostic) => `  ${theme.fg("warning", "!")} ${theme.fg("text", diagnostic.code)} ${theme.fg("dim", diagnostic.message)}`);
	for (const step of attentionSteps(details)) rows.push(formatStepRow(step, theme));
	return rows.slice(0, maxRows);
}

export function stepsByStatus(details: AgentTeamDetails, status: StepStatus): StepSnapshot[] {
	return details.steps.filter((step) => step.status === status);
}

function stepRows(steps: StepSnapshot[], theme: Theme, maxRows: number): string[] {
	const rows = steps.slice(0, maxRows).map((step) => formatStepRow(step, theme));
	if (steps.length > maxRows) rows.push(`  ${theme.fg("dim", `+${steps.length - maxRows} more lanes`)}`);
	return rows;
}

function formatStepRow(step: StepSnapshot, theme: Theme): string {
	const color = step.status === "running" ? "accent" : step.status === "pending" ? "muted" : statusColor(step.status);
	const marker = step.status === "running" ? ">" : step.status === "pending" ? "-" : "!";
	return `  ${theme.fg(color, marker)} ${theme.fg("text", humanStepLabel(step))} ${theme.fg("dim", stepActivity(step))}`;
}

export function focusedActivitySummary(details: AgentTeamDetails): string | undefined {
	const running = stepsByStatus(details, "running");
	if (running.length > 0) {
		const first = running[0];
		const suffix = running.length > 1 ? `; +${running.length - 1} more lanes` : "";
		return `${humanStepLabel(first)} ${stepActivity(first)}${suffix}`;
	}
	const pending = stepsByStatus(details, "pending");
	if (pending.length > 0) return `queued: ${pendingSummary(pending)}`;
	return undefined;
}

export function pendingSummary(steps: StepSnapshot[]): string {
	const visible = steps.slice(0, MAX_IDS).map(humanStepLabel).join(", ");
	return steps.length > MAX_IDS ? `${visible}, +${steps.length - MAX_IDS} more` : visible;
}

function stepActivity(step: StepSnapshot): string {
	if (step.errorMessage) return humanActivity(step.errorMessage);
	if (step.status === "pending") return step.needs.length > 0 ? `waiting for ${step.needs.join(", ")}` : "waiting";
	return step.lastActivity ? humanActivity(step.lastActivity) : "starting";
}

function humanStepLabel(step: StepSnapshot): string {
	const separator = step.agentRef.indexOf(":");
	const refName = separator >= 0 ? step.agentRef.slice(separator + 1) : step.agentRef;
	return refName.length > 0 && !refName.startsWith("inline:") ? refName : step.id;
}

export function humanActivity(text: string): string {
	return text
		.replace(/^tool\s+/, "")
		.replace(/^step\s+/, "")
		.replace(/^assistant writing$/u, "writing")
		.replace(/^child prompt sent$/u, "prompt sent")
		.replace(/^prompt accepted; waiting for child output$/u, "waiting for output")
		.replace(/^model turn active \(no output yet\)$/u, "thinking")
		.replace(/^child spawned$/u, "starting")
		.replace(/^assistant final$/u, "finalizing")
		.replace(/\s+/g, " ")
		.trim();
}

export function humanRunStatus(status: RunSnapshot["status"]): string {
	if (status === "canceling") return "canceling";
	return status;
}

export function humanStepStatus(status: StepStatus): string {
	if (status === "timed_out") return "timed out";
	return status;
}

export function liveProgressSummary(counts: Record<StepStatus, number>): string {
	const total = stepTotal(counts);
	const done = terminalStepCount(counts);
	const parts = [`${done}/${total} complete`];
	if (counts.failed > 0) parts.push(`${counts.failed} failed`);
	if (counts.blocked > 0) parts.push(`${counts.blocked} blocked`);
	if (counts.timed_out > 0) parts.push(`${counts.timed_out} timed out`);
	if (counts.canceled > 0) parts.push(`${counts.canceled} canceled`);
	if (counts.running > 0) parts.push(`${counts.running} working`);
	if (counts.pending > 0) parts.push(`${counts.pending} queued`);
	return parts.join("  ");
}

export function progressMeter(counts: Record<StepStatus, number>, theme: Theme): string {
	const total = Math.max(1, stepTotal(counts));
	const done = terminalStepCount(counts);
	const filled = Math.min(PROGRESS_BAR_WIDTH, Math.round((done / total) * PROGRESS_BAR_WIDTH));
	const meter = `[${"=".repeat(filled)}${"-".repeat(PROGRESS_BAR_WIDTH - filled)}]`;
	const color = counts.failed > 0 || counts.blocked > 0 || counts.timed_out > 0 ? "warning" : counts.running > 0 ? "accent" : "success";
	return theme.fg(color, meter);
}

function terminalStepCount(counts: Record<StepStatus, number>): number {
	return counts.succeeded + counts.failed + counts.blocked + counts.timed_out + counts.canceled;
}

function stepTotal(counts: Record<StepStatus, number>): number {
	return Object.values(counts).reduce((sum, count) => sum + count, 0);
}

export function objectiveTitle(objective: string, maxChars = 36): string {
	const normalized = objective.replace(/\s+/g, " ").trim();
	if (normalized.length <= maxChars) return normalized;
	return `${normalized.slice(0, Math.max(0, maxChars - 3))}...`;
}

function fitLine(line: string, width: number): string {
	return truncateToWidth(line, width, "…");
}

export function statusColor(status: string): "success" | "warning" | "error" | "accent" {
	if (status === "succeeded") return "success";
	if (status === "running" || status === "canceling") return "accent";
	if (status === "mixed" || status === "canceled") return "warning";
	return "error";
}

export function shortRunId(runId: string | undefined): string {
	if (!runId) return "";
	return runId.length <= 18 ? runId : `${runId.slice(0, 8)}...${runId.slice(-6)}`;
}

export function truncate(text: string, maxChars: number): string {
	const normalized = text.replace(/\s+/g, " ").trim();
	if (normalized.length <= maxChars) return normalized;
	return `${normalized.slice(0, Math.max(0, maxChars - 3))}...`;
}

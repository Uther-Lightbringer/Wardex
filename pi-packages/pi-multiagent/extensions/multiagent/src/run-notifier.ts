/** Compact pushed-notice scheduler for detached runs. */

import { safeRuntimeCallback } from "./runtime-diagnostics.ts";
import type { AgentTeamRuntimeOptions } from "./runtime-options.ts";
import { unrefTimer } from "./runtime-options.ts";
import type { AgentTeamDetails, AgentTeamNotice, NotifyOptions, RunStatus, StepSnapshot, StepStatus } from "./types.ts";

interface RunNotifierOptions {
	runId: string;
	notify: NotifyOptions;
	runtimeOptions: AgentTeamRuntimeOptions;
	recordDiagnostic: (code: string, label: string, message: string) => void;
	isTerminal: () => boolean;
	details: (notice: AgentTeamNotice) => AgentTeamDetails;
}

export function stepNoticeReasons(input: { stepId: string; status: StepStatus; sinkStepIds: string[] }): string[] {
	const reasons: string[] = [];
	if (input.sinkStepIds.includes(input.stepId)) reasons.push(`sink ${input.stepId} ${input.status}`);
	if (input.status === "failed" || input.status === "blocked" || input.status === "timed_out" || input.status === "canceled") reasons.push(`step ${input.stepId} ${input.status}`);
	return reasons;
}

export function terminalStepNoticeReasons(steps: StepSnapshot[]): string[] {
	return steps.filter((step) => step.status === "failed" || step.status === "blocked" || step.status === "timed_out").map((step) => `step ${step.id} ${step.status}`);
}

/** Coalesce non-terminal milestones and send exactly one enabled terminal notice. */
export class RunNotifier {
	private readonly options: RunNotifierOptions;
	private terminalSent = false;
	private milestoneCount = 0;
	private lastMilestoneAt = 0;
	private pendingReasons = new Set<string>();
	private timer: ReturnType<typeof setTimeout> | undefined;

	constructor(options: RunNotifierOptions) {
		this.options = options;
	}

	cancelTimers(): void {
		if (!this.timer) return;
		clearTimeout(this.timer);
		this.timer = undefined;
	}

	queueMilestone(reason: string): void {
		const notify = this.options.notify;
		if (notify.mode !== "milestones" || this.options.isTerminal()) return;
		if (notify.maxNotices <= this.milestoneCount) return;
		this.pendingReasons.add(reason);
		const nowMs = Date.now();
		const delayMs = Math.max(0, notify.minIntervalSeconds * 1000 - (nowMs - this.lastMilestoneAt));
		if (delayMs === 0) {
			this.flushMilestone();
			return;
		}
		if (this.timer) return;
		this.timer = setTimeout(() => this.flushMilestone(), delayMs);
		unrefTimer(this.timer);
	}

	sendTerminal(status: RunStatus, stepReasons: string[] = []): void {
		if (this.terminalSent || this.options.notify.mode === "none") return;
		this.terminalSent = true;
		this.pendingReasons.clear();
		this.cancelTimers();
		this.send(true, [`terminal:${status}`, ...stepReasons]);
	}

	private flushMilestone(): void {
		this.cancelTimers();
		const notify = this.options.notify;
		if (this.pendingReasons.size === 0 || notify.mode !== "milestones" || this.options.isTerminal()) return;
		if (notify.maxNotices <= this.milestoneCount) {
			this.pendingReasons.clear();
			return;
		}
		const reasons = [...this.pendingReasons];
		this.pendingReasons.clear();
		this.milestoneCount += 1;
		this.lastMilestoneAt = Date.now();
		this.send(false, reasons);
	}

	private send(terminal: boolean, reasons: string[]): void {
		const notice: AgentTeamNotice = {
			runId: this.options.runId,
			mode: this.options.notify.mode,
			terminal,
			reasons,
			noticeCount: terminal ? this.milestoneCount + 1 : this.milestoneCount,
			noticeLimitReached: !terminal && this.options.notify.maxNotices <= this.milestoneCount,
		};
		const errorMessage = safeRuntimeCallback(() => this.options.runtimeOptions.onRunNotice?.(this.options.details(notice)), "Could not send agent_team notice");
		if (errorMessage) this.options.recordDiagnostic("run-notice-callback-failed", "agent_team-notice", errorMessage);
	}
}

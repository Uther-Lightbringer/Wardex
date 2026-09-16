/** Compute a detached run terminal status from finalized step snapshots. */

import type { RunStatus, StepSnapshot } from "./types.ts";

export function terminalRunStatus(input: { currentStatus: RunStatus; expiryRequested: boolean; steps: StepSnapshot[] }): RunStatus {
	if (input.expiryRequested) return "expired";
	if (input.currentStatus === "canceling") return "canceled";
	if (input.steps.some((step) => step.status === "timed_out")) return "failed";
	if (input.steps.some((step) => step.status === "failed" || step.status === "blocked" || step.status === "canceled")) return input.steps.some((step) => step.status === "succeeded") ? "mixed" : "failed";
	return "succeeded";
}

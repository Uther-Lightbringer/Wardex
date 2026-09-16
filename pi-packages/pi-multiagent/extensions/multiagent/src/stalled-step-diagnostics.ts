import { isTerminalStepStatus } from "./detached-output.ts";
import type { StepStatus } from "./types.ts";

export interface StalledStepDependencySpec {
	id: string;
	needs: readonly string[];
	after: readonly string[];
}

export type DependencyStatus = StepStatus | "missing";

export function stalledStepBlockerMessage(step: StalledStepDependencySpec, statusOf: (id: string) => DependencyStatus): string {
	const reasons: string[] = [];
	const waitingNeeds = step.needs.map((id) => ({ id, status: statusOf(id) })).filter((item) => item.status !== "succeeded");
	const waitingAfter = step.after.map((id) => ({ id, status: statusOf(id) })).filter((item) => item.status === "missing" || !isTerminalStepStatus(item.status));
	if (waitingNeeds.length > 0) reasons.push(`needs waiting: ${formatDependencyStatuses(waitingNeeds)}`);
	if (waitingAfter.length > 0) reasons.push(`after waiting: ${formatDependencyStatuses(waitingAfter)}`);
	if (reasons.length === 0) reasons.push("no unmet dependencies visible; dependency-cycle hint: planning should reject cycles, so inspect graph state and diagnostics");
	return `No runnable steps remain for ${step.id}; ${reasons.join("; ")}.`;
}

function formatDependencyStatuses(items: { id: string; status: DependencyStatus }[]): string {
	return items.map((item) => `${item.id}=${item.status}`).join(", ");
}

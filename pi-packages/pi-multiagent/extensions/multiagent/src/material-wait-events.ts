/** Parent-visible events that should wake run_status(waitSeconds). */

import type { BackgroundEvent } from "./types.ts";

interface MaterialWaitTarget {
	stepId: string | undefined;
	sinkStepIds: readonly string[];
}

export function isMaterialRunStatusWaitEvent(event: BackgroundEvent, target: MaterialWaitTarget): boolean {
	if (isRunLifecycleEvent(event)) return true;
	if (event.type === "diagnostic" && event.status === "error") return event.stepId === undefined || target.stepId === undefined || event.stepId === target.stepId;
	if (target.stepId !== undefined && event.stepId !== target.stepId) return false;
	return event.type === "step" && event.label === "finish" && isMaterialStepFinish(event, target);
}

function isRunLifecycleEvent(event: BackgroundEvent): boolean {
	return event.type === "run" && (event.label === "terminal" || event.label === "cancel" || event.label === "expired");
}

function isMaterialStepFinish(event: BackgroundEvent, target: MaterialWaitTarget): boolean {
	if (event.status && event.status !== "succeeded") return true;
	if (target.stepId !== undefined) return true;
	return event.stepId !== undefined && target.sinkStepIds.includes(event.stepId);
}

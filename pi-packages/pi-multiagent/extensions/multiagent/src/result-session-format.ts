import type { StepOutput, StepSnapshot } from "./types.ts";
import { modelText } from "./result-model-text.ts";

function sessionValue(session: StepOutput["childSession"] | StepSnapshot["childSession"]): string | undefined {
	return session?.sessionFile ?? session?.sessionId ?? session?.sessionDir;
}

export function optionalChildSession(step: StepSnapshot): string {
	if (!step.childSession) return "";
	const value = sessionValue(step.childSession);
	return value ? ` childSession=${JSON.stringify(modelText(value))}` : " childSession=unavailable";
}

export function optionalRetryHistory(step: StepSnapshot): string {
	return step.retryHistory && step.retryHistory.length > 0 ? ` retries=${step.retryHistory.length}` : "";
}

export function formatOutputChildSession(output: StepOutput): string {
	if (!output.childSession) return "";
	const value = sessionValue(output.childSession);
	return value ? `Child session: ${JSON.stringify(modelText(value))}` : "Child session: unavailable";
}

export function compactOutputChildSession(output: StepOutput): string {
	if (!output.childSession) return "";
	const value = sessionValue(output.childSession);
	return value ? ` childSession=${JSON.stringify(modelText(value))}` : " childSession=unavailable";
}

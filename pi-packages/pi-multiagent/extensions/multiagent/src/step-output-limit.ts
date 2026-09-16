/** Per-step retained assistant-output limit normalization. */

import type { AgentDiagnostic, StepOutputLimit } from "./types.ts";
import { MAX_ASSISTANT_FINAL_MESSAGES_PER_STEP, MAX_STEP_OUTPUT_BYTES } from "./types.ts";

export const DEFAULT_STEP_OUTPUT_LIMIT: StepOutputLimit = {
	maxBytes: MAX_STEP_OUTPUT_BYTES,
	maxAssistantFinals: MAX_ASSISTANT_FINAL_MESSAGES_PER_STEP,
};

export function normalizeStepOutputLimit(input: Partial<StepOutputLimit> | undefined, diagnostics: AgentDiagnostic[], path: string): StepOutputLimit | undefined {
	const maxBytes = input?.maxBytes ?? DEFAULT_STEP_OUTPUT_LIMIT.maxBytes;
	const maxAssistantFinals = input?.maxAssistantFinals ?? DEFAULT_STEP_OUTPUT_LIMIT.maxAssistantFinals;
	const failures: string[] = [];
	if (!validInteger(maxBytes, 1, MAX_STEP_OUTPUT_BYTES)) failures.push(`maxBytes must be an integer from 1 to ${MAX_STEP_OUTPUT_BYTES}.`);
	if (!validInteger(maxAssistantFinals, 1, MAX_ASSISTANT_FINAL_MESSAGES_PER_STEP)) failures.push(`maxAssistantFinals must be an integer from 1 to ${MAX_ASSISTANT_FINAL_MESSAGES_PER_STEP}.`);
	if (failures.length > 0) {
		diagnostics.push({ code: "step-output-limit-invalid", message: `Invalid retained outputLimit: ${failures.join(" ")}`, path, severity: "error" });
		return undefined;
	}
	return { maxBytes, maxAssistantFinals };
}

export function nonDefaultStepOutputLimit(limit: StepOutputLimit): StepOutputLimit | undefined {
	return isDefaultStepOutputLimit(limit) ? undefined : limit;
}

export function isDefaultStepOutputLimit(limit: StepOutputLimit): boolean {
	return limit.maxBytes === DEFAULT_STEP_OUTPUT_LIMIT.maxBytes && limit.maxAssistantFinals === DEFAULT_STEP_OUTPUT_LIMIT.maxAssistantFinals;
}

export function formatStepOutputLimit(limit: StepOutputLimit): string {
	return `maxBytes=${limit.maxBytes}, maxAssistantFinals=${limit.maxAssistantFinals}`;
}

function validInteger(value: number, min: number, max: number): boolean {
	return Number.isInteger(value) && value >= min && value <= max;
}

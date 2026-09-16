import { childToolNames } from "./tool-policy.ts";
import { READONLY_CHILD_TOOL_NAMES, type TeamStepSpec } from "./types.ts";
import type { RpcStepResult } from "./rpc-child-types.ts";

export const MAX_TRANSPORT_RETRY_ATTEMPTS = 1;

const TRANSPORT_ERROR_PATTERNS = [
	/upstream connect error/i,
	/disconnect\/reset before headers/i,
	/reset reason: connection termination/i,
	/connection (?:reset|terminated|termination|closed)/i,
	/socket hang up/i,
	/ECONNRESET|ETIMEDOUT|EAI_AGAIN|ENETUNREACH|ECONNREFUSED/i,
	/502 Bad Gateway|503 Service Unavailable|504 Gateway Timeout/i,
	/fetch failed|network error/i,
];

export function shouldRetryTransportFailure(step: TeamStepSpec, result: RpcStepResult, attemptIndex: number): boolean {
	if (attemptIndex >= MAX_TRANSPORT_RETRY_ATTEMPTS) return false;
	if (result.parentMessagesAccepted === true) return false;
	if (result.status !== "failed") return false;
	if (result.assistantFinals.length > 0 || result.nonFinalText !== undefined || result.text.trim().length > 0) return false;
	if (!isReadOnlyIdempotentStep(step)) return false;
	return isRetryableTransportError(result.errorMessage);
}

export function isReadOnlyIdempotentStep(step: TeamStepSpec): boolean {
	if (step.agent.extensionTools.length > 0) return false;
	const readonly = new Set<string>(READONLY_CHILD_TOOL_NAMES);
	return childToolNames(step.agent).every((tool) => readonly.has(tool));
}

export function isRetryableTransportError(message: string | undefined): boolean {
	if (!message) return false;
	return TRANSPORT_ERROR_PATTERNS.some((pattern) => pattern.test(message));
}

/** Shared model-facing run_status wait receipt copy. */

import type { AgentTeamDetails } from "./types.ts";

export function formatWaitReceiptForModel(wait: AgentTeamDetails["wait"], text: (value: string) => string): string {
	if (!wait) return "";
	const target = wait.stepId ? ` step=${text(wait.stepId)}` : "";
	const cursor = `cursor ${text(wait.cursorBefore)} -> ${text(wait.cursorAfter)}`;
	if (wait.outcome === "timeout") return `Wait: timeout after ${wait.requestedSeconds}s${target}; no material event occurred; timeout is not a failure; ${cursor}.`;
	if (wait.outcome === "material") return `Wait: material event observed within ${wait.requestedSeconds}s${target}; ${cursor}.`;
	if (wait.outcome === "already-material") return `Wait: material event was already available at the requested cursor${target}; ${cursor}.`;
	return `Wait: run was already terminal before waiting${target}; ${cursor}.`;
}

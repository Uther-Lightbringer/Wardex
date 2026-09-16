/** Retained detached-run registry. */

import type { DetachedRun } from "./detached-run.ts";

const registry = new Map<string, DetachedRun>();

export function registerDetachedRun(run: DetachedRun): void {
	registry.set(run.id, run);
}

export function getDetachedRun(runId: string | undefined): DetachedRun | undefined {
	return runId ? registry.get(runId) : undefined;
}

export function forgetDetachedRun(runId: string): void {
	registry.delete(runId);
}

export function listDetachedRuns(): DetachedRun[] {
	return [...registry.values()];
}

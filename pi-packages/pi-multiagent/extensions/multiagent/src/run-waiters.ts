/** Bounded run_status waiters for detached run material events. */

import { isMaterialRunStatusWaitEvent } from "./material-wait-events.ts";
import { unrefTimer } from "./runtime-options.ts";
import type { BackgroundEvent, RunStatusWaitOutcome } from "./types.ts";

interface RunWaiter {
	stepId: string | undefined;
	sinkStepIds: readonly string[];
	resolve: (outcome: Extract<RunStatusWaitOutcome, "material" | "timeout">) => void;
}

export class RunWaiters {
	private readonly waiters = new Set<RunWaiter>();

	add(input: { stepId?: string; sinkStepIds: readonly string[]; milliseconds: number }): Promise<Extract<RunStatusWaitOutcome, "material" | "timeout">> {
		return new Promise((resolve) => {
			let timer: ReturnType<typeof setTimeout> | undefined;
			const waiter: RunWaiter = { stepId: input.stepId, sinkStepIds: input.sinkStepIds, resolve: finish };
			function finish(outcome: Extract<RunStatusWaitOutcome, "material" | "timeout">): void {
				if (timer) clearTimeout(timer);
				resolve(outcome);
			}
			this.waiters.add(waiter);
			timer = setTimeout(() => {
				this.waiters.delete(waiter);
				finish("timeout");
			}, input.milliseconds);
			unrefTimer(timer);
		});
	}

	notify(event: BackgroundEvent): void {
		for (const waiter of [...this.waiters]) {
			if (!isMaterialRunStatusWaitEvent(event, waiter)) continue;
			this.waiters.delete(waiter);
			waiter.resolve("material");
		}
	}
}

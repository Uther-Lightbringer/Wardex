import type { RpcChildEventInput } from "./rpc-child-types.ts";

const DEFAULT_PROGRESS_WATCHDOG_MS = 60_000;

export class RpcProgressWatchdog {
	private readonly timeoutMs: number;
	private readonly onEvent: (input: RpcChildEventInput) => void;
	private timer: ReturnType<typeof setInterval> | undefined;
	private lastProgressAt = Date.now();
	private lastProgressLabel = "spawn";
	private lastReportedAt = 0;

	constructor(input: { timeoutMs?: number; onEvent: (event: RpcChildEventInput) => void }) {
		this.timeoutMs = input.timeoutMs && input.timeoutMs > 0 ? Math.trunc(input.timeoutMs) : DEFAULT_PROGRESS_WATCHDOG_MS;
		this.onEvent = input.onEvent;
	}

	start(): void {
		if (this.timer) return;
		this.timer = setInterval(() => this.check(), Math.max(10, Math.floor(this.timeoutMs / 2)));
		this.timer.unref?.();
	}

	stop(): void {
		if (!this.timer) return;
		clearInterval(this.timer);
		this.timer = undefined;
	}

	progress(label: string): void {
		this.lastProgressAt = Date.now();
		this.lastProgressLabel = label;
	}

	private check(): void {
		const elapsed = Date.now() - this.lastProgressAt;
		this.onEvent({ type: "rpc", label: "heartbeat", preview: `quiet for ${Math.round(elapsed / 1000)}s since ${this.lastProgressLabel}`, status: "running" });
		if (elapsed < this.timeoutMs || this.lastReportedAt === this.lastProgressAt) return;
		this.lastReportedAt = this.lastProgressAt;
		this.onEvent({ type: "diagnostic", label: "progress-watchdog", preview: `no child RPC progress for ${Math.round(elapsed / 1000)}s since ${this.lastProgressLabel}`, status: "error" });
	}
}

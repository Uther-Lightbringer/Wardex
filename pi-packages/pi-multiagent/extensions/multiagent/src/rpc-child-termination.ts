import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { killProcessTree } from "./child-launch.ts";
import type { RpcStepResult } from "./rpc-child-types.ts";

const SIGKILL_CONFIRM_MS = 250;
const SIGTERM_GRACE_MS = 250;

export interface TerminateRpcChildInput {
	child: ChildProcessWithoutNullStreams | undefined;
	isTerminating: () => boolean;
	markTerminating: () => void;
	isChildClosed: () => boolean;
	isChildExited: () => boolean;
	closingResult: () => RpcStepResult | undefined;
	setKillTimer: (timer: ReturnType<typeof setTimeout>) => void;
	onEvent: (input: { type: "diagnostic"; label?: string; preview?: string; status?: string }) => void;
	complete: (result: RpcStepResult) => void;
}

export function terminateRpcChild(input: TerminateRpcChildInput): void {
	const child = input.child;
	if (!child || input.isTerminating() || input.isChildClosed() || input.isChildExited()) return;
	input.markTerminating();
	try {
		child.stdin.end();
	} catch (error) {
		input.onEvent({ type: "diagnostic", label: "stdin-end", preview: error instanceof Error ? error.message : "stdin end failed", status: "error" });
	}
	if (child.exitCode === null && !killProcessTree(child, "SIGTERM")) input.onEvent({ type: "diagnostic", label: "SIGTERM", preview: "not accepted", status: "error" });
	const timer = setTimeout(() => {
		if (input.isChildExited() || child.exitCode !== null) return;
		if (!killProcessTree(child, "SIGKILL")) input.onEvent({ type: "diagnostic", label: "SIGKILL", preview: "not accepted", status: "error" });
		const confirm = setTimeout(() => {
			const result = input.closingResult();
			if (input.isChildExited() || child.exitCode !== null || !result) return;
			input.onEvent({ type: "diagnostic", label: "SIGKILL", preview: `open after ${SIGKILL_CONFIRM_MS}ms; closeout forced`, status: "error" });
			input.complete(result);
		}, SIGKILL_CONFIRM_MS);
		confirm.unref?.();
		input.setKillTimer(confirm);
	}, SIGTERM_GRACE_MS);
	timer.unref?.();
	input.setKillTimer(timer);
}

import type { RpcChildEventInput, RpcStepResult } from "./rpc-child-types.ts";
import type { StepStatus } from "./types.ts";

const EXIT_CLOSE_GRACE_MS = 500;

export function scheduleRpcExitCloseout(input: { childClosed: () => boolean; finalized: () => boolean; closingResult: () => RpcStepResult | undefined; recoveringOverflow: () => boolean; onEvent: (input: RpcChildEventInput) => void; complete: (result: RpcStepResult) => void; fail: (status: StepStatus, message: string, label?: string) => void }): ReturnType<typeof setTimeout> {
	const timer = setTimeout(() => {
		if (input.childClosed() || input.finalized()) return;
		const closingResult = input.closingResult();
		if (closingResult) {
			input.onEvent({ type: "diagnostic", label: "process-exit", preview: `stdio close open ${EXIT_CLOSE_GRACE_MS}ms after exit; closeout forced`, status: "error" });
			input.complete(closingResult);
			return;
		}
		const recovering = input.recoveringOverflow();
		input.fail("failed", recovering ? "context-overflow-unrecovered: child process exited before a valid post-recovery assistant final." : "Subagent process exited before terminal agent_end.", recovering ? "context-overflow-unrecovered" : "failed");
	}, EXIT_CLOSE_GRACE_MS);
	timer.unref?.();
	return timer;
}

import { safeRuntimeCallback } from "./runtime-diagnostics.ts";

export function createRunUiCallback(onError: (message: string) => void): (callback: () => string | undefined) => void {
	let busy = false;
	return (callback) => {
		if (busy) return;
		busy = true;
		const message = safeRuntimeCallback(callback, "agent_team UI failed");
		if (message) onError(message);
		busy = false;
	};
}

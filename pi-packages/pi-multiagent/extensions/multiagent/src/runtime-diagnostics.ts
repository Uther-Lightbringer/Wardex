/** Runtime diagnostics for detached background callbacks. */

import type { BackgroundEventStore } from "./background-events.ts";
import type { AgentDiagnostic } from "./types.ts";

export function recordRuntimeDiagnostic(diagnostics: AgentDiagnostic[], events: BackgroundEventStore, code: string, label: string, message: string): void {
	if (!diagnostics.some((item) => item.code === code && item.message === message)) diagnostics.push({ code, message, path: undefined, severity: "warning" });
	events.append({ type: "diagnostic", label, preview: message, status: "error" });
}

export function safeRuntimeCallback(callback: () => string | undefined, failurePrefix: string): string | undefined {
	try {
		return callback();
	} catch (error) {
		return `${failurePrefix}: ${errorMessage(error)}`;
	}
}

function errorMessage(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

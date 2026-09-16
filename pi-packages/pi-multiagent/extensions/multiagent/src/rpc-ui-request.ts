import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { serializeRpcJsonLine, type RpcJsonRecord } from "./rpc-jsonl.ts";

const FIRE_AND_FORGET_UI_METHODS = new Set(["notify", "setStatus", "setWidget", "setTitle", "set_editor_text"]);
const BLOCKING_UI_METHODS = new Set(["select", "confirm", "input", "editor"]);

type UiEvent = { type: "ui" | "diagnostic"; label?: string; preview?: string; status?: string };

export interface UnattendedUiRequestOptions {
	record: RpcJsonRecord;
	child: ChildProcessWithoutNullStreams | undefined;
	finalized: boolean;
	onEvent: (input: UiEvent) => void;
	onDenied: (message: string) => void;
}

export function handleUnattendedUiRequest(options: UnattendedUiRequestOptions): void {
	const method = typeof options.record.method === "string" ? options.record.method : "unknown";
	const id = typeof options.record.id === "string" ? options.record.id : undefined;
	if (FIRE_AND_FORGET_UI_METHODS.has(method)) {
		options.onEvent({ type: "ui", label: method, preview: "UI request suppressed", status: "done" });
		return;
	}
	options.onEvent({ type: "ui", label: method, preview: "UI request denied", status: "error" });
	if (id && BLOCKING_UI_METHODS.has(method)) sendUiCancel(options.child, options.finalized, id, options.onEvent);
	options.onDenied(`Unattended extension UI request denied: ${method}.`);
}

function sendUiCancel(child: ChildProcessWithoutNullStreams | undefined, finalized: boolean, id: string, onEvent: (input: UiEvent) => void): void {
	if (!child || finalized) return;
	try {
		child.stdin.write(serializeRpcJsonLine({ type: "extension_ui_response", id, cancelled: true }));
	} catch (error) {
		onEvent({ type: "diagnostic", label: "ui-cancel", preview: error instanceof Error ? error.message : "UI cancel write failed", status: "error" });
	}
}

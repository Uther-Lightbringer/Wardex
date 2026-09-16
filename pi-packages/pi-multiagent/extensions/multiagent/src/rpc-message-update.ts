import { AssistantOutputBudget, type OutputBudgetFailure } from "./rpc-output-budget.ts";
import { isRecord, stringField } from "./rpc-record-utils.ts";
import type { RpcJsonRecord } from "./rpc-jsonl.ts";

export interface MessageUpdateHandlers {
	liveText: string;
	outputBudget: AssistantOutputBudget;
	failOutputBudget: (failure: OutputBudgetFailure) => void;
	onText: ((text: string) => void) | undefined;
	onEvent: (input: { type: "rpc"; label?: string; preview?: string; status?: string }) => void;
}

export function handleAssistantMessageUpdate(record: RpcJsonRecord, handlers: MessageUpdateHandlers): string {
	const event = isRecord(record.assistantMessageEvent) ? record.assistantMessageEvent : undefined;
	const eventType = typeof event?.type === "string" ? event.type : "unknown";
	if (eventType === "start") {
		handlers.outputBudget.resetLiveText();
		handlers.onText?.("");
		handlers.onEvent({ type: "rpc", label: "message_start", preview: "assistant started", status: "running" });
		return "";
	}
	if (eventType === "text_delta") {
		const delta = stringField(event?.delta);
		if (delta === undefined) return handlers.liveText;
		const check = handlers.outputBudget.appendLiveTextDelta(delta);
		if (!check.ok) {
			handlers.failOutputBudget(check.failure);
			return handlers.liveText;
		}
		const liveText = `${handlers.liveText}${delta}`;
		handlers.onText?.(liveText);
		return liveText;
	}
	if (eventType === "text_end") {
		const content = stringField(event?.content);
		if (content === undefined) return handlers.liveText;
		const check = handlers.outputBudget.measureText(content, "assistant text_end");
		if (!check.ok) {
			handlers.failOutputBudget(check.failure);
			return handlers.liveText;
		}
		handlers.outputBudget.setLiveTextBytes(check.bytes);
		handlers.onText?.(content);
		return content;
	}
	const activity = nonTextActivity(eventType);
	if (activity) handlers.onEvent(activity);
	return handlers.liveText;
}

function nonTextActivity(eventType: string): { type: "rpc"; label: string; preview: string; status: string } | undefined {
	if (eventType === "thinking_start") return { type: "rpc", label: "thinking", preview: "reasoning started", status: "running" };
	if (eventType === "thinking_end") return { type: "rpc", label: "thinking", preview: "reasoning ended", status: "done" };
	if (eventType === "toolcall_start") return { type: "rpc", label: "toolcall", preview: "tool call started", status: "running" };
	if (eventType === "toolcall_end") return { type: "rpc", label: "toolcall", preview: "tool call ended", status: "done" };
	if (eventType === "tool_use") return { type: "rpc", label: "tool_use", preview: "assistant tool activity", status: "running" };
	if (eventType.endsWith("_delta")) return undefined;
	return { type: "rpc", label: `message_update:${eventType}`, preview: "assistant activity", status: "running" };
}

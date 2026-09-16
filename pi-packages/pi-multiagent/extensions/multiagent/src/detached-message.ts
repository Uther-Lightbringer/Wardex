import { isTerminalRunStatus, isTerminalStepStatus } from "./detached-output.ts";
import type { StepState } from "./detached-state.ts";
import type { MessageReceiptCache } from "./message-idempotency.ts";
import type { DetachedRunEventInput } from "./detached-run-options.ts";
import type { MessageChannel, MessageReceipt, RunStatus } from "./types.ts";

export async function sendDetachedMessage(input: {
	runId: string;
	runStatus: RunStatus;
	states: Map<string, StepState>;
	receipts: MessageReceiptCache;
	stepId: string;
	channel: MessageChannel;
	text: string;
	clientMessageId: string | undefined;
	appendEvent: (event: DetachedRunEventInput) => void;
}): Promise<MessageReceipt> {
	const { runId, runStatus, states, receipts, stepId, channel, text, clientMessageId, appendEvent } = input;
	const state = states.get(stepId);
	if (!state) return messageReceipt(runId, appendEvent, stepId, channel, clientMessageId, false, "No step in the retained detached run matches stepId.");
	const hit = receipts.lookup(stepId, channel, text, clientMessageId);
	if (hit === "conflict") return messageReceipt(runId, appendEvent, stepId, channel, clientMessageId, false, "Conflicting clientMessageId");
	if (hit) return hit;
	let done: (receipt: MessageReceipt) => void = () => {};
	const pending = new Promise<MessageReceipt>((resolve) => { done = resolve; });
	const reserved = receipts.reserve(stepId, channel, text, clientMessageId, pending);
	if (reserved === "conflict") return messageReceipt(runId, appendEvent, stepId, channel, clientMessageId, false, "Conflicting clientMessageId");
	if (reserved) return reserved;
	const settle = (receipt: MessageReceipt) => {
		receipts.settle(stepId, channel, text, clientMessageId, receipt);
		done(receipt);
		return receipt;
	};
	if (isTerminalStepStatus(state.status) || isTerminalRunStatus(runStatus)) return settle(messageReceipt(runId, appendEvent, stepId, channel, clientMessageId, false, "Step is terminal; message is live-only and cannot resurrect completed work."));
	if (!state.controller || state.status !== "running" || runStatus !== "running") return settle(messageReceipt(runId, appendEvent, stepId, channel, clientMessageId, false, "Step not live or messageable."));
	try {
		const ack = await state.controller.message(channel, text);
		return settle(messageReceipt(runId, appendEvent, stepId, channel, clientMessageId, ack.success, ack.error, false));
	} catch (error) {
		return settle(messageReceipt(runId, appendEvent, stepId, channel, clientMessageId, false, error instanceof Error ? error.message : String(error)));
	}
}

function messageReceipt(runId: string, appendEvent: (event: DetachedRunEventInput) => void, stepId: string, channel: MessageChannel, clientMessageId: string | undefined, accepted: boolean, undeliveredReason: string | undefined, recordEvent = true): MessageReceipt {
	if (recordEvent) appendEvent({ stepId, type: "parent_message", label: channel, preview: accepted ? "accepted for delivery" : undeliveredReason, status: accepted ? "done" : "error" });
	return { runId, stepId, channel, clientMessageId, accepted, undeliveredReason, reused: false };
}

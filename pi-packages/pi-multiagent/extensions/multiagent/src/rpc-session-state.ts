import { dirname } from "node:path";
import type { ChildSessionMetadata, MessageChannel } from "./types.ts";
import type { RpcJsonRecord } from "./rpc-jsonl.ts";

export interface RpcStateSnapshot {
	sessionId: string | undefined;
	sessionFile: string | undefined;
	sessionName: string | undefined;
	pendingMessageCount: number | undefined;
	isStreaming: boolean | undefined;
}

export function childSessionFromState(data: unknown, launchSessionDir: string | undefined): ChildSessionMetadata | undefined {
	const state = rpcStateSnapshot(data);
	if (!state.sessionId && !state.sessionFile && !state.sessionName && !launchSessionDir) return undefined;
	return {
		sessionId: state.sessionId,
		sessionName: state.sessionName,
		sessionFile: state.sessionFile,
		sessionDir: state.sessionFile ? dirname(state.sessionFile) : launchSessionDir,
		launchSessionDir,
		stateSource: "rpc_get_state",
	};
}

export function rpcStateSnapshot(data: unknown): RpcStateSnapshot {
	if (!isRecord(data)) return { sessionId: undefined, sessionFile: undefined, sessionName: undefined, pendingMessageCount: undefined, isStreaming: undefined };
	return {
		sessionId: stringField(data.sessionId),
		sessionFile: stringField(data.sessionFile),
		sessionName: stringField(data.sessionName),
		pendingMessageCount: numberField(data.pendingMessageCount),
		isStreaming: booleanField(data.isStreaming),
	};
}

export interface QueueUpdateSnapshot {
	steering: string[];
	followUp: string[];
}

export function queueUpdateSnapshot(record: RpcJsonRecord): QueueUpdateSnapshot | undefined {
	if (record.type !== "queue_update") return undefined;
	return { steering: stringArray(record.steering), followUp: stringArray(record.followUp) };
}

export interface ParentMessageObservation {
	type: "parent_message" | "diagnostic";
	label: string;
	preview: string;
	status: string;
}

interface TrackedParentMessage {
	channel: MessageChannel;
	text: string;
	queuedObserved: boolean;
	consumedObserved: boolean;
	closed: boolean;
}

export class ParentMessageObservability {
	private readonly pending: TrackedParentMessage[] = [];

	accepted(channel: MessageChannel, text: string): ParentMessageObservation[] {
		this.pending.push({ channel, text, queuedObserved: false, consumedObserved: false, closed: false });
		return [{ type: "parent_message", label: channel, preview: "accepted for delivery; delivery_state=transport-accepted", status: "done" }];
	}

	denied(channel: MessageChannel, error: string | undefined): ParentMessageObservation[] {
		return [{ type: "parent_message", label: channel, preview: error ?? "delivery denied", status: "error" }];
	}

	queueUpdate(update: QueueUpdateSnapshot): ParentMessageObservation[] {
		const observations: ParentMessageObservation[] = [];
		const steering = countMessages(update.steering);
		const followUp = countMessages(update.followUp);
		for (const message of this.pending) {
			if (message.closed) continue;
			const queueCounts = message.channel === "steer" ? steering : followUp;
			const queuedNow = takeQueuedOccurrence(queueCounts, message.text);
			if (queuedNow && !message.queuedObserved) {
				message.queuedObserved = true;
				observations.push({ type: "parent_message", label: `${message.channel}_queued`, preview: "Pi queue contains accepted parent message", status: "running" });
			} else if (!queuedNow && message.queuedObserved && !message.consumedObserved) {
				message.consumedObserved = true;
				message.closed = true;
				observations.push({ type: "parent_message", label: `${message.channel}_consumed`, preview: "Pi queue removed the message before a user-message turn; semantic compliance remains unproven", status: "done" });
			}
		}
		return observations;
	}

	terminalNoRetry(): ParentMessageObservation[] {
		const observations: ParentMessageObservation[] = [];
		for (const message of this.pending) {
			if (message.closed) continue;
			message.closed = true;
			const label = message.queuedObserved ? `${message.channel}_no_next_turn` : `${message.channel}_consumption_unproven`;
			const preview = message.queuedObserved ? "child reached terminal agent_end before Pi consumed the queued parent message" : "child reached terminal agent_end before any queue-consumption proof was observed";
			observations.push({ type: "diagnostic", label, preview, status: "error" });
		}
		return observations;
	}
}

function countMessages(messages: string[]): Map<string, number> {
	const counts = new Map<string, number>();
	for (const message of messages) counts.set(message, (counts.get(message) ?? 0) + 1);
	return counts;
}

function takeQueuedOccurrence(counts: Map<string, number>, message: string): boolean {
	const count = counts.get(message) ?? 0;
	if (count <= 0) return false;
	if (count === 1) counts.delete(message);
	else counts.set(message, count - 1);
	return true;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null;
}

function stringField(value: unknown): string | undefined {
	return typeof value === "string" && value.trim().length > 0 ? value : undefined;
}

function numberField(value: unknown): number | undefined {
	return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function booleanField(value: unknown): boolean | undefined {
	return typeof value === "boolean" ? value : undefined;
}

function stringArray(value: unknown): string[] {
	return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

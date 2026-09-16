import type { ChildSessionMetadata, MessageChannel } from "./types.ts";
import type { RpcCommandAck } from "./rpc-command-queue.ts";
import type { RpcJsonRecord } from "./rpc-jsonl.ts";
import { childSessionFromState, ParentMessageObservability, queueUpdateSnapshot, type ParentMessageObservation } from "./rpc-session-state.ts";
import { RpcProgressWatchdog } from "./rpc-progress-watchdog.ts";
import type { RpcChildEventInput } from "./rpc-child-types.ts";

export class RpcChildObservability {
	private readonly sessionDir: string | undefined;
	private readonly onEvent: (input: RpcChildEventInput) => void;
	private readonly onChildSession: ((metadata: ChildSessionMetadata) => void) | undefined;
	private readonly watchdog: RpcProgressWatchdog;
	private readonly parentMessages = new ParentMessageObservability();
	private childSession: ChildSessionMetadata | undefined;
	private acceptedParentMessage = false;

	constructor(input: { sessionDir?: string; progressWatchdogMs?: number; onEvent: (input: RpcChildEventInput) => void; onChildSession?: (metadata: ChildSessionMetadata) => void }) {
		this.sessionDir = input.sessionDir;
		this.onEvent = input.onEvent;
		this.onChildSession = input.onChildSession;
		this.watchdog = new RpcProgressWatchdog({ timeoutMs: input.progressWatchdogMs, onEvent: input.onEvent });
	}

	get session(): ChildSessionMetadata | undefined {
		return this.childSession;
	}

	get parentMessagesAccepted(): boolean {
		return this.acceptedParentMessage;
	}

	start(): void {
		this.watchdog.start();
	}

	stop(): void {
		this.watchdog.stop();
	}

	progress(label: string): void {
		this.watchdog.progress(label);
	}

	async captureChildSession(send: (command: RpcJsonRecord) => Promise<RpcCommandAck>): Promise<void> {
		const ack = await send({ type: "get_state" });
		if (ack.success) {
			this.setChildSession(childSessionFromState(ack.data, this.sessionDir) ?? unavailableChildSession(this.sessionDir));
			const sessionFile = this.childSession?.sessionFile ? ` sessionFile=${this.childSession.sessionFile}` : "";
			this.onEvent({ type: "rpc", label: "child_session", preview: `sessionId=${this.childSession?.sessionId ?? "unknown"}${sessionFile}`, status: "done" });
			return;
		}
		this.setChildSession(unavailableChildSession(this.sessionDir));
		this.onEvent({ type: "diagnostic", label: "child_session_unavailable", preview: ack.error ?? "get_state did not return child session metadata", status: "done" });
	}

	messageSending(channel: MessageChannel): void {
		this.onEvent({ type: "parent_message", label: channel, preview: "sending to live child", status: "running" });
	}

	messageAck(channel: MessageChannel, message: string, ack: RpcCommandAck): void {
		if (ack.success) this.acceptedParentMessage = true;
		this.emit(ack.success ? this.parentMessages.accepted(channel, message) : this.parentMessages.denied(channel, ack.error));
	}

	handleQueueUpdate(record: RpcJsonRecord): boolean {
		const update = queueUpdateSnapshot(record);
		if (!update) return false;
		this.onEvent({ type: "rpc", label: "queue_update", preview: `steering=${update.steering.length} followUp=${update.followUp.length}`, status: "running" });
		this.emit(this.parentMessages.queueUpdate(update));
		return true;
	}

	terminalNoRetry(): void {
		this.emit(this.parentMessages.terminalNoRetry());
	}

	private setChildSession(metadata: ChildSessionMetadata): void {
		this.childSession = metadata;
		this.onChildSession?.(metadata);
	}

	private emit(observations: ParentMessageObservation[]): void {
		for (const observation of observations) this.onEvent(observation);
	}
}

function unavailableChildSession(launchSessionDir: string | undefined): ChildSessionMetadata {
	return { sessionId: undefined, sessionName: undefined, sessionFile: undefined, sessionDir: undefined, launchSessionDir, stateSource: "unavailable" };
}

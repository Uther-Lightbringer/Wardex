import { serializeRpcJsonLine, type RpcJsonRecord } from "./rpc-jsonl.ts";
import { stringField } from "./rpc-record-utils.ts";

export interface RpcCommandAck {
	success: boolean;
	error: string | undefined;
	data?: unknown;
}

interface PendingRpcCommand {
	command: string;
	resolve: (ack: RpcCommandAck) => void;
	timer: ReturnType<typeof setTimeout> | undefined;
	cleanup: (() => void) | undefined;
}

interface RpcCommandWriter {
	write: (line: string) => boolean;
	once: ((event: "drain", listener: () => void) => RpcCommandWriter) & ((event: "error", listener: (error: Error) => void) => RpcCommandWriter);
	off: ((event: "drain", listener: () => void) => RpcCommandWriter) & ((event: "error", listener: (error: Error) => void) => RpcCommandWriter);
}

export class RpcCommandQueue {
	private readonly timeoutMs: number;
	private readonly pending = new Map<string, PendingRpcCommand>();
	private nextCommandId = 0;

	constructor(timeoutMs: number) {
		this.timeoutMs = timeoutMs;
	}

	get pendingCount(): number {
		return this.pending.size;
	}

	send(stdin: RpcCommandWriter | undefined, command: RpcJsonRecord): Promise<RpcCommandAck> {
		if (!stdin) return Promise.resolve({ success: false, error: "RPC child is not live." });
		const id = `cmd-${++this.nextCommandId}`;
		const commandName = typeof command.type === "string" ? command.type : "unknown";
		const payload = { id, ...command };
		const ack = new Promise<RpcCommandAck>((resolve) => {
			this.pending.set(id, { command: commandName, resolve, timer: undefined, cleanup: undefined });
		});
		try {
			const flushed = stdin.write(serializeRpcJsonLine(payload));
			if (flushed) this.startAckTimer(id, commandName);
			else this.waitForDrain(stdin, id, commandName);
		} catch (error) {
			this.resolvePending(id, { success: false, error: `RPC stdin write failed: ${error instanceof Error ? error.message : String(error)}` });
		}
		return ack;
	}

	handleResponse(record: RpcJsonRecord, onDiagnostic: (input: { command: string; message: string | undefined }) => void): void {
		const id = typeof record.id === "string" ? record.id : undefined;
		const command = typeof record.command === "string" ? record.command : "unknown";
		if (!id) {
			if (record.success === false) onDiagnostic({ command, message: stringField(record.error) });
			return;
		}
		const pending = this.pending.get(id);
		if (!pending) {
			onDiagnostic({ command, message: `Unexpected RPC response id ${id}.` });
			return;
		}
		const ack: RpcCommandAck = { success: record.success === true, error: record.success === true ? undefined : stringField(record.error) ?? `RPC command ${command} failed.` };
		if (Object.hasOwn(record, "data")) ack.data = record.data;
		this.resolvePending(id, ack);
	}

	closeWith(errorForCommand: (command: string) => string): void {
		for (const [id, pending] of this.pending.entries()) this.resolvePending(id, { success: false, error: errorForCommand(pending.command) });
	}

	private waitForDrain(stdin: RpcCommandWriter, id: string, commandName: string): void {
		const onDrain = () => {
			const pending = this.pending.get(id);
			if (!pending) return;
			if (pending.timer) clearTimeout(pending.timer);
			pending.timer = undefined;
			pending.cleanup?.();
			pending.cleanup = undefined;
			this.startAckTimer(id, commandName);
		};
		const onError = (error: Error) => {
			this.resolvePending(id, { success: false, error: `RPC stdin write failed before drain: ${error.message}` });
		};
		const pending = this.pending.get(id);
		if (!pending) return;
		pending.cleanup = () => {
			stdin.off("drain", onDrain);
			stdin.off("error", onError);
		};
		pending.timer = setTimeout(() => {
			this.resolvePending(id, { success: false, error: `RPC stdin write did not drain before timeout for command ${commandName}.` });
		}, this.timeoutMs);
		pending.timer.unref?.();
		stdin.once("drain", onDrain);
		stdin.once("error", onError);
	}

	private startAckTimer(id: string, commandName: string): void {
		const pending = this.pending.get(id);
		if (!pending || pending.timer) return;
		pending.timer = setTimeout(() => {
			this.resolvePending(id, { success: false, error: `RPC command ${commandName} timed out waiting for response.` });
		}, this.timeoutMs);
		pending.timer.unref?.();
	}

	private resolvePending(id: string, ack: RpcCommandAck): void {
		const pending = this.pending.get(id);
		if (!pending) return;
		if (pending.timer) clearTimeout(pending.timer);
		pending.cleanup?.();
		this.pending.delete(id);
		pending.resolve(ack);
	}
}

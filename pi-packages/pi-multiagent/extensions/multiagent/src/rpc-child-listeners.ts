import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { attachRpcJsonlReader, type RpcJsonRecord, type RpcJsonlReader } from "./rpc-jsonl.ts";

export interface RpcChildListenerCallbacks {
	onRecord: (record: RpcJsonRecord) => void;
	onStdoutError: (message: string) => void;
	onStderrData: (text: string) => void;
	onStderrError: (error: Error) => void;
	onStdinError: (error: Error) => void;
	onChildError: (error: Error) => void;
	onExit: () => void;
	onClose: () => void;
}

export class RpcChildListeners {
	private child: ChildProcessWithoutNullStreams | undefined;
	private stdoutReader: RpcJsonlReader | undefined;
	private stderrDataListener: ((chunk: Buffer | string) => void) | undefined;
	private stdoutErrorGuard: ((error: Error) => void) | undefined;
	private stderrErrorListener: ((error: Error) => void) | undefined;
	private stderrErrorGuard: ((error: Error) => void) | undefined;
	private stdinErrorListener: ((error: Error) => void) | undefined;
	private stdinErrorGuard: ((error: Error) => void) | undefined;
	private childErrorListener: ((error: Error) => void) | undefined;
	private childErrorGuard: ((error: Error) => void) | undefined;
	private childExitListener: (() => void) | undefined;
	private childCloseListener: (() => void) | undefined;
	private closeCleanupListener: (() => void) | undefined;

	attach(child: ChildProcessWithoutNullStreams, callbacks: RpcChildListenerCallbacks): void {
		this.child = child;
		this.stdoutReader = attachRpcJsonlReader(child.stdout, callbacks.onRecord, callbacks.onStdoutError);
		this.stderrDataListener = (chunk: Buffer | string) => callbacks.onStderrData(typeof chunk === "string" ? chunk : chunk.toString("utf8"));
		this.stderrErrorListener = callbacks.onStderrError;
		this.stdinErrorListener = callbacks.onStdinError;
		this.childErrorListener = callbacks.onChildError;
		this.childExitListener = callbacks.onExit;
		this.childCloseListener = callbacks.onClose;
		child.stderr.on("data", this.stderrDataListener);
		child.stderr.on("error", this.stderrErrorListener);
		child.stdin.on("error", this.stdinErrorListener);
		child.on("error", this.childErrorListener);
		child.on("exit", this.childExitListener);
		child.on("close", this.childCloseListener);
	}

	detachIo(includeStdin: boolean, keepErrorGuards = false): void {
		this.stdoutReader?.detach();
		this.stdoutReader = undefined;
		if (this.child && this.stderrDataListener) this.child.stderr.off("data", this.stderrDataListener);
		if (this.child && this.stderrErrorListener) this.child.stderr.off("error", this.stderrErrorListener);
		if (keepErrorGuards) this.installErrorGuards();
		else this.detachErrorGuards();
		if (includeStdin && this.child && this.stdinErrorListener) this.child.stdin.off("error", this.stdinErrorListener);
		this.stderrDataListener = undefined;
		this.stderrErrorListener = undefined;
		if (includeStdin) this.stdinErrorListener = undefined;
	}

	detachForCompletion(keepErrorGuards: boolean): void {
		this.detachIo(true, keepErrorGuards);
		if (keepErrorGuards) this.installCloseCleanup();
		this.detachProcess();
	}

	private installErrorGuards(): void {
		if (!this.child) return;
		if (!this.stdoutErrorGuard) {
			this.stdoutErrorGuard = () => undefined;
			this.child.stdout.on("error", this.stdoutErrorGuard);
		}
		if (!this.stderrErrorGuard) {
			this.stderrErrorGuard = () => undefined;
			this.child.stderr.on("error", this.stderrErrorGuard);
		}
		if (!this.stdinErrorGuard) {
			this.stdinErrorGuard = () => undefined;
			this.child.stdin.on("error", this.stdinErrorGuard);
		}
		if (!this.childErrorGuard) {
			this.childErrorGuard = () => undefined;
			this.child.on("error", this.childErrorGuard);
		}
	}

	private detachErrorGuards(): void {
		if (this.child && this.stdoutErrorGuard) this.child.stdout.off("error", this.stdoutErrorGuard);
		if (this.child && this.stderrErrorGuard) this.child.stderr.off("error", this.stderrErrorGuard);
		if (this.child && this.stdinErrorGuard) this.child.stdin.off("error", this.stdinErrorGuard);
		if (this.child && this.childErrorGuard) this.child.off("error", this.childErrorGuard);
		this.stdoutErrorGuard = undefined;
		this.stderrErrorGuard = undefined;
		this.stdinErrorGuard = undefined;
		this.childErrorGuard = undefined;
	}

	private installCloseCleanup(): void {
		if (!this.child || this.closeCleanupListener) return;
		this.closeCleanupListener = () => {
			this.detachIo(true);
			this.detachProcess();
			this.closeCleanupListener = undefined;
		};
		this.child.once("close", this.closeCleanupListener);
	}

	detachProcess(): void {
		if (this.child && this.childErrorListener) this.child.off("error", this.childErrorListener);
		if (this.child && this.childExitListener) this.child.off("exit", this.childExitListener);
		if (this.child && this.childCloseListener) this.child.off("close", this.childCloseListener);
		this.childErrorListener = undefined;
		this.childExitListener = undefined;
		this.childCloseListener = undefined;
	}
}

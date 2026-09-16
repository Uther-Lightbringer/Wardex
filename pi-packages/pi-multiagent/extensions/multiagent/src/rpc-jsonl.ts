/** Strict LF-delimited JSONL framing for Pi RPC child processes. */

import { StringDecoder } from "node:string_decoder";
import type { Readable } from "node:stream";
import { RPC_RECORD_MAX_CHARS } from "./types.ts";

export type RpcJsonRecord = Record<string, unknown>;

export interface RpcJsonlReader {
	detach: () => void;
	end: () => void;
}

export function serializeRpcJsonLine(value: RpcJsonRecord): string {
	return `${JSON.stringify(value)}\n`;
}

export function attachRpcJsonlReader(stream: Readable, onRecord: (record: RpcJsonRecord) => void, onError: (message: string) => void, maxRecordBytes = RPC_RECORD_MAX_CHARS): RpcJsonlReader {
	const decoder = new StringDecoder("utf8");
	let buffer = "";
	let failed = false;
	let detached = false;
	const detach = () => {
		if (detached) return;
		detached = true;
		stream.off("data", onData);
		stream.off("end", onEnd);
		stream.off("error", onStreamError);
	};
	const fail = (message: string) => {
		if (failed) return;
		failed = true;
		detach();
		onError(message);
	};
	const processLine = (line: string) => {
		if (failed) return;
		const normalized = line.endsWith("\r") ? line.slice(0, -1) : line;
		if (Buffer.byteLength(normalized, "utf8") > maxRecordBytes) {
			fail(`RPC JSONL record exceeded ${maxRecordBytes} bytes.`);
			return;
		}
		let parsed: unknown;
		try {
			parsed = JSON.parse(normalized);
		} catch (error) {
			fail(`RPC JSONL parse failed: ${error instanceof Error ? error.message : String(error)}`);
			return;
		}
		if (!isRecord(parsed)) {
			fail("RPC JSONL record must be a JSON object.");
			return;
		}
		onRecord(parsed);
	};
	const processBufferedLines = () => {
		let newline = buffer.indexOf("\n");
		while (newline !== -1) {
			const line = buffer.slice(0, newline);
			buffer = buffer.slice(newline + 1);
			processLine(line);
			if (failed) return;
			newline = buffer.indexOf("\n");
		}
		if (Buffer.byteLength(buffer, "utf8") > maxRecordBytes) fail(`RPC JSONL pending record exceeded ${maxRecordBytes} bytes.`);
	};
	const onData = (chunk: Buffer | string) => {
		if (failed) return;
		buffer += typeof chunk === "string" ? chunk : decoder.write(chunk);
		processBufferedLines();
	};
	const onEnd = () => {
		if (failed) return;
		buffer += decoder.end();
		processBufferedLines();
		if (!failed && buffer.length > 0) fail("RPC JSONL stream ended with an unterminated record.");
		if (!failed) detach();
	};
	const onStreamError = (error: Error) => fail(`RPC JSONL stream error: ${error.message}`);
	stream.on("data", onData);
	stream.on("end", onEnd);
	stream.on("error", onStreamError);
	return { detach, end: onEnd };
}

function isRecord(value: unknown): value is RpcJsonRecord {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

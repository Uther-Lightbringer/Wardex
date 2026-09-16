import { stringField } from "./rpc-record-utils.ts";
import type { RpcJsonRecord } from "./rpc-jsonl.ts";
import { toolErrorPreview } from "./rpc-tool-events.ts";
import type { RpcChildEventInput } from "./rpc-child-types.ts";

export function handleRpcToolEvent(type: string, record: RpcJsonRecord, onEvent: (input: RpcChildEventInput) => void): void {
	const name = stringField(record.toolName) ?? "tool";
	const isEnd = type === "tool_execution_end";
	const isError = isEnd && record.isError === true;
	const status = isEnd ? (isError ? "error" : "done") : "running";
	const preview = isError ? toolErrorPreview(record) : type.replace("tool_execution_", "");
	onEvent({ type: "tool", label: name, preview, status });
}

/** Child tool event formatting helpers. */

import { extractEventText, isRecord, stringField } from "./rpc-record-utils.ts";
import type { RpcJsonRecord } from "./rpc-jsonl.ts";

export function toolErrorPreview(record: RpcJsonRecord): string {
	return firstNonBlank(toolResultText(record.result), stringField(record.error), extractEventText(record)) ?? "error";
}

export function firstNonBlank(...values: (string | undefined)[]): string | undefined {
	for (const value of values) {
		if (value !== undefined && value.trim().length > 0) return value;
	}
	return undefined;
}

function toolResultText(result: unknown): string | undefined {
	if (!isRecord(result)) return undefined;
	if (Array.isArray(result.content)) {
		const text = result.content.map((block) => isRecord(block) && block.type === "text" ? stringField(block.text) : undefined).filter((item): item is string => item !== undefined).join("\n").trim();
		if (text.length > 0) return text;
	}
	return firstNonBlank(stringField(result.error), stringField(result.message));
}

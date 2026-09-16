/** Shared model-output truncation helpers. */

export const DEFAULT_MAX_LINES = 2000;
export const DEFAULT_MAX_BYTES = 50 * 1024;
const TRUNCATION_NOTICE = "[agent_team output truncated; use step_result or artifact paths for full text.]";

export function formatTruncatedModelContent(content: string): string {
	const truncation = truncateHead(content);
	return truncation.truncated ? `${closeDanglingOutputBlock(truncation.content)}\n\n${TRUNCATION_NOTICE}` : truncation.content;
}

export function truncateHead(content: string): { content: string; truncated: boolean; totalLines: number; totalBytes: number; outputLines: number; outputBytes: number; firstLineExceedsLimit: boolean } {
	const totalBytes = Buffer.byteLength(content, "utf8");
	const lines = content.split("\n");
	if (lines.length <= DEFAULT_MAX_LINES && totalBytes <= DEFAULT_MAX_BYTES) return { content, truncated: false, totalLines: lines.length, totalBytes, outputLines: lines.length, outputBytes: totalBytes, firstLineExceedsLimit: false };
	const out: string[] = [];
	let bytes = 0;
	for (const line of lines.slice(0, DEFAULT_MAX_LINES)) {
		const next = Buffer.byteLength(line, "utf8") + (out.length > 0 ? 1 : 0);
		if (bytes + next > DEFAULT_MAX_BYTES) break;
		out.push(line);
		bytes += next;
	}
	return { content: out.join("\n"), truncated: true, totalLines: lines.length, totalBytes, outputLines: out.length, outputBytes: bytes, firstLineExceedsLimit: out.length === 0 };
}

export function describeOutputLimit(): string {
	return `${DEFAULT_MAX_LINES} lines or 50KB`;
}

function closeDanglingOutputBlock(content: string): string {
	const stepId = findDanglingOutputBlock(content);
	return stepId ? `${content}\n[agent_team output end: ${stepId}]` : content;
}

function findDanglingOutputBlock(content: string): string | undefined {
	let openStepId: string | undefined;
	for (const line of content.split("\n")) {
		const begin = line.match(/^\[agent_team output begin: ([^\]]+)\]$/);
		if (begin?.[1]) {
			openStepId = begin[1];
			continue;
		}
		if (line.match(/^\[agent_team output end: ([^\]]+)\]$/)) openStepId = undefined;
	}
	return openStepId;
}

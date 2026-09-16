export function modelText(text: string): string {
	return escapeOutputBlockMarkers(text).replace(/\s+/g, " ").trim();
}

export function boundedModelText(text: string, maxChars: number): string {
	const normalized = modelText(text);
	if (normalized.length <= maxChars) return normalized;
	return `${normalized.slice(0, maxChars)}... [truncated ${normalized.length - maxChars} chars]`;
}

export function escapeOutputBlockMarkers(output: string): string {
	return output.replace(/(^|\r\n|\n|\r|\u2028|\u2029)(\[agent_team output (?:begin|end):)/g, "$1\\$2");
}

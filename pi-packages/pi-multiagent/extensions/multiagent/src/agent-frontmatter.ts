/** Frontmatter parsing helpers for reusable agent prompts. */

export const AGENT_TAG_PATTERN = /^(?!.*--)[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$/;
export const MAX_AGENT_TAGS = 32;

export interface ParsedMarkdown {
	frontmatter: Record<string, string>;
	body: string;
}

export interface ParsedAgentTags {
	tags: string[];
	invalidTags: string[];
	tooMany: boolean;
}

export function parseMarkdownFrontmatter(content: string): ParsedMarkdown {
	const normalized = content.replace(/\r\n/g, "\n");
	if (!normalized.startsWith("---\n")) return { frontmatter: {}, body: normalized };
	const end = normalized.indexOf("\n---", 4);
	if (end === -1) return { frontmatter: {}, body: normalized };
	const raw = normalized.slice(4, end);
	const body = normalized.slice(end + 5).replace(/^\n/, "");
	const frontmatter: Record<string, string> = {};
	for (const line of raw.split("\n")) {
		const index = line.indexOf(":");
		if (index <= 0) continue;
		const key = line.slice(0, index).trim();
		const value = line.slice(index + 1).trim().replace(/^[\'"]|[\'"]$/g, "");
		if (key.length > 0) frontmatter[key] = value;
	}
	return { frontmatter, body };
}

export function splitFrontmatterList(value: string | undefined): string[] | undefined {
	if (!value) return undefined;
	const entries = value
		.split(/[\s,]+/)
		.map((entry) => entry.trim())
		.filter((entry) => entry.length > 0);
	return entries.length > 0 ? entries : undefined;
}

export function parseAgentTags(value: string | undefined): ParsedAgentTags {
	const rawTags = splitFrontmatterList(value) ?? [];
	const seen = new Set<string>();
	const tags: string[] = [];
	const invalidTags: string[] = [];
	for (const rawTag of rawTags) {
		const tag = rawTag.toLowerCase();
		if (!AGENT_TAG_PATTERN.test(tag)) {
			invalidTags.push(rawTag);
			continue;
		}
		if (seen.has(tag)) continue;
		seen.add(tag);
		tags.push(tag);
	}
	return { tags: tags.slice(0, MAX_AGENT_TAGS), invalidTags, tooMany: tags.length > MAX_AGENT_TAGS };
}

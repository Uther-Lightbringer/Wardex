/** Agent library discovery for pi-multiagent. */

import { createHash } from "node:crypto";
import { existsSync, lstatSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import type {
	AgentConfig,
	AgentDiagnostic,
	AgentDiscoveryResult,
	CatalogAgentSummary,
	LibraryOptions,
	LibrarySource,
} from "./types.ts";
import { readAgentFileContent } from "./agent-file-content.ts";
import { parseAgentTags, parseMarkdownFrontmatter, splitFrontmatterList } from "./agent-frontmatter.ts";
import { DEFAULT_LIBRARY_SOURCES, LIBRARY_SOURCE_VALUES, TOOL_NAME_PATTERN } from "./types.ts";
import { validateToolNames } from "./tool-policy.ts";
import { findNearestProjectDir, getGlobalPiDir, isContainedPath, safeRealpath } from "./project-root.ts";

const AGENT_NAME_PATTERN = /^[a-z][a-z0-9-]{0,62}$/;
const TOOL_NAME_REGEX = new RegExp(TOOL_NAME_PATTERN);
const VALID_THINKING = new Set(["off", "minimal", "low", "medium", "high", "xhigh"]);
const SOURCE_PRECEDENCE: LibrarySource[] = ["package", "user", "project"];
const CATALOG_QUERY_STOP_WORDS = new Set(["a", "an", "and", "as", "for", "in", "of", "or", "the", "to", "use", "with"]);

function readAgentFile(filePath: string, source: Exclude<LibrarySource, never>, diagnostics: AgentDiagnostic[], containmentRoot: string | undefined): AgentConfig | undefined {
	const loaded = readAgentFileContent(filePath, containmentRoot ? { containmentRoot } : undefined);
	if (!loaded.ok) {
		diagnostics.push({ code: loaded.error.includes("exceeds") ? "agent-file-too-large" : "agent-read-failed", path: filePath, message: loaded.error, severity: "warning" });
		return undefined;
	}
	const content = loaded.content;
	const parsed = parseMarkdownFrontmatter(content);
	const name = parsed.frontmatter.name;
	const description = parsed.frontmatter.description;
	if (!name || !description) {
		diagnostics.push({
			code: "agent-frontmatter-invalid",
			path: filePath,
			message: "Agents require name and description.",
			severity: "warning",
		});
		return undefined;
	}
	if (!AGENT_NAME_PATTERN.test(name)) {
		diagnostics.push({ code: "agent-name-invalid", path: filePath, message: `Invalid agent name: ${name}`, severity: "warning" });
		return undefined;
	}
	if (parsed.frontmatter.extensionTools !== undefined) {
		diagnostics.push({
			code: "agent-extension-tools-denied",
			path: filePath,
			message: `Library agent ${source}:${name} cannot self-declare extensionTools; bind grants in the agent_team invocation.`,
			severity: "warning",
		});
		return undefined;
	}
	if (parsed.frontmatter.callerSkills !== undefined) {
		diagnostics.push({
			code: "agent-caller-skills-denied",
			path: filePath,
			message: `Library agent ${source}:${name} cannot self-declare caller skill grants; subagent skill propagation is controlled only by the agent_team product configuration.`,
			severity: "warning",
		});
		return undefined;
	}
	const thinking = parsed.frontmatter.thinking;
	if (thinking && !VALID_THINKING.has(thinking)) {
		diagnostics.push({
			code: "agent-thinking-invalid",
			path: filePath,
			message: `Invalid thinking level for ${name}: ${thinking}`,
			severity: "warning",
		});
		return undefined;
	}
	const tools = splitFrontmatterList(parsed.frontmatter.tools);
	const invalidTools = tools?.filter((tool) => !TOOL_NAME_REGEX.test(tool)) ?? [];
	if (invalidTools.length > 0) {
		diagnostics.push({
			code: "agent-tools-invalid",
			path: filePath,
			message: `Invalid tool names for ${name}: ${invalidTools.join(", ")}`,
			severity: "warning",
		});
		return undefined;
	}
	const parsedTags = parseAgentTags(parsed.frontmatter.tags);
	if (parsedTags.invalidTags.length > 0 || parsedTags.tooMany) {
		const reason = parsedTags.invalidTags.length > 0 ? `invalid tags: ${parsedTags.invalidTags.join(", ")}` : "too many tags; maximum is 32";
		diagnostics.push({
			code: "agent-tags-invalid",
			path: filePath,
			message: `Invalid tags for ${name}: ${reason}`,
			severity: "warning",
		});
		return undefined;
	}
	if (!validateToolNames(tools, `library agent ${source}:${name}`, diagnostics, filePath, "warning")) return undefined;
	return {
		name,
		ref: `${source}:${name}`,
		description,
		tags: parsedTags.tags,
		tools,
		model: parsed.frontmatter.model || undefined,
		thinking: thinking as AgentConfig["thinking"],
		systemPrompt: parsed.body.trim(),
		source,
		filePath,
		sha256: createHash("sha256").update(content).digest("hex"),
	};
}

function loadAgentsFromDir(dir: string, source: LibrarySource, diagnostics: AgentDiagnostic[]): AgentConfig[] {
	if (!existsSync(dir)) return [];
	const realProjectDir = source === "project" ? validateProjectAgentsDir(dir, diagnostics) : undefined;
	if (source === "project" && !realProjectDir) return [];
	let entries;
	try {
		entries = readdirSync(dir, { withFileTypes: true });
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		diagnostics.push({ code: "agent-dir-list-failed", path: dir, message, severity: "warning" });
		return [];
	}
	const agents: AgentConfig[] = [];
	for (const entry of entries) {
		if (!entry.name.endsWith(".md")) continue;
		const filePath = join(dir, entry.name);
		if (!entry.isFile() && !entry.isSymbolicLink()) continue;
		if (source === "user" && entry.isSymbolicLink()) {
			diagnostics.push({
				code: "user-agent-symlink-denied",
				path: filePath,
				message: "User agent symlinks are denied; use regular files in the configured user agent directory.",
				severity: "warning",
			});
			continue;
		}
		if (source === "project" && entry.isSymbolicLink()) {
			diagnostics.push({
				code: "project-agent-symlink-denied",
				path: filePath,
				message: "Project agent symlinks are denied; prompts must be regular files inside .pi/agents.",
				severity: "warning",
			});
			continue;
		}
		if (source === "project" && realProjectDir) {
			const realFile = safeRealpath(filePath);
			if (!realFile || !isContainedPath(realProjectDir, realFile)) {
				diagnostics.push({
					code: "project-agent-path-escape-denied",
					path: filePath,
					message: "Project agent path resolves outside the trusted .pi/agents directory.",
					severity: "warning",
				});
				continue;
			}
		}
		const agent = readAgentFile(filePath, source, diagnostics, source === "project" ? realProjectDir : undefined);
		if (agent) agents.push(agent);
	}
	return agents.sort((left, right) => left.name.localeCompare(right.name));
}

function safeLstat(path: string): ReturnType<typeof lstatSync> | undefined {
	try {
		return lstatSync(path);
	} catch {
		return undefined;
	}
}

function validateProjectAgentsDir(dir: string, diagnostics: AgentDiagnostic[]): string | undefined {
	const dirStats = safeLstat(dir);
	if (!dirStats) return undefined;
	if (dirStats.isSymbolicLink()) {
		diagnostics.push({
			code: "project-agent-dir-symlink-denied",
			path: dir,
			message: "Project .pi/agents directory symlinks are denied; prompts must live inside the repository.",
			severity: "warning",
		});
		return undefined;
	}
	const realProjectDir = safeRealpath(dir);
	const realProjectRoot = safeRealpath(dirname(dirname(dir)));
	if (!realProjectDir || !realProjectRoot || !isContainedPath(realProjectRoot, realProjectDir)) {
		diagnostics.push({
			code: "project-agent-dir-path-escape-denied",
			path: dir,
			message: "Project .pi/agents directory resolves outside the trusted project root.",
			severity: "warning",
		});
		return undefined;
	}
	return realProjectDir;
}

export function findNearestProjectAgentsDir(cwd: string, globalPiDir = getGlobalPiDir()): string | undefined {
	const projectPiDir = findNearestProjectDir(cwd, ".pi", globalPiDir);
	if (!projectPiDir) return undefined;
	const candidate = join(projectPiDir, "agents");
	const stats = safeLstat(candidate);
	return stats?.isDirectory() || stats?.isSymbolicLink() ? candidate : undefined;
}


export function getDefaultUserAgentsDir(env: NodeJS.ProcessEnv = process.env): string {
	return join(env.PI_CODING_AGENT_DIR ?? join(getGlobalPiDir(), "agent"), "agents");
}

export function normalizeLibraryOptions(input: {
	sources?: LibrarySource[];
	query?: string;
} | undefined): LibraryOptions {
	const sources = input?.sources && input.sources.length > 0 ? dedupeSources(input.sources) : DEFAULT_LIBRARY_SOURCES;
	return {
		sources,
		query: normalizeQuery(input?.query),
	};
}

export function discoverAgents(options: {
	cwd: string;
	packageAgentsDir: string;
	library: LibraryOptions;
	userAgentsDir?: string;
	projectAgentsDir?: string;
	globalPiDir?: string;
}): AgentDiscoveryResult {
	const diagnostics: AgentDiagnostic[] = [];
	const globalPiDir = options.globalPiDir ?? getGlobalPiDir();
	const userAgentsDir = options.userAgentsDir ?? getDefaultUserAgentsDir();
	const projectAgentsDir = options.projectAgentsDir ?? findNearestProjectAgentsDir(options.cwd, globalPiDir);
	const requestedSources = new Set(options.library.sources);
	const activeSources = SOURCE_PRECEDENCE.filter((source) => requestedSources.has(source));
	const byRef = new Map<string, AgentConfig>();
	for (const source of activeSources) {
		const dir = source === "package" ? options.packageAgentsDir : source === "user" ? userAgentsDir : projectAgentsDir;
		if (!dir) continue;
		for (const agent of loadAgentsFromDir(dir, source, diagnostics)) {
			if (byRef.has(agent.ref)) {
				diagnostics.push({
					code: "agent-ref-duplicate",
					path: agent.filePath,
					message: `Agent ref ${agent.ref} is already loaded; duplicate source ref denied.`,
					severity: "warning",
				});
				continue;
			}
			byRef.set(agent.ref, agent);
		}
	}
	return {
		agents: Array.from(byRef.values()).sort(compareAgents),
		diagnostics,
		packageAgentsDir: options.packageAgentsDir,
		userAgentsDir,
		projectAgentsDir,
		sources: activeSources,
	};
}

export function catalogAgents(discovery: AgentDiscoveryResult, query: string | undefined): CatalogAgentSummary[] {
	const normalizedQuery = query?.toLowerCase().trim();
	const tokens = catalogQueryTokens(normalizedQuery);
	return discovery.agents
		.map((agent) => ({ agent, score: scoreCatalogAgent(agent, normalizedQuery, tokens) }))
		.filter((entry) => !normalizedQuery || entry.score > 0)
		.sort((left, right) => right.score - left.score || compareAgents(left.agent, right.agent))
		.map(({ agent }) => ({
			name: agent.name,
			ref: agent.ref,
			source: agent.source,
			description: agent.description,
			tags: agent.tags,
			tools: agent.tools,
			model: agent.model,
			thinking: agent.thinking,
			filePath: agent.filePath,
			sha256: agent.sha256,
		}));
}

function scoreCatalogAgent(agent: AgentConfig, query: string | undefined, tokens: string[]): number {
	if (!query) return 1;
	const text = catalogAgentSearchText(agent);
	const nameAndRefTokens = searchableTokens([agent.name, refWithoutSource(agent.ref)]);
	const exactTags = new Set(agent.tags);
	const tagTokens = searchableTokens(agent.tags);
	let score = tokens.length > 1 && text.includes(query) ? 100 : 0;
	for (const token of tokens) {
		if (nameAndRefTokens.has(token)) score += 40 + token.length;
		else if (exactTags.has(token)) score += 50 + token.length;
		else if (tagTokens.has(token)) score += 25 + token.length;
		else if (textSearchTokens(agent).has(token)) score += token.length;
	}
	return score;
}

function catalogAgentSearchText(agent: AgentConfig): string {
	return searchFields(agent).join(" ").toLowerCase();
}

function textSearchTokens(agent: AgentConfig): Set<string> {
	return searchableTokens(searchFields(agent));
}

function searchFields(agent: AgentConfig): string[] {
	return [agent.name, refWithoutSource(agent.ref), agent.description, agent.tags.join(" "), agent.tools?.join(" ") ?? "", agent.model ?? "", agent.thinking ?? ""];
}

function refWithoutSource(ref: string): string {
	const separator = ref.indexOf(":");
	return separator === -1 ? ref : ref.slice(separator + 1);
}

function searchableTokens(fields: string[]): Set<string> {
	const tokens = new Set<string>();
	for (const field of fields) for (const token of field.toLowerCase().split(/[^a-z0-9-]+/)) addSearchToken(tokens, token);
	return tokens;
}

function catalogQueryTokens(query: string | undefined): string[] {
	if (!query) return [];
	const seen = new Set<string>();
	const tokens: string[] = [];
	for (const rawToken of query.split(/[^a-z0-9-]+/)) {
		for (const token of expandedSearchTokens(rawToken)) {
			if (token.length < 2 || CATALOG_QUERY_STOP_WORDS.has(token) || seen.has(token)) continue;
			seen.add(token);
			tokens.push(token);
		}
	}
	return tokens;
}

function addSearchToken(tokens: Set<string>, rawToken: string): void {
	for (const token of expandedSearchTokens(rawToken)) if (token.length >= 2 && !CATALOG_QUERY_STOP_WORDS.has(token)) tokens.add(token);
}

function expandedSearchTokens(rawToken: string): string[] {
	const token = rawToken.trim();
	if (token.length === 0) return [];
	const parts = token.split("-").filter((part) => part.length > 0);
	const expanded = parts.length > 1 ? [token, ...parts] : [token];
	const singulars = expanded.filter((part) => part.length > 3 && part.endsWith("s")).map((part) => part.slice(0, -1));
	return [...expanded, ...singulars];
}

function dedupeSources(sources: LibrarySource[]): LibrarySource[] {
	const allowed = new Set<LibrarySource>(LIBRARY_SOURCE_VALUES);
	const seen = new Set<LibrarySource>();
	const result: LibrarySource[] = [];
	for (const source of sources) {
		if (!allowed.has(source) || seen.has(source)) continue;
		seen.add(source);
		result.push(source);
	}
	return result;
}

function compareAgents(left: AgentConfig, right: AgentConfig): number {
	const name = left.name.localeCompare(right.name);
	if (name !== 0) return name;
	return SOURCE_PRECEDENCE.indexOf(left.source) - SOURCE_PRECEDENCE.indexOf(right.source);
}

function normalizeQuery(query: string | undefined): string | undefined {
	const trimmed = query?.trim();
	return trimmed && trimmed.length > 0 ? trimmed : undefined;
}

/** Product-configured caller-visible Pi skill propagation for isolated subagents. */

import { createHash } from "node:crypto";
import { lstatSync, readFileSync, realpathSync, statSync } from "node:fs";
import { isAbsolute } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type {
	AgentDiagnostic,
	ParentSkillInfo,
	ParentSkillInventory,
	ResolvedCallerSkill,
	ResolvedCallerSkillSource,
	SubagentSkillMode,
} from "./types.ts";
import { MAX_CALLER_SKILLS, SKILL_NAME_PATTERN } from "./types.ts";

const SKILL_NAME_REGEX = new RegExp(SKILL_NAME_PATTERN);
const MAX_SKILL_HASH_BYTES = 4 * 1024 * 1024;
const EMPTY_PARENT_SKILLS: ParentSkillInventory = { apiAvailable: true, readActive: false, errorMessage: undefined, skills: [] };

export interface CallerSkillResolutionContext {
	parentSkills: ParentSkillInventory | undefined;
	sourceCache: Map<string, SkillSourceReadResult>;
}

type SkillSourceReadResult = { source: ResolvedCallerSkillSource; hidden: boolean; error?: never } | { source?: never; hidden?: never; error: string };

export function getParentSkillInventory(pi: ExtensionAPI): ParentSkillInventory {
	try {
		const activeTools = new Set(pi.getActiveTools());
		if (!activeTools.has("read")) return { apiAvailable: true, readActive: false, errorMessage: undefined, skills: [] };
		const skills: ParentSkillInfo[] = pi.getCommands()
			.filter((command) => command.source === "skill" && command.name.startsWith("skill:"))
			.map((command) => ({
				name: command.name.slice("skill:".length),
				description: command.description,
				sourceInfo: {
					path: command.sourceInfo.path,
					source: command.sourceInfo.source,
					scope: command.sourceInfo.scope,
					origin: command.sourceInfo.origin,
					baseDir: command.sourceInfo.baseDir,
				},
			}))
			.filter((skill) => SKILL_NAME_REGEX.test(skill.name));
		return { apiAvailable: true, readActive: true, errorMessage: undefined, skills };
	} catch (error) {
		return { apiAvailable: false, readActive: false, errorMessage: `Could not read parent Pi skill inventory: ${error instanceof Error ? error.message : String(error)}`, skills: [] };
	}
}

export function createCallerSkillResolutionContext(parentSkills: ParentSkillInventory | undefined, _cwd: string): CallerSkillResolutionContext {
	return { parentSkills: parentSkills ?? EMPTY_PARENT_SKILLS, sourceCache: new Map() };
}

export function resolveAgentCallerSkills(input: {
	mode: SubagentSkillMode;
	tools: string[];
	label: string;
	path: string;
	diagnostics: AgentDiagnostic[];
	context: CallerSkillResolutionContext | undefined;
}): ResolvedCallerSkill[] | undefined {
	if (input.mode === "disabled") return [];
	if (!input.tools.includes("read")) {
		input.diagnostics.push({ code: "subagent-skills-read-required", message: `${input.label} would receive caller Pi skills, but skill files can be loaded only when the child filesystem read/discovery suite is granted. Enable graph.authority.allowFilesystemRead or set ${"--agent-team-subagent-skills disabled"}.`, path: input.path, severity: "error" });
		return undefined;
	}
	const parentSkills = input.context?.parentSkills;
	if (parentSkills === undefined || !parentSkills.apiAvailable) {
		input.diagnostics.push({ code: "subagent-skills-inventory-unavailable", message: parentSkills?.errorMessage ?? `Cannot resolve caller Pi skills for ${input.label}: parent Pi skill inventory is unavailable.`, path: input.path, severity: "error" });
		return undefined;
	}
	if (!parentSkills.readActive) {
		input.diagnostics.push({ code: "subagent-skills-parent-read-inactive", message: `${input.label} would receive caller Pi skills, but the parent read tool is not active so caller skill files cannot be loaded all-or-nothing. Enable the parent read tool or launch Pi with --agent-team-subagent-skills disabled.`, path: input.path, severity: "error" });
		return undefined;
	}
	return resolveVisibleCallerSkills(input, parentSkills);
}

export function verifyResolvedCallerSkillSources(skills: ResolvedCallerSkill[]): string | undefined {
	const checked = new Set<string>();
	for (const skill of skills) {
		if (checked.has(skill.source.realpath)) continue;
		checked.add(skill.source.realpath);
		const current = readCallerSkillSource(skill);
		if ("error" in current) return `Caller skill source changed before launch for ${skill.name}: ${current.error}`;
		if (!sameCallerSkillSourceState(skill.source, current.source)) return `Caller skill source changed before launch for ${skill.name}; refusing to load stale skill instructions.`;
	}
	return undefined;
}

function resolveVisibleCallerSkills(input: {
	label: string;
	path: string;
	diagnostics: AgentDiagnostic[];
	context: CallerSkillResolutionContext | undefined;
}, parentSkills: ParentSkillInventory): ResolvedCallerSkill[] | undefined {
	const parentByName = parentSkillMap(parentSkills, input);
	if (!parentByName) return undefined;
	const resolved: ResolvedCallerSkill[] = [];
	for (const skill of parentSkills.skills) {
		const source = readCallerSkillSourceCached(skill, input.context);
		if ("error" in source) {
			input.diagnostics.push({ code: "caller-skill-source-unavailable", message: `Cannot load caller skill ${skill.name}: ${source.error}`, path: skill.sourceInfo.path, severity: "error" });
			return undefined;
		}
		if (source.hidden) continue;
		resolved.push({ name: skill.name, description: skill.description, source: source.source });
	}
	if (resolved.length > MAX_CALLER_SKILLS) {
		input.diagnostics.push({ code: "subagent-skills-too-many", message: `${input.label} would receive ${resolved.length} caller Pi skills; maximum is ${MAX_CALLER_SKILLS}. Disable subagent skill propagation or reduce the caller-visible skill set.`, path: input.path, severity: "error" });
		return undefined;
	}
	return resolved;
}

function parentSkillMap(parentSkills: ParentSkillInventory, input: { path: string; diagnostics: AgentDiagnostic[] }): Map<string, ParentSkillInfo> | undefined {
	const duplicate = firstDuplicate(parentSkills.skills.map((skill) => skill.name));
	if (duplicate) {
		input.diagnostics.push({ code: "caller-skills-ambiguous", message: `Multiple visible parent Pi skills are named ${duplicate}; reload Pi or resolve the skill-name collision before delegation.`, path: input.path, severity: "error" });
		return undefined;
	}
	return new Map(parentSkills.skills.map((skill) => [skill.name, skill]));
}

function readCallerSkillSourceCached(skill: ParentSkillInfo, context: CallerSkillResolutionContext | undefined): SkillSourceReadResult {
	const key = `${skill.name}\u0000${skill.sourceInfo.path}`;
	const cached = context?.sourceCache.get(key);
	if (cached) return cached;
	const current = readCallerSkillSource(skill);
	context?.sourceCache.set(key, current);
	return current;
}

function readCallerSkillSource(skill: ParentSkillInfo | ResolvedCallerSkill): SkillSourceReadResult {
	const sourceInfo = "sourceInfo" in skill ? skill.sourceInfo : skill.source;
	if (!isAbsolute(sourceInfo.path)) return { error: "skill source path is not absolute" };
	if (!sourceInfo.path.endsWith(".md")) return { error: "skill source path is not a markdown file" };
	try {
		const lexical = lstatSync(sourceInfo.path);
		if (!lexical.isFile() && !lexical.isSymbolicLink()) return { error: "skill source path is not a regular file" };
		const realpath = realpathSync(sourceInfo.path);
		if (!realpath.endsWith(".md")) return { error: "skill source realpath is not a markdown file" };
		const stats = statSync(realpath);
		if (!stats.isFile()) return { error: "skill source realpath is not a regular file" };
		if (stats.size > MAX_SKILL_HASH_BYTES) return { error: `skill source exceeds ${MAX_SKILL_HASH_BYTES} byte fingerprint limit` };
		const content = readFileSync(realpath);
		return {
			hidden: parseDisableModelInvocation(content.toString("utf8")),
			source: {
				path: sourceInfo.path,
				realpath,
				source: sourceInfo.source,
				scope: sourceInfo.scope,
				origin: sourceInfo.origin,
				baseDir: sourceInfo.baseDir,
				dev: stats.dev,
				ino: stats.ino,
				size: stats.size,
				mtimeMs: stats.mtimeMs,
				sha256: createHash("sha256").update(content).digest("hex"),
			},
		};
	} catch (error) {
		return { error: error instanceof Error ? error.message : String(error) };
	}
}

function sameCallerSkillSourceState(left: ResolvedCallerSkillSource, right: ResolvedCallerSkillSource): boolean {
	return left.realpath === right.realpath && left.dev === right.dev && left.ino === right.ino && left.size === right.size && left.mtimeMs === right.mtimeMs && left.sha256 === right.sha256;
}

function parseDisableModelInvocation(content: string): boolean {
	const normalized = content.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
	if (!normalized.startsWith("---")) return false;
	const end = normalized.indexOf("\n---", 3);
	if (end === -1) return false;
	const frontmatter = normalized.slice(4, end);
	for (const line of frontmatter.split("\n")) {
		const index = line.indexOf(":");
		if (index <= 0) continue;
		const key = line.slice(0, index).trim();
		if (key !== "disable-model-invocation") continue;
		return yamlBooleanTrue(line.slice(index + 1));
	}
	return false;
}

function yamlBooleanTrue(value: string): boolean {
	return stripYamlScalarComment(value).trim().replace(/^[\'\"]|[\'\"]$/g, "").toLowerCase() === "true";
}

function stripYamlScalarComment(value: string): string {
	let quote = "";
	for (let index = 0; index < value.length; index += 1) {
		const char = value[index];
		if ((char === "'" || char === '"') && quote === "") quote = char;
		else if (char === quote) quote = "";
		else if (char === "#" && quote === "" && (index === 0 || /\s/.test(value[index - 1] ?? ""))) return value.slice(0, index);
	}
	return value;
}

function firstDuplicate(values: string[]): string | undefined {
	const seen = new Set<string>();
	for (const value of values) {
		if (seen.has(value)) return value;
		seen.add(value);
	}
	return undefined;
}

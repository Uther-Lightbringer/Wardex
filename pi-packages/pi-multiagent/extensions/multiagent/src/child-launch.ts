/** Child Pi RPC launch argument construction for detached subagents. */

import { type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { basename, delimiter, isAbsolute, join, resolve } from "node:path";
import type { AgentInvocationDefaults, ResolvedAgent } from "./types.ts";
import { READONLY_CHILD_TOOL_NAMES } from "./types.ts";
import { effectiveAgentInvocation } from "./agent-invocation.ts";
import { childToolNames } from "./tool-policy.ts";
import { findNearestWorkspaceRoot, isContainedPath, safeRealpath } from "./project-root.ts";

export interface SpawnOptions {
	cwd: string;
	shell: false;
	stdio: ["pipe", "pipe", "pipe"];
	detached: boolean;
}

export type SpawnProcess = (command: string, args: string[], options: SpawnOptions) => ChildProcessWithoutNullStreams;

export interface ChildLaunchOptions {
	sessionDir?: string;
	sessionName?: string;
}

export function buildPiArgs(agent: ResolvedAgent, defaults: AgentInvocationDefaults, promptPath: string, launch: ChildLaunchOptions = {}): string[] {
	const args = [
		"--mode",
		"rpc",
		...sessionArgs(launch),
		...extensionArgs(agent),
		"--no-context-files",
		"--no-skills",
		...skillArgs(agent),
		"--no-prompt-templates",
		"--no-themes",
		"--system-prompt",
		"",
		"--append-system-prompt",
		promptPath,
	];
	const { model, thinking } = effectiveAgentInvocation(agent, defaults);
	if (model) args.push("--model", model);
	if (thinking) args.push("--thinking", thinking);
	if (!hasMandatoryReadSuite(agent.tools)) throw new Error(`Resolved agent ${agent.ref} is missing mandatory read/discovery tools.`);
	const tools = childToolNames(agent);
	args.push("--tools", tools.join(","));
	return args;
}

function hasMandatoryReadSuite(tools: string[]): boolean {
	return READONLY_CHILD_TOOL_NAMES.every((tool) => tools.includes(tool));
}

function sessionArgs(launch: ChildLaunchOptions): string[] {
	const args: string[] = [];
	if (launch.sessionName) args.push("--name", launch.sessionName);
	if (launch.sessionDir) args.push("--session-dir", launch.sessionDir);
	return args;
}

function extensionArgs(agent: ResolvedAgent): string[] {
	const args: string[] = [];
	const seen = new Set<string>();
	for (const grant of agent.extensionTools) {
		if (seen.has(grant.source.realpath)) continue;
		seen.add(grant.source.realpath);
		args.push("--extension", grant.source.realpath);
	}
	return args;
}

function skillArgs(agent: ResolvedAgent): string[] {
	const args: string[] = [];
	const seen = new Set<string>();
	for (const skill of agent.callerSkills) {
		if (seen.has(skill.source.realpath)) continue;
		seen.add(skill.source.realpath);
		args.push("--skill", skill.source.realpath);
	}
	return args;
}

export function getPiInvocation(args: string[], cwd: string): { command: string; args: string[] } {
	const deniedRoots = findDeniedRoots(cwd);
	const currentScript = process.argv[1];
	const isBunVirtualScript = currentScript?.startsWith("/$bunfs/root/") ?? false;
	const currentScriptPath = currentScript && !isBunVirtualScript ? resolve(currentScript) : undefined;
	if (currentScriptPath && existsSync(currentScriptPath) && isExecutableFile(process.execPath) && isTrustedLaunchPath(deniedRoots, process.execPath) && isTrustedLaunchPath(deniedRoots, currentScriptPath)) return { command: process.execPath, args: [currentScriptPath, ...args] };
	const execName = basename(process.execPath).toLowerCase();
	if (!/^(node|bun)(\.exe)?$/.test(execName) && isExecutableFile(process.execPath) && isTrustedLaunchPath(deniedRoots, process.execPath)) return { command: process.execPath, args };
	const command = resolvePiCommandFromPath(process.env.PATH ?? "", cwd);
	if (!command) throw new Error("Unable to resolve a trusted absolute pi launcher from PATH; refusing to spawn bare pi.");
	return { command, args };
}

export function resolvePiCommandFromPath(pathEnv: string, cwd: string): string | undefined {
	const deniedRoots = findDeniedRoots(cwd);
	for (const entry of pathEnv.split(delimiter)) {
		if (!entry || !isAbsolute(entry)) continue;
		const candidate = resolve(entry, process.platform === "win32" ? "pi.cmd" : "pi");
		if (isExecutableFile(candidate) && !isContainedInAnyRoot(deniedRoots, candidate)) return candidate;
		if (process.platform === "win32") {
			const psCandidate = resolve(entry, "pi.ps1");
			if (isExecutableFile(psCandidate) && !isContainedInAnyRoot(deniedRoots, psCandidate)) return psCandidate;
		}
	}
	return undefined;
}

export function killProcessTree(child: ChildProcessWithoutNullStreams, signal: NodeJS.Signals): boolean {
	if (process.platform !== "win32" && child.pid !== undefined) {
		try {
			process.kill(-child.pid, signal);
			return true;
		} catch {
			return child.kill(signal);
		}
	}
	return child.kill(signal);
}

function findDeniedRoots(cwd: string): string[] {
	const roots = new Set<string>();
	addProjectRoot(roots, resolve(cwd));
	const realCwd = safeRealpath(resolve(cwd));
	if (realCwd) addProjectRoot(roots, realCwd);
	return [...roots];
}

function addProjectRoot(roots: Set<string>, cwd: string): void {
	roots.add(findNearestWorkspaceRoot(cwd));
}

function isTrustedLaunchPath(deniedRoots: string[], path: string): boolean {
	return !isContainedInAnyRoot(deniedRoots, path);
}

function isContainedInAnyRoot(roots: string[], child: string): boolean {
	const lexicalChild = resolve(child);
	if (roots.some((root) => isContainedPath(root, lexicalChild))) return true;
	const realChild = safeRealpath(lexicalChild);
	return realChild ? roots.some((root) => isContainedPath(root, realChild)) : false;
}

function isExecutableFile(path: string): boolean {
	try {
		const stats = statSync(path);
		return stats.isFile() && (process.platform === "win32" || (stats.mode & 0o111) !== 0);
	} catch {
		return false;
	}
}

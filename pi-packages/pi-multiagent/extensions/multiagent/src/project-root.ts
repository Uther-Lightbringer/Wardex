/** Shared project/workspace root detection with global ~/.pi exclusion. */

import { lstatSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";

export function getGlobalPiDir(): string {
	return join(homedir(), ".pi");
}

export function findNearestWorkspaceRoot(cwd: string, globalPiDir = getGlobalPiDir()): string {
	return findNearestProjectRoot(cwd, globalPiDir) ?? resolve(cwd);
}

export function findNearestProjectRoot(cwd: string, globalPiDir = getGlobalPiDir()): string | undefined {
	let current = resolve(cwd);
	while (true) {
		if (pathExists(join(current, ".git"))) return current;
		const piMarker = join(current, ".pi");
		if (pathExists(piMarker) && !isSamePath(piMarker, globalPiDir)) return current;
		const parent = dirname(current);
		if (parent === current) return undefined;
		current = parent;
	}
}

export function findNearestProjectMarker(cwd: string, name: string, ignoredPath?: string): string | undefined {
	let current = resolve(cwd);
	while (true) {
		const candidate = join(current, name);
		if (pathExists(candidate) && !isSamePath(candidate, ignoredPath)) return candidate;
		const parent = dirname(current);
		if (parent === current) return undefined;
		current = parent;
	}
}

export function findNearestProjectDir(cwd: string, name: string, ignoredPath?: string): string | undefined {
	let current = resolve(cwd);
	while (true) {
		const candidate = join(current, name);
		if (isDirectory(candidate) && !isSamePath(candidate, ignoredPath)) return candidate;
		const parent = dirname(current);
		if (parent === current) return undefined;
		current = parent;
	}
}

export function pathExists(path: string): boolean {
	try {
		lstatSync(path);
		return true;
	} catch {
		return false;
	}
}

export function isDirectory(path: string): boolean {
	try {
		return lstatSync(path).isDirectory();
	} catch {
		return false;
	}
}

export function safeRealpath(path: string): string | undefined {
	try {
		return resolve(realpathSync(path));
	} catch {
		return undefined;
	}
}

export function isContainedPath(parent: string, child: string): boolean {
	const normalizedParent = resolve(parent);
	const normalizedChild = resolve(child);
	const prefix = normalizedParent.endsWith(sep) ? normalizedParent : `${normalizedParent}${sep}`;
	return normalizedChild === normalizedParent || normalizedChild.startsWith(prefix);
}

export function isSamePath(left: string | undefined, right: string | undefined): boolean {
	if (!left || !right) return false;
	const normalizedLeft = resolve(left);
	const normalizedRight = resolve(right);
	if (normalizedLeft === normalizedRight) return true;
	const realLeft = safeRealpath(normalizedLeft);
	const realRight = safeRealpath(normalizedRight);
	return realLeft !== undefined && realRight !== undefined && realLeft === realRight;
}

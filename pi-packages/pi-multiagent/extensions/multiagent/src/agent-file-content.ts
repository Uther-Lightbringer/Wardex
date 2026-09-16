/** Bounded agent markdown loading for package/user/project libraries. */

import { closeSync, constants, fstatSync, lstatSync, openSync, readSync } from "node:fs";
import { resolve } from "node:path";
import { isContainedPath, safeRealpath } from "./project-root.ts";
import { MAX_AGENT_FILE_BYTES } from "./types.ts";

const OPEN_NOFOLLOW = constants.O_NOFOLLOW === undefined ? constants.O_RDONLY : constants.O_RDONLY | constants.O_NOFOLLOW;

export interface AgentFileReadOptions {
	containmentRoot?: string;
}

export type AgentFileReadResult = { ok: true; content: string } | { ok: false; error: string };

export function readAgentFileContent(filePath: string, options: AgentFileReadOptions = {}): AgentFileReadResult {
	let fd: number;
	try {
		fd = openSync(filePath, OPEN_NOFOLLOW);
	} catch (error) {
		return { ok: false, error: errorMessage(error) };
	}
	try {
		const stats = fstatSync(fd);
		const validationError = validateOpenedPath(filePath, stats, options);
		if (validationError) return { ok: false, error: validationError };
		if (stats.size > MAX_AGENT_FILE_BYTES) return { ok: false, error: `agent file exceeds ${MAX_AGENT_FILE_BYTES} bytes` };
		return readBounded(fd);
	} catch (error) {
		return { ok: false, error: errorMessage(error) };
	} finally {
		closeSync(fd);
	}
}

function validateOpenedPath(filePath: string, stats: ReturnType<typeof fstatSync>, options: AgentFileReadOptions): string | undefined {
	if (!stats.isFile()) return "agent file must be a regular file";
	const lexical = lstatSync(filePath);
	if (lexical.isSymbolicLink()) return "agent file changed during open: symlink denied";
	if (!lexical.isFile()) return "agent file changed during open: not a regular file";
	if (lexical.dev !== stats.dev || lexical.ino !== stats.ino) return "agent file changed during open: identity mismatch";
	if (!options.containmentRoot) return undefined;
	const root = safeRealpath(options.containmentRoot) ?? resolve(options.containmentRoot);
	const openedPath = safeRealpath(filePath);
	if (!openedPath || !isContainedPath(root, openedPath)) return "agent file changed during open: path escape denied";
	return undefined;
}

function readBounded(fd: number): AgentFileReadResult {
	const chunks: Buffer[] = [];
	const buffer = Buffer.alloc(Math.min(64 * 1024, MAX_AGENT_FILE_BYTES + 1));
	let bytes = 0;
	while (true) {
		const remaining = MAX_AGENT_FILE_BYTES + 1 - bytes;
		const read = readSync(fd, buffer, 0, Math.min(buffer.length, remaining), null);
		if (read === 0) return { ok: true, content: Buffer.concat(chunks, bytes).toString("utf8") };
		if (bytes + read > MAX_AGENT_FILE_BYTES) return { ok: false, error: `agent file exceeds ${MAX_AGENT_FILE_BYTES} bytes` };
		chunks.push(Buffer.from(buffer.subarray(0, read)));
		bytes += read;
	}
}

function errorMessage(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

/** Pure detached graph-file ingress for agent_team start calls. */

import { closeSync, constants, fstatSync, lstatSync, openSync, readSync, realpathSync } from "node:fs";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { Compile } from "typebox/compile";
import { AgentTeamSchema, type GraphSpec } from "./schemas.ts";
import type { AgentDiagnostic } from "./types.ts";
import { MAX_GRAPH_FILE_BYTES } from "./types.ts";

const GRAPH_FILE_EXTENSION = ".json";
const validateGraph = Compile(AgentTeamSchema.properties.graph);

export interface MaterializedGraphInput {
	input: unknown;
	diagnostics: AgentDiagnostic[];
}

interface OpenGraphFile {
	fd: number;
	path: string;
}

export function materializeAgentTeamInput(input: unknown, cwd: string): MaterializedGraphInput {
	if (!isRecord(input) || input.graphFile === undefined) return { input, diagnostics: [] };
	const diagnostics = validateGraphFileWrapper(input);
	if (diagnostics.some((item) => item.severity === "error")) return { input, diagnostics };
	const graphFile = input.graphFile;
	if (typeof graphFile !== "string") return { input, diagnostics: [...diagnostics, makeDiagnostic("graph-file-required", "graphFile must be a non-empty relative JSON path.", "/graphFile")] };
	const opened = openGraphFile(graphFile, cwd);
	if (opened.diagnostic) return { input, diagnostics: [...diagnostics, opened.diagnostic] };
	if (!opened.file) return { input, diagnostics };
	try {
		const loaded = readGraphFile(opened.file);
		if (loaded.diagnostics.length > 0) return { input, diagnostics: [...diagnostics, ...loaded.diagnostics] };
		const { graphFile: _graphFile, ...rest } = input;
		return { input: { ...rest, graph: loaded.graph }, diagnostics };
	} finally {
		closeSync(opened.file.fd);
	}
}

function validateGraphFileWrapper(input: Record<string, unknown>): AgentDiagnostic[] {
	const diagnostics: AgentDiagnostic[] = [];
	if (input.action !== "start") diagnostics.push(makeDiagnostic("graph-file-start-only", "graphFile is valid only with action:\"start\".", "/graphFile"));
	if (typeof input.graphFile !== "string" || !input.graphFile.trim()) diagnostics.push(makeDiagnostic("graph-file-required", "graphFile must be a non-empty relative JSON path.", "/graphFile"));
	if (input.graph !== undefined) diagnostics.push(makeDiagnostic("graph-file-inline-graph-denied", "Use exactly one of graph or graphFile.", "/"));
	return diagnostics;
}

function openGraphFile(path: string, cwd: string): { file?: OpenGraphFile; diagnostic?: AgentDiagnostic } {
	if (path.includes("\0")) return { diagnostic: makeDiagnostic("graph-file-path-invalid", "graphFile path contains a NUL byte.", "/graphFile") };
	if (isAbsolute(path)) return { diagnostic: makeDiagnostic("graph-file-absolute-denied", "graphFile must be relative to the current working directory.", "/graphFile") };
	if (!path.endsWith(GRAPH_FILE_EXTENSION)) return { diagnostic: makeDiagnostic("graph-file-extension-invalid", "graphFile must point to a .json file.", "/graphFile") };
	let realCwd: string;
	let realPath: string;
	try {
		realCwd = realpathSync(cwd);
	} catch (error) {
		return { diagnostic: makeDiagnostic("graph-file-cwd-invalid", `Could not resolve cwd for graphFile: ${errorMessage(error)}`, "/graphFile") };
	}
	const lexicalPath = resolve(realCwd, path);
	if (!isContainedPath(realCwd, lexicalPath)) return { diagnostic: makeDiagnostic("graph-file-path-escape-denied", "graphFile must resolve inside the current working directory.", "/graphFile") };
	const symlinkDiagnostic = findSymlinkPathDiagnostic(realCwd, lexicalPath);
	if (symlinkDiagnostic) return { diagnostic: symlinkDiagnostic };
	try {
		const stats = lstatSync(lexicalPath);
		if (stats.isSymbolicLink()) return { diagnostic: makeDiagnostic("graph-file-symlink-denied", "graphFile symlinks are denied; use a regular JSON file inside the current workspace.", "/graphFile") };
		if (!stats.isFile()) return { diagnostic: makeDiagnostic("graph-file-not-file", "graphFile must point to a regular JSON file.", "/graphFile") };
		realPath = realpathSync(lexicalPath);
	} catch (error) {
		return { diagnostic: makeDiagnostic("graph-file-unreadable", `Could not access graphFile: ${errorMessage(error)}`, "/graphFile") };
	}
	if (!isContainedPath(realCwd, realPath)) return { diagnostic: makeDiagnostic("graph-file-path-escape-denied", "graphFile must resolve inside the current working directory.", "/graphFile") };
	let fd: number;
	try {
		fd = openSync(realPath, constants.O_RDONLY | constants.O_NOFOLLOW);
	} catch (error) {
		return { diagnostic: makeDiagnostic("graph-file-open-failed", `Could not open graphFile safely: ${errorMessage(error)}`, "/graphFile") };
	}
	try {
		const stats = fstatSync(fd);
		if (!stats.isFile()) return closeWithDiagnostic(fd, "graph-file-not-file", "graphFile must point to a regular JSON file.");
		if (stats.size > MAX_GRAPH_FILE_BYTES) return closeWithDiagnostic(fd, "graph-file-too-large", `graphFile exceeds ${MAX_GRAPH_FILE_BYTES} bytes.`);
	} catch (error) {
		return closeWithDiagnostic(fd, "graph-file-stat-failed", `Could not inspect graphFile: ${errorMessage(error)}`);
	}
	return { file: { fd, path: realPath } };
}

function closeWithDiagnostic(fd: number, code: string, message: string): { diagnostic: AgentDiagnostic } {
	closeSync(fd);
	return { diagnostic: makeDiagnostic(code, message, "/graphFile") };
}

function findSymlinkPathDiagnostic(realCwd: string, lexicalPath: string): AgentDiagnostic | undefined {
	const relativePath = relative(realCwd, lexicalPath);
	let current = realCwd;
	for (const segment of relativePath.split(sep)) {
		if (!segment) continue;
		current = join(current, segment);
		try {
			if (lstatSync(current).isSymbolicLink()) return makeDiagnostic("graph-file-symlink-denied", "graphFile symlinks are denied; use a regular JSON file inside the current workspace.", "/graphFile");
		} catch (error) {
			return makeDiagnostic("graph-file-unreadable", `Could not access graphFile: ${errorMessage(error)}`, "/graphFile");
		}
	}
	return undefined;
}

function readBoundedGraphFile(file: OpenGraphFile): { text: string; diagnostic?: never } | { text?: never; diagnostic: AgentDiagnostic } {
	const chunks: Buffer[] = [];
	const buffer = Buffer.alloc(Math.min(64 * 1024, MAX_GRAPH_FILE_BYTES + 1));
	let bytes = 0;
	try {
		while (true) {
			const remaining = MAX_GRAPH_FILE_BYTES + 1 - bytes;
			const read = readSync(file.fd, buffer, 0, Math.min(buffer.length, remaining), null);
			if (read === 0) return { text: Buffer.concat(chunks, bytes).toString("utf8") };
			if (bytes + read > MAX_GRAPH_FILE_BYTES) return { diagnostic: makeDiagnostic("graph-file-too-large", `graphFile exceeds ${MAX_GRAPH_FILE_BYTES} bytes.`, "/graphFile") };
			chunks.push(Buffer.from(buffer.subarray(0, read)));
			bytes += read;
		}
	} catch (error) {
		return { diagnostic: makeDiagnostic("graph-file-read-failed", `Could not read graphFile: ${errorMessage(error)}`, "/graphFile") };
	}
}

function readGraphFile(file: OpenGraphFile): { graph?: GraphSpec; diagnostics: AgentDiagnostic[] } {
	const loaded = readBoundedGraphFile(file);
	if (loaded.diagnostic) return { diagnostics: [loaded.diagnostic] };
	let parsed: unknown;
	try {
		parsed = JSON.parse(loaded.text);
	} catch (error) {
		return { diagnostics: [makeDiagnostic("graph-file-json-invalid", `Could not parse graphFile JSON: ${errorMessage(error)}`, "/graphFile")] };
	}
	if (!isRecord(parsed)) return { diagnostics: [makeDiagnostic("graph-file-object-required", "graphFile JSON must be an object.", "/graphFile")] };
	const forbidden = ["action", "runId", "graphFile", "run_status", "step_result", "message", "cancel", "cleanup"].filter((key) => parsed[key] !== undefined);
	if (forbidden.length > 0) return { diagnostics: [makeDiagnostic("graph-file-control-fields-denied", `graphFile must be a pure graph; remove control fields: ${forbidden.join(", ")}.`, "/graphFile")] };
	if (!validateGraph.Check(parsed)) {
		return { diagnostics: [...validateGraph.Errors(parsed)].map((error) => makeDiagnostic("graph-file-schema-invalid", `graphFile schema violation: ${error.message}`, `/graphFile${error.instancePath}`)) };
	}
	return { graph: parsed as GraphSpec, diagnostics: [] };
}

function makeDiagnostic(code: string, message: string, path: string): AgentDiagnostic {
	return { code, message, path, severity: "error" };
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null;
}

function isContainedPath(parent: string, child: string): boolean {
	const normalizedParent = resolve(parent);
	const normalizedChild = resolve(child);
	return normalizedChild === normalizedParent || normalizedChild.startsWith(`${normalizedParent}${sep}`);
}

function errorMessage(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

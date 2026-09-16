/** Manifest-backed artifact ownership for detached runs. */

import { createHash } from "node:crypto";
import { chmodSync, closeSync, constants, lstatSync, mkdtempSync, openSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve, sep } from "node:path";

export interface ArtifactRecord {
	path: string;
	purpose: string;
	size: number;
	sha256: string;
}

interface RunRootIdentity {
	realpath: string;
	dev: number;
	ino: number;
}

export interface RunArtifactStore {
	runDir: string;
	manifestPath: string;
	records: ArtifactRecord[];
	root: RunRootIdentity;
}

let manifestWriteCounter = 0;

export function createRunArtifactStore(): RunArtifactStore {
	const runDir = realpathSync(mkdtempSync(join(tmpdir(), "pi-multiagent-run-")));
	chmodSync(runDir, 0o700);
	const store = { runDir, manifestPath: join(runDir, "manifest.json"), records: [], root: readRunRootIdentity(runDir) };
	writeManifest(store);
	return store;
}

export function writeRunArtifact(store: RunArtifactStore, name: string, purpose: string, content: string): ArtifactRecord {
	assertRunRootUnchanged(store);
	const path = artifactPathForName(store.runDir, name);
	writeExclusiveFile(path, content);
	const record = makeRecord(path, purpose, store);
	store.records.push(record);
	writeManifest(store);
	return record;
}

export function recordRunArtifact(store: RunArtifactStore, path: string, purpose: string): ArtifactRecord {
	assertRunRootUnchanged(store);
	const record = makeRecord(path, purpose, store);
	store.records.push(record);
	writeManifest(store);
	return record;
}

export function cleanupRunArtifacts(store: RunArtifactStore): string[] {
	if (store.records.length === 0 && !pathExists(store.runDir)) return [];
	assertRunRootUnchanged(store);
	const deleted: string[] = [];
	for (const record of [...store.records].reverse()) if (deleteOwnedPath(store, record.path, false)) deleted.push(record.path);
	if (deleteOwnedPath(store, store.manifestPath, false)) deleted.push(store.manifestPath);
	if (deleteOwnedPath(store, store.runDir, true)) deleted.push(store.runDir);
	store.records = [];
	return deleted;
}

function artifactPathForName(runDir: string, name: string): string {
	if (name.includes("\0") || name !== basename(name) || name === "." || name === "..") throw new Error(`Artifact name must be a basename: ${name}`);
	const path = resolve(runDir, name);
	if (!isContainedPath(runDir, path)) throw new Error(`Artifact path escaped run directory: ${name}`);
	return path;
}

function writeManifest(store: RunArtifactStore): void {
	assertRunRootUnchanged(store);
	const content = JSON.stringify({ runDir: store.runDir, records: store.records }, null, 2);
	manifestWriteCounter += 1;
	const tempPath = artifactPathForName(store.runDir, `.manifest-${process.pid}-${Date.now()}-${manifestWriteCounter}.tmp`);
	writeExclusiveFile(tempPath, content);
	assertRunRootUnchanged(store);
	renameSync(tempPath, store.manifestPath);
	assertRunRootUnchanged(store);
}

function writeExclusiveFile(path: string, content: string): void {
	let fd: number | undefined;
	try {
		fd = openSync(path, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | noFollowFlag(), 0o600);
		writeFileSync(fd, content, { encoding: "utf8" });
	} finally {
		if (fd !== undefined) closeSync(fd);
	}
}

function noFollowFlag(): number {
	return typeof constants.O_NOFOLLOW === "number" ? constants.O_NOFOLLOW : 0;
}

function makeRecord(path: string, purpose: string, store: RunArtifactStore): ArtifactRecord {
	assertRunRootUnchanged(store);
	const lexicalPath = resolve(path);
	if (!isContainedPath(store.runDir, lexicalPath)) throw new Error(`Artifact path escaped run directory: ${path}`);
	const lexicalStats = lstatSync(lexicalPath);
	if (lexicalStats.isSymbolicLink()) throw new Error(`Artifact symlinks are denied: ${path}`);
	if (!lexicalStats.isFile()) throw new Error(`Artifact is not a regular file: ${path}`);
	const realpath = realpathSync(lexicalPath);
	if (!isContainedPath(store.root.realpath, realpath)) throw new Error(`Artifact realpath escaped run directory: ${path}`);
	const content = readFileSync(lexicalPath);
	return { path: realpath, purpose, size: lexicalStats.size, sha256: createHash("sha256").update(content).digest("hex") };
}

function pathExists(path: string): boolean {
	try {
		lstatSync(path);
		return true;
	} catch {
		return false;
	}
}

function deleteOwnedPath(store: RunArtifactStore, path: string, allowDirectory: boolean): boolean {
	assertRunRootUnchanged(store);
	const lexicalPath = resolve(path);
	if (!isContainedPath(store.runDir, lexicalPath)) return false;
	let stats: ReturnType<typeof lstatSync>;
	try {
		stats = lstatSync(lexicalPath);
	} catch {
		return false;
	}
	if (stats.isDirectory() && !allowDirectory) return false;
	const deletesRunRoot = lexicalPath === store.runDir;
	rmSync(lexicalPath, { recursive: allowDirectory, force: true });
	if (!deletesRunRoot) assertRunRootUnchanged(store);
	return true;
}

function readRunRootIdentity(runDir: string): RunRootIdentity {
	const stats = lstatSync(runDir);
	if (stats.isSymbolicLink()) throw new Error(`Artifact run directory symlink is denied: ${runDir}`);
	if (!stats.isDirectory()) throw new Error(`Artifact run directory is not a directory: ${runDir}`);
	return { realpath: realpathSync(runDir), dev: stats.dev, ino: stats.ino };
}

function assertRunRootUnchanged(store: RunArtifactStore): void {
	const current = readRunRootIdentity(store.runDir);
	if (current.realpath !== store.root.realpath || current.dev !== store.root.dev || current.ino !== store.root.ino) throw new Error(`Artifact run directory changed before write or cleanup: ${store.runDir}`);
}

function isContainedPath(parent: string, child: string): boolean {
	const normalizedParent = resolve(parent);
	const normalizedChild = resolve(child);
	const prefix = normalizedParent.endsWith(sep) ? normalizedParent : `${normalizedParent}${sep}`;
	return normalizedChild === normalizedParent || normalizedChild.startsWith(prefix);
}

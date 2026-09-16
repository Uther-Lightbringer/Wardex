import { lstatSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";

export function findProjectSettingsFile(cwd: string, globalPiDir = join(homedir(), ".pi")): string | undefined {
	let current = cwd;
	const ignoredSettings = resolve(globalPiDir, "settings.json");
	while (true) {
		const candidate = join(current, ".pi", "settings.json");
		try {
			lstatSync(candidate);
			if (resolve(candidate) !== ignoredSettings) return candidate;
		} catch {
			// Missing settings at this level; keep walking ancestors.
		}
		const parent = dirname(current);
		if (parent === current) return undefined;
		current = parent;
	}
}

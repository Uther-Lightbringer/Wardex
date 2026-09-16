/** Launch-time cwd identity revalidation for child process starts. */

import { lstatSync, realpathSync, statSync } from "node:fs";
import type { TeamStepSpec } from "./types.ts";

export function validateLaunchCwd(step: TeamStepSpec): string | undefined {
	try {
		const lstat = lstatSync(step.cwd);
		if (lstat.isSymbolicLink()) return "Step cwd changed before launch: symlink denied.";
		const stats = statSync(step.cwd);
		if (!stats.isDirectory()) return "Step cwd changed before launch: not a directory.";
		const realpath = realpathSync(step.cwd);
		if (realpath !== step.cwdIdentity.realpath) return "Step cwd changed before launch: realpath mismatch.";
		if (stats.dev !== step.cwdIdentity.dev || stats.ino !== step.cwdIdentity.ino) return "Step cwd changed before launch: directory identity mismatch.";
		return undefined;
	} catch (error) {
		return `Step cwd changed before launch: ${error instanceof Error ? error.message : String(error)}`;
	}
}

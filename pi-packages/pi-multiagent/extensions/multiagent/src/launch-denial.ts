/** Launch-time fail-closed checks for resolved detached child steps. */

import { findProjectSettingsFile } from "./project-settings.ts";
import { verifyResolvedCallerSkillSources } from "./caller-skills.ts";
import { verifyResolvedExtensionSources } from "./tool-policy.ts";
import type { TeamStepSpec } from "./types.ts";

export function findStepLaunchDenial(step: TeamStepSpec): string | undefined {
	const extensionChange = verifyResolvedExtensionSources(step.agent.extensionTools);
	if (extensionChange) return extensionChange;
	const skillChange = verifyResolvedCallerSkillSources(step.agent.callerSkills);
	if (skillChange) return skillChange;
	if (!step.agent.tools.includes("bash")) return undefined;
	const settings = findProjectSettingsFile(step.cwd);
	return settings ? `bash-enabled child cwd is denied because project Pi settings exist at ${settings}.` : undefined;
}

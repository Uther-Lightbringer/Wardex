/** Product-level subagent skill propagation configuration. */

import type { AgentDiagnostic, SubagentSkillMode } from "./types.ts";
import { SUBAGENT_SKILL_MODE_VALUES } from "./types.ts";

export const SUBAGENT_SKILLS_FLAG = "agent-team-subagent-skills";
export const DEFAULT_SUBAGENT_SKILL_MODE: SubagentSkillMode = "enabled";

export interface SubagentSkillConfig {
	mode: SubagentSkillMode;
}

export function readSubagentSkillConfig(value: boolean | string | undefined): { config: SubagentSkillConfig; diagnostics: AgentDiagnostic[] } {
	if (value === undefined) return { config: { mode: DEFAULT_SUBAGENT_SKILL_MODE }, diagnostics: [] };
	if (typeof value !== "string") return invalidSubagentSkillConfig(value);
	const mode = value.trim().toLowerCase();
	if (isSubagentSkillMode(mode)) return { config: { mode }, diagnostics: [] };
	return invalidSubagentSkillConfig(value);
}

function invalidSubagentSkillConfig(value: unknown): { config: SubagentSkillConfig; diagnostics: AgentDiagnostic[] } {
	return {
		config: { mode: DEFAULT_SUBAGENT_SKILL_MODE },
		diagnostics: [{ code: "subagent-skills-config-invalid", message: `Invalid ${SUBAGENT_SKILLS_FLAG} value ${JSON.stringify(value)}. Use enabled or disabled.`, path: SUBAGENT_SKILLS_FLAG, severity: "error" }],
	};
}

function isSubagentSkillMode(value: string): value is SubagentSkillMode {
	return SUBAGENT_SKILL_MODE_VALUES.includes(value as SubagentSkillMode);
}

/** Step-local child invocation overrides. */

import type { GraphSpec } from "./schemas.ts";
import type { AgentDiagnostic, InvocationThinkingLevel } from "./types.ts";
import { INVOCATION_THINKING_LEVEL_VALUES } from "./types.ts";

const INVOCATION_THINKING_LEVEL_SET = new Set<string>(INVOCATION_THINKING_LEVEL_VALUES);

export interface StepAgentOverrides {
	model: string | undefined;
	thinking: InvocationThinkingLevel | undefined;
}

export function normalizeStepAgentOverrides(spec: GraphSpec["steps"][number]["agent"], diagnostics: AgentDiagnostic[], path: string): StepAgentOverrides | undefined {
	const model = normalizeStepModel(spec.model, diagnostics, `${path}/model`);
	if (model === null) return undefined;
	if (spec.thinking !== undefined && !INVOCATION_THINKING_LEVEL_SET.has(spec.thinking)) {
		diagnostics.push({ code: "step-agent-thinking-invalid", message: `Invalid step thinking level: ${spec.thinking}.`, path: `${path}/thinking`, severity: "error" });
		return undefined;
	}
	return { model, thinking: spec.thinking };
}

function normalizeStepModel(value: string | undefined, diagnostics: AgentDiagnostic[], path: string): string | undefined | null {
	if (value === undefined) return undefined;
	const model = value.trim();
	if (model.length > 0) return model;
	diagnostics.push({ code: "step-agent-model-required", message: "Step agent model must contain non-whitespace text.", path, severity: "error" });
	return null;
}

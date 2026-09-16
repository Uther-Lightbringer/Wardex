/** Dependency output selection for delegated step prompts. */

import { formatStepOutputText } from "./detached-output.ts";
import type { StepState } from "./detached-state.ts";
import type { StepOutput, TeamStepSpec } from "./types.ts";
import { INLINE_HANDOFF_CHARS, INLINE_HANDOFF_PREVIEW_CHARS } from "./types.ts";

export function collectUpstreamOutputs(step: TeamStepSpec, states: Map<string, StepState>): StepOutput[] {
	return upstreamRefs(step).map((dependency) => upstreamOutput(dependency, states)).filter((output): output is StepOutput => output !== undefined);
}

function upstreamRefs(step: TeamStepSpec): string[] {
	return [...new Set([...step.needs, ...step.after])];
}

function upstreamOutput(stepId: string, states: Map<string, StepState>): StepOutput | undefined {
	const state = states.get(stepId);
	if (!state?.output) return undefined;
	const fullText = state.finalText === undefined ? undefined : formatStepOutputText(state.finalText, state.assistantFinals, state.nonFinalText);
	if (fullText === undefined) return { ...state.output, text: undefined };
	if (fullText.length <= INLINE_HANDOFF_CHARS) return { ...state.output, text: fullText };
	return { ...state.output, text: oversizedUpstreamPreview(fullText, state.output.filePath) };
}

function oversizedUpstreamPreview(text: string, filePath: string | undefined): string {
	const preview = text.slice(0, INLINE_HANDOFF_PREVIEW_CHARS);
	const reference = filePath ? `Full output artifact: ${JSON.stringify(filePath)}` : "Full output artifact: unavailable";
	return `${preview}\n\n[agent_team upstream output truncated before handoff; ${reference}; full chars=${text.length}]`;
}

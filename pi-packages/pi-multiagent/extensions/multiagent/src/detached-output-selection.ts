/** Select bounded run_status and step_result outputs from detached step state. */

import { boundedStepOutput } from "./detached-output.ts";
import type { StepState } from "./detached-state.ts";
import type { AgentTeamDetails, StepOutput } from "./types.ts";

export function selectOutputsForAction(action: AgentTeamDetails["action"], stepId: string | undefined, maxBytes: number, includePreview: boolean, states: Iterable<StepState>, sinkStepIds: string[]): StepOutput[] {
	if (action === "step_result" && stepId) return selectStepResultOutput(stepId, maxBytes, includePreview, states);
	if (action === "run_status") return selectSinkOutputs(maxBytes, includePreview, states, sinkStepIds);
	return [];
}

function selectStepResultOutput(stepId: string, maxBytes: number, includePreview: boolean, states: Iterable<StepState>): StepOutput[] {
	for (const state of states) {
		if (state.spec.id !== stepId) continue;
		if (state.output) return [previewSelection(state.output, maxBytes, includePreview)];
		return [previewSelection({ stepId, status: state.status, text: state.liveText, filePath: undefined, chars: state.liveText.length, childSession: state.childSession, retryHistory: state.retryHistory.length > 0 ? state.retryHistory : undefined }, maxBytes, includePreview)];
	}
	return [];
}

function selectSinkOutputs(maxBytes: number, includePreview: boolean, states: Iterable<StepState>, sinkStepIds: string[]): StepOutput[] {
	const sinkIds = new Set(sinkStepIds);
	const outputs = [...states].map((state) => state.output).filter((output): output is StepOutput => output !== undefined && sinkIds.has(output.stepId));
	if (!includePreview) return outputs.map(withoutPreview);
	return boundedStepOutputs(outputs, maxBytes);
}

function previewSelection(output: StepOutput, maxBytes: number, includePreview: boolean): StepOutput {
	return includePreview ? boundedStepOutput(output, maxBytes) : withoutPreview(output);
}

function withoutPreview(output: StepOutput): StepOutput {
	return { ...output, text: undefined };
}

function boundedStepOutputs(outputs: StepOutput[], maxBytes: number): StepOutput[] {
	let remainingBytes = maxBytes;
	return outputs.map((output, index) => {
		if (output.text === undefined) return output;
		const remainingOutputs = Math.max(1, outputs.length - index);
		const budget = Math.max(1, Math.floor(remainingBytes / remainingOutputs));
		const bounded = boundedStepOutput(output, budget);
		remainingBytes = Math.max(0, remainingBytes - Buffer.byteLength(bounded.text ?? "", "utf8"));
		return bounded;
	});
}

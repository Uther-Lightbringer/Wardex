import { spawn } from "node:child_process";
import type { RunArtifactStore } from "./background-artifacts.ts";
import { buildDelegatedTask, writePromptFile } from "./delegated-prompt.ts";
import type { DetachedRunEventInput } from "./detached-run-options.ts";
import type { StepState } from "./detached-state.ts";
import { validateLaunchCwd } from "./launch-cwd.ts";
import { findStepLaunchDenial } from "./launch-denial.ts";
import { RpcChildController } from "./rpc-child-controller.ts";
import type { RpcStepResult } from "./rpc-child-types.ts";
import type { AgentTeamRuntimeOptions } from "./runtime-options.ts";
import { shouldRetryTransportFailure } from "./transport-retry-policy.ts";
import { collectUpstreamOutputs } from "./upstream-outputs.ts";
import type { ResolvedGraph, TeamStepSpec } from "./types.ts";

export async function runDetachedStepRpc(input: { runId: string; objective: string; limits: ResolvedGraph["limits"]; state: StepState; states: Map<string, StepState>; artifactStore: RunArtifactStore; options: AgentTeamRuntimeOptions; isRunRunning: () => boolean; finishState: (state: StepState, status: "failed", errorMessage: string) => void; finishFromRpcResult: (state: StepState, result: RpcStepResult) => void; updateLiveText: (state: StepState, text: string) => void; appendEvent: (event: DetachedRunEventInput) => void; touch: () => void }): Promise<void> {
	const { state } = input;
	if (state.status !== "running" || !input.isRunRunning()) return;
	const promptPath = writePromptFile(state.spec.agent, input.artifactStore, state.spec.id);
	if (state.status !== "running" || !input.isRunRunning()) return;
	const launchDenial = findStepLaunchDenial(state.spec) ?? validateLaunchCwd(state.spec);
	if (launchDenial) {
		input.finishState(state, "failed", launchDenial);
		return;
	}
	const task = buildDelegatedTask(input.objective, state.spec, collectUpstreamOutputs(state.spec, input.states));
	let attempt = 0;
	while (state.status === "running" && input.isRunRunning()) {
		const controller = new RpcChildController({
			agent: state.spec.agent,
			defaults: input.options.defaults,
			limits: input.limits,
			outputLimit: state.spec.outputLimit,
			cwd: state.spec.cwd,
			promptPath,
			sessionDir: input.options.sessionDir,
			sessionName: childSessionName(input.runId, state.spec, attempt),
			spawnProcess: input.options.spawnProcess ?? spawn,
			ackTimeoutMs: input.options.rpcCommandAckTimeoutMs,
			progressWatchdogMs: input.options.rpcProgressWatchdogMs,
			onText: (text) => input.updateLiveText(state, text),
			onChildSession: (metadata) => {
				state.childSession = metadata;
				input.touch();
			},
			onEvent: (event) => input.appendEvent({ ...event, stepId: state.spec.id }),
		});
		state.controller = controller;
		const result = await controller.run(task);
		state.controller = undefined;
		if (shouldRetryTransportFailure(state.spec, result, attempt)) {
			state.retryHistory.push({ attempt: attempt + 1, reason: result.errorMessage ?? "transport failure", childSession: result.childSession });
			input.appendEvent({ stepId: state.spec.id, type: "diagnostic", label: "transport_retry", preview: `retrying read-only step after transport failure: ${result.errorMessage ?? "unknown"}`, status: "running" });
			attempt += 1;
			continue;
		}
		input.finishFromRpcResult(state, result);
		return;
	}
}

function childSessionName(runId: string, step: TeamStepSpec, attempt: number): string {
	const retry = attempt > 0 ? ` retry-${attempt}` : "";
	return `agent_team ${runId}/${step.id} ${step.agent.ref}${retry}`;
}

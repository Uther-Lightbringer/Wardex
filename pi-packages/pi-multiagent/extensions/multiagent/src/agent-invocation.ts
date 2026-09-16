/** Effective child invocation metadata shared by launch, status, and artifacts. */

import type { AgentInvocationDefaults, AgentInvocationMetadata, ResolvedAgent } from "./types.ts";

export function effectiveAgentInvocation(agent: ResolvedAgent, defaults: AgentInvocationDefaults): AgentInvocationMetadata {
	return { model: agent.model ?? defaults.model, thinking: agent.thinking ?? defaults.thinking };
}

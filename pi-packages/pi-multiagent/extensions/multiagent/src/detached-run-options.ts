import type { AgentDiagnostic, AgentTeamNotice, CleanupReceipt, EventType, MessageReceipt, RunStatusWaitReceipt } from "./types.ts";

export interface DetachedRunDetailsOptions {
	cursor?: string;
	stepId?: string;
	maxBytes?: number;
	preview?: boolean;
	wait?: RunStatusWaitReceipt;
	message?: MessageReceipt;
	cleanup?: CleanupReceipt;
	ok?: boolean;
	error?: { code: string; message: string };
	includeEvents?: boolean;
	notice?: AgentTeamNotice;
	diagnostics?: AgentDiagnostic[];
}

export interface DetachedRunEventInput {
	stepId?: string;
	type: EventType;
	label?: string;
	preview?: string;
	status?: string;
}

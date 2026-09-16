/** Retained assistant-output budget for one RPC child step. */

import type { StepOutputLimit } from "./types.ts";
import { formatAssistantFinalMessages } from "./detached-output.ts";
import { DEFAULT_STEP_OUTPUT_LIMIT } from "./step-output-limit.ts";

const OUTPUT_BUDGET_LABEL = "step-output-budget-exceeded";

export interface OutputBudgetFailure {
	label: typeof OUTPUT_BUDGET_LABEL;
	message: string;
}

export type OutputBudgetCheck = { ok: true; bytes: number } | { ok: false; failure: OutputBudgetFailure };

export class AssistantOutputBudget {
	private liveTextBytes = 0;
	private assistantFinals: string[] = [];
	private readonly limit: StepOutputLimit;

	constructor(limit: StepOutputLimit = DEFAULT_STEP_OUTPUT_LIMIT) {
		this.limit = limit;
	}

	resetLiveText(): void {
		this.liveTextBytes = 0;
	}

	appendLiveTextDelta(delta: string): OutputBudgetCheck {
		const nextBytes = this.liveTextBytes + Buffer.byteLength(delta, "utf8");
		if (nextBytes > this.limit.maxBytes) return budgetExceeded("assistant text_delta", nextBytes, this.limit);
		this.liveTextBytes = nextBytes;
		return { ok: true, bytes: nextBytes };
	}

	measureText(text: string, label: string): OutputBudgetCheck {
		const bytes = Buffer.byteLength(text, "utf8");
		return bytes <= this.limit.maxBytes ? { ok: true, bytes } : budgetExceeded(label, bytes, this.limit);
	}

	setLiveTextBytes(bytes: number): void {
		this.liveTextBytes = bytes;
	}

	canAcceptAssistantFinal(text: string): OutputBudgetFailure | undefined {
		if (this.assistantFinals.length >= this.limit.maxAssistantFinals) {
			return { label: OUTPUT_BUDGET_LABEL, message: `step-output-budget-exceeded: Subagent emitted too many non-empty assistant finals; limit=${this.limit.maxAssistantFinals}.` };
		}
		const nextFinals = [...this.assistantFinals, text];
		const nextBytes = Buffer.byteLength(nextFinals.length === 1 ? text : formatAssistantFinalMessages(nextFinals), "utf8");
		return nextBytes <= this.limit.maxBytes ? undefined : budgetExceeded("assistant finals", nextBytes, this.limit).failure;
	}

	recordAssistantFinal(text: string): void {
		this.assistantFinals.push(text);
	}

	resetAssistantFinals(): void {
		this.assistantFinals = [];
	}
}

function budgetExceeded(label: string, bytes: number, limit: StepOutputLimit): { ok: false; failure: OutputBudgetFailure } {
	return {
		ok: false,
		failure: {
			label: OUTPUT_BUDGET_LABEL,
			message: `step-output-budget-exceeded: Subagent ${label} would retain ${bytes} bytes; per-step assistant output limit=${limit.maxBytes} bytes.`,
		},
	};
}

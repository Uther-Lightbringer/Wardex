import { MAX_PARENT_MESSAGE_CHARS_PER_STEP, MAX_PARENT_MESSAGES_PER_STEP } from "./types.ts";

export class ParentMessageBudget {
	private sent = 0;
	private chars = 0;

	reserve(text: string): string | undefined {
		if (this.sent >= MAX_PARENT_MESSAGES_PER_STEP) return this.message(this.chars);
		const nextChars = this.chars + text.length;
		if (nextChars > MAX_PARENT_MESSAGE_CHARS_PER_STEP) return this.message(nextChars);
		this.sent += 1;
		this.chars = nextChars;
		return undefined;
	}

	private message(chars: number): string {
		return `Parent message budget exceeded: attempts=${this.sent}/${MAX_PARENT_MESSAGES_PER_STEP}, chars=${chars}/${MAX_PARENT_MESSAGE_CHARS_PER_STEP}.`;
	}
}

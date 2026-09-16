export function messageChannelSemantics(channel: string): string {
	if (channel === "steer") return "Channel steer queues after the current assistant turn finishes tool calls, before the next LLM call; use it for clarification or scope correction, not impatience.";
	if (channel === "follow_up") return "Channel follow_up defers a live follow-up until the child is quiescent before terminalization, if still messageable. Use it only for short in-scope addenda: copy a needed artifact path; not post-terminal chat or premature finals.";
	return "Channel semantics are defined by child Pi RPC delivery.";
}

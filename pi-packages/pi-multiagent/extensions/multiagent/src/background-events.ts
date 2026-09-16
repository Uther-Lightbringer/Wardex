/** Bounded event store with run_status cursors for detached runs. */

import { isMaterialRunStatusWaitEvent } from "./material-wait-events.ts";
import type { BackgroundEvent, EventType } from "./types.ts";
import { EVENT_PREVIEW_CHARS, MAX_EVENTS_PER_RUN } from "./types.ts";

export class BackgroundEventStore {
	private seq = 0;
	private events: BackgroundEvent[] = [];

	append(input: { stepId?: string; type: EventType; label?: string; preview?: string; status?: string }): BackgroundEvent {
		const event: BackgroundEvent = {
			seq: ++this.seq,
			time: new Date().toISOString(),
			stepId: input.stepId,
			type: input.type,
			label: input.label,
			preview: input.preview ? truncate(input.preview, EVENT_PREVIEW_CHARS) : undefined,
			status: input.status,
		};
		this.events.push(event);
		if (this.events.length > MAX_EVENTS_PER_RUN) this.events.splice(0, this.events.length - MAX_EVENTS_PER_RUN);
		return event;
	}

	currentCursor(): string {
		return String(this.seq);
	}

	last(): BackgroundEvent | undefined {
		return this.events.at(-1);
	}

	hasMaterialAfter(cursor: string | undefined, stepId: string | undefined, sinkStepIds: readonly string[]): boolean {
		const after = parseCursor(cursor);
		return this.events.some((event) => event.seq > after && isMaterialRunStatusWaitEvent(event, { stepId, sinkStepIds }));
	}

	delta(cursor: string | undefined, stepId: string | undefined, maxBytes: number): { events: BackgroundEvent[]; cursor: string } {
		const after = parseCursor(cursor);
		const selected: BackgroundEvent[] = [];
		let bytes = 0;
		let deliveredCursor = after;
		for (const event of this.events) {
			if (event.seq <= after) continue;
			if (!eventMatchesWaitTarget(event, stepId)) {
				deliveredCursor = event.seq;
				continue;
			}
			const boundedEvent = eventBoundedForBudget(event, maxBytes);
			const eventBytes = eventByteLength(boundedEvent);
			if (selected.length > 0 && bytes + eventBytes > maxBytes) break;
			selected.push(boundedEvent);
			bytes += eventBytes;
			deliveredCursor = event.seq;
		}
		return { events: selected, cursor: String(selected.length === 0 ? Math.max(this.seq, after) : deliveredCursor) };
	}
}

function eventMatchesWaitTarget(event: BackgroundEvent, stepId: string | undefined): boolean {
	if (stepId === undefined) return true;
	return event.stepId === stepId || (event.type === "run" && (event.label === "terminal" || event.label === "cancel" || event.label === "expired"));
}

function parseCursor(cursor: string | undefined): number {
	if (!cursor) return 0;
	const value = Number(cursor);
	return Number.isFinite(value) && value > 0 ? Math.floor(value) : 0;
}

function eventBoundedForBudget(event: BackgroundEvent, maxBytes: number): BackgroundEvent {
	if (event.preview === undefined || eventByteLength(event) <= maxBytes) return event;
	const withoutPreview = { ...event, preview: undefined };
	if (eventByteLength(withoutPreview) > maxBytes) return withoutPreview;
	const suffix = "... [debug preview truncated by maxBytes]";
	let low = 0;
	let high = event.preview.length;
	let best: BackgroundEvent = withoutPreview;
	while (low <= high) {
		const length = Math.floor((low + high) / 2);
		const candidate = { ...event, preview: `${event.preview.slice(0, length)}${suffix}` };
		if (eventByteLength(candidate) <= maxBytes) {
			best = candidate;
			low = length + 1;
		} else high = length - 1;
	}
	return best;
}

function eventByteLength(event: BackgroundEvent): number {
	return Buffer.byteLength(JSON.stringify(event), "utf8");
}

function truncate(text: string, maxChars: number): string {
	return text.length <= maxChars ? text : `${text.slice(0, maxChars)}...`;
}

import { basename } from "node:path";
import type { AgentTeamDetails } from "./types.ts";

const MAX_IDS = 3;

export interface ArtifactOutput {
	stepId: string;
	status: AgentTeamDetails["outputs"][number]["status"];
	filePath: string | undefined;
	chars: number;
}

export function summarizeArtifacts(outputs: ArtifactOutput[]): string {
	const visible = outputs.slice(0, MAX_IDS).map((output) => artifactName(output.filePath)).join(",");
	return outputs.length > MAX_IDS ? `artifacts=${visible},+${outputs.length - MAX_IDS}` : `artifacts=${visible}`;
}

export function summarizeFullArtifactPaths(outputs: ArtifactOutput[]): string {
	const visible = outputs.slice(0, MAX_IDS).map((output) => `${output.stepId}=${output.filePath ?? "no artifact"}`).join(", ");
	return outputs.length > MAX_IDS ? `${visible}, +${outputs.length - MAX_IDS} more; use run_status/step_result` : visible;
}

export function terminalArtifacts(details: AgentTeamDetails): ArtifactOutput[] {
	const outputByStep = new Map(details.outputs.map((output) => [output.stepId, output]));
	const rows = details.steps.filter((step) => step.status !== "pending" && step.status !== "running").map((step) => {
		const output = outputByStep.get(step.id);
		return { stepId: step.id, status: step.status, filePath: step.outputFilePath ?? output?.filePath, chars: step.outputChars ?? output?.chars ?? 0 };
	});
	if (rows.length === 0) return details.outputs;
	return sinkFirst(rows, details.run?.sinkStepIds ?? []);
}

export function artifactName(path: string | undefined): string {
	return path ? basename(path) : "no artifact";
}

function sinkFirst(rows: ArtifactOutput[], sinkStepIds: string[]): ArtifactOutput[] {
	const sinkOrder = new Map(sinkStepIds.map((stepId, index) => [stepId, index]));
	return rows.map((row, index) => ({ row, index })).sort((left, right) => {
		const leftSink = sinkOrder.get(left.row.stepId);
		const rightSink = sinkOrder.get(right.row.stepId);
		if (leftSink !== undefined || rightSink !== undefined) {
			if (leftSink === undefined) return 1;
			if (rightSink === undefined) return -1;
			if (leftSink !== rightSink) return leftSink - rightSink;
		}
		return left.index - right.index;
	}).map((item) => item.row);
}

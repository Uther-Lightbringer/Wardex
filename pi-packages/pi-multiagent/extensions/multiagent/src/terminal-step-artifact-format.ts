import type { StepOutput, StepSnapshot } from "./types.ts";

const MAX_SECTION_BYTES = 8000;
const PATH_CHARS = 220;
const CWD_CHARS = 160;
const TASK_CHARS = 180;
const MAX_UPSTREAM_ARTIFACTS = 5;

export function formatTerminalStepArtifacts(steps: StepSnapshot[], outputs: StepOutput[] = []): string {
	const outputByStep = new Map(outputs.map((output) => [output.stepId, output]));
	const rows = steps.filter((step) => step.status !== "pending" && step.status !== "running").map((step) => formatStepArtifact(step, outputByStep.get(step.id)));
	return rows.length > 0 ? `\n## Terminal step artifacts\n${boundedRows(rows)}` : "";
}

function boundedRows(rows: string[]): string {
	const visible: string[] = [];
	let used = 0;
	for (let index = 0; index < rows.length; index += 1) {
		const row = rows[index] ?? "";
		const rowBytes = Buffer.byteLength(row, "utf8") + 1;
		if (visible.length > 0 && used + rowBytes > MAX_SECTION_BYTES) {
			visible.push(`- ... ${rows.length - index} more terminal step artifact(s); use step_result or artifact paths for full metadata.`);
			break;
		}
		visible.push(row);
		used += rowBytes;
	}
	return visible.join("\n");
}

function formatStepArtifact(step: StepSnapshot, output: StepOutput | undefined): string {
	const filePath = step.outputFilePath ?? output?.filePath;
	const chars = step.outputChars ?? output?.chars ?? 0;
	const artifact = filePath ? JSON.stringify(boundedModelText(filePath, PATH_CHARS)) : "none";
	const cwd = step.cwd ? ` cwd=${JSON.stringify(boundedModelText(step.cwd, CWD_CHARS))}` : "";
	const stopReason = step.stopReason ? ` stopReason=${JSON.stringify(boundedModelText(step.stopReason, CWD_CHARS))}` : "";
	const task = step.taskPreview ? ` task=${JSON.stringify(boundedModelText(step.taskPreview, TASK_CHARS))}` : "";
	const childSession = formatChildSession(step);
	const retries = step.retryHistory && step.retryHistory.length > 0 ? ` retries=${step.retryHistory.length}` : "";
	return `- ${step.id} [${step.status}]: artifact=${artifact} chars=${chars}${cwd}${childSession}${retries}${formatStepUpstreamArtifacts(step)}${stopReason}${task}`;
}

function formatChildSession(step: StepSnapshot): string {
	const session = step.childSession;
	if (!session) return "";
	const value = session.sessionFile ?? session.sessionId ?? session.sessionDir;
	return value ? ` childSession=${JSON.stringify(boundedModelText(value, PATH_CHARS))}` : " childSession=unavailable";
}

function formatStepUpstreamArtifacts(step: StepSnapshot): string {
	if (!step.upstreamArtifacts || step.upstreamArtifacts.length === 0) return " upstream=none";
	const visible = step.upstreamArtifacts.slice(0, MAX_UPSTREAM_ARTIFACTS).map((artifact) => `${modelText(artifact.stepId)}:${modelText(artifact.status)}=${artifact.filePath ? JSON.stringify(boundedModelText(artifact.filePath, PATH_CHARS)) : "none"}`);
	if (step.upstreamArtifacts.length > MAX_UPSTREAM_ARTIFACTS) visible.push(`+${step.upstreamArtifacts.length - MAX_UPSTREAM_ARTIFACTS} more`);
	return ` upstream=${visible.join(",")}`;
}

function boundedModelText(text: string, maxChars: number): string {
	const normalized = modelText(text);
	return normalized.length <= maxChars ? normalized : `${normalized.slice(0, maxChars)}... [truncated ${normalized.length - maxChars} chars]`;
}

function modelText(text: string): string {
	return text.replace(/(^|\r\n|\n|\r|\u2028|\u2029)(\[agent_team output (?:begin|end):)/g, "$1\\$2").replace(/\s+/g, " ").trim();
}

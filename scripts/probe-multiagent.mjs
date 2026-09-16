// RPC probe for pi-multiagent: spawns pi --mode rpc in a temp project, asks the
// agent to run the minimal read-only scout graph, and dumps every stdout line
// (RPC events) to a log file so we can inspect real event shapes.
//
// Usage: node scripts/probe-multiagent.mjs [timeoutSeconds]
// Requires: pi installed via `pi install npm:pi-multiagent` (~/.pi/agent/npm).
// Auth: reuses ~/.pi/agent auth (deepseek by default per settings.json).

import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PI_EXE = process.env["WARDEX_PI_EXE"] ?? "C:/workspace/pi/packages/coding-agent/dist/pi.exe";
const TIMEOUT_S = Number(process.argv[2] ?? 240);

const projDir = mkdtempSync(join(tmpdir(), "pi-ma-probe-"));
const logPath = join(projDir, "..", "pi-ma-probe-events.jsonl");
console.log(`[probe] project dir: ${projDir}`);
console.log(`[probe] event log:   ${logPath}`);

const PROMPT =
	`Use the agent_team tool to answer one scoped question about this directory. ` +
	`Run exactly this graph (read-only): objective "List what files exist here and summarize in one sentence.", ` +
	`authority { allowFilesystemRead: true }, one step id "inspect" with agent ref package:scout, task ` +
	`"List files. Do not edit or run commands. Return a one-sentence summary.". ` +
	`After starting, wait with run_status (waitSeconds 30), then fetch step_result with preview:true, then report the summary.`;

const child = spawn(
	PI_EXE,
	["--mode", "rpc", "--no-session"],
	{ stdio: ["pipe", "pipe", "pipe"], cwd: projDir },
);

let seq = 0;
const lines = [];
let pending = new Map(); // id -> resolve
let toolEvents = 0;

function send(obj) {
	const s = JSON.stringify(obj);
	console.log(`[probe -> pi] ${s.slice(0, 200)}`);
	child.stdin.write(s + "\n");
}

function request(type, params = {}) {
	return new Promise((resolve) => {
		const id = ++seq;
		pending.set(id, resolve);
		send({ id, type, ...params });
	});
}

child.stdout.setEncoding("utf8");
let buf = "";
child.stdout.on("data", (chunk) => {
	buf += chunk;
	let idx;
	while ((idx = buf.indexOf("\n")) >= 0) {
		const line = buf.slice(0, idx).trim();
		buf = buf.slice(idx + 1);
		if (!line) continue;
		lines.push(line);
		try {
			const evt = JSON.parse(line);
			if (evt.type === "message_update") {
				const e = evt.assistantMessageEvent ?? {};
				if (e.type === "toolcall_start")
					console.log(`[evt] toolcall_start idx=${e.contentIndex}`);
				else if (e.type === "toolcall_end")
					console.log(`[evt] toolcall_end args=${JSON.stringify(e.toolCall?.arguments)?.slice(0, 160)}`);
			} else if (String(evt.type).startsWith("tool_execution")) {
				toolEvents++;
				console.log(`[evt] ${evt.type} ${evt.toolName} :: ${JSON.stringify(evt.partialResult ?? evt.result ?? "").slice(0, 220)}`);
			} else if (evt.type === "response") {
				console.log(`[evt] response id=${evt.id} cmd=${evt.command} success=${evt.success} ${evt.error ?? ""}`.slice(0, 200));
				const p = pending.get(evt.id);
				if (p) { pending.delete(evt.id); p(evt); }
			} else if (evt.type === "agent_start") {
			} else if (evt.type === "agent_end") {
				console.log(`[evt] agent_end stopReason=${evt.stopReason ?? evt.reason ?? "?"}`);
			} else if (evt.type === "extension_ui_request") {
				console.log(`[evt] extension_ui_request!! ${line.slice(0, 300)}`);
			}
		} catch {
			console.log(`[raw] ${line.slice(0, 200)}`);
		}
	}
});

child.stderr.setEncoding("utf8");
child.stderr.on("data", (d) => console.error(`[stderr] ${d.trim().slice(0, 300)}`));
child.on("exit", (code) => console.log(`[probe] pi exited code=${code}`));

function fail(msg) {
	console.error(`[probe FAIL] ${msg}`);
	writeFileSync(logPath, lines.join("\n"));
	child.kill("SIGKILL");
	process.exit(1);
}

// --- handshake ---
await new Promise((r) => setTimeout(r, 2000));

const st = await request("get_state");
console.log(`[probe] state: cwd=${st.cwd ?? st.result?.cwd ?? "?"} model=${JSON.stringify(st.model ?? st.result?.model) ?? "?"}`);

// send prompt; response arrives immediately (success ack), agent_end signals completion
await request("prompt", { message: PROMPT });

await new Promise((resolve) => {
	const t = setInterval(() => {
		if (lines.some((l) => l.includes('"agent_end"'))) { clearInterval(t); resolve(); }
	}, 1000);
});

// drain a little more, then finish
setTimeout(() => {
	writeFileSync(logPath, lines.join("\n"));
	console.log(`\n[probe] DONE. events=${lines.length} tool_execution_events=${toolEvents}`);
	console.log(`[probe] full event stream written to ${logPath}`);
	try { rmSync(projDir, { recursive: true, force: true }); } catch {}
	child.kill("SIGKILL");
	process.exit(0);
}, 3000);

setTimeout(() => {
	console.warn("[probe] timeout — dumping what we have");
	writeFileSync(logPath, lines.join("\n"));
	child.kill("SIGKILL");
	process.exit(2);
}, TIMEOUT_S * 1000);

// Scripted ACP agent for hands-free sessions (HANDSFREE=1, see
// src/main/handsfree.ts): handsfree-fake-provider.sh runs it when Grok is
// started the way crates/engine-acp/src/adapter.rs starts Grok (its flags,
// then `agent stdio`), so a hands-free run has a second structured provider
// beside the fake Claude (T3 section 5: switch provider inside a thread). No
// account, network or provider state, and zero dependencies.
//
// It speaks ACP v1 as newline JSON-RPC on stdio, in the frames
// crates/engine-acp/src/adapter_tests.rs (`fake_agent`) uses: `initialize`
// (one model), `session/new` and `session/load` (any id it handed out, or
// one it is asked for), `session/prompt` (streams a short reply as
// `agent_message_chunk` updates, makes one deterministic edit in the
// session's folder through an `edit` tool call with a `diff` block, then ends
// the turn) and `session/cancel`. The edit:
//   write <path> <text>    create or overwrite <path> with <text>
//   append <path> <text>   append <text> to <path> as a line
//   anything else          append the prompt's first line to HANDSFREE-NOTES.md
// `slow` waits about 3 s before answering and `ask` asks the client's
// permission (`session/request_permission`) before the edit, skipping it when
// refused; a <path> outside the folder is refused. Any other request gets
// "method not found".
import fs from "node:fs";
import path from "node:path";

const VERSION = "1.0.0-handsfree";
const NOTES = "HANDSFREE-NOTES.md";
const SLOW_MS = 3_000;
// A real agent's updates arrive over time and its answer after them; one that
// wrote them all in one instant could be mapped after the turn's end.
const STEP_MS = 25;
const SETTLE_MS = 150;
const MODEL = { modelId: "grok-fake", name: "Grok (fake)" };

const sessions = new Map();
let nextSession = 0;
let nextRequest = 0;
const waiting = new Map();
let buffered = "";

const sleep = ms => new Promise(resolve => { setTimeout(resolve, ms); });

function send(frame) { process.stdout.write(`${JSON.stringify(frame)}\n`); }

function answer(id, result) { send({ jsonrpc: "2.0", id, result }); }

function fail(id, code, message) { send({ jsonrpc: "2.0", id, error: { code, message } }); }

async function update(sessionId, body) {
  send({ jsonrpc: "2.0", method: "session/update", params: { sessionId, update: body } });
  await sleep(STEP_MS);
}

/** Ends a prompt turn, once its updates have had time to land. */
async function endTurn(id) {
  await sleep(SETTLE_MS);
  answer(id, { stopReason: "end_turn" });
}

/** Asks the client something and resolves its result (or null on an error). */
function ask(method, params) {
  nextRequest += 1;
  const id = `fake-acp-${nextRequest}`;
  send({ jsonrpc: "2.0", id, method, params });
  return new Promise(resolve => { waiting.set(id, resolve); });
}


/** `<path>` resolved inside `cwd`, or null when it would leave it. */
function inside(cwd, relative) {
  const target = path.resolve(cwd, relative);
  return target === cwd || target.startsWith(`${cwd}${path.sep}`) ? target : null;
}

/** The edit a prompt asks for: the file, its text before and after. */
function plannedEdit(cwd, prompt) {
  const words = prompt.replace(/^\s*(slow|ask)\b\s*/giu, "").replace(/^\s*(slow|ask)\b\s*/giu, "");
  const command = /^(write|append)\s+(\S+)\s+([^\n]*)/u.exec(words);
  const [mode, relative, text] = command === null
    ? ["append", NOTES, prompt.split("\n")[0] ?? ""]
    : [command[1], command[2], command[3]];
  const file = inside(cwd, relative);
  if (file === null) return { refused: `${relative} is outside the working folder.` };
  let before = "";
  try { before = fs.readFileSync(file, "utf8"); } catch { before = ""; }
  const after = mode === "write" ? `${text}\n` : `${before}${text}\n`;
  return { file, before, after };
}

async function prompt(id, params) {
  const sessionId = params.sessionId;
  const session = sessions.get(sessionId);
  if (session === undefined) {
    fail(id, -32602, `unknown session ${String(sessionId)}`);
    return;
  }
  const text = (params.prompt ?? [])
    .filter(block => block?.type === "text")
    .map(block => block.text)
    .join("\n");
  session.turns += 1;
  if (/^\s*slow\b/iu.test(text) || /^\s*ask\s+slow\b/iu.test(text)) await sleep(SLOW_MS);
  await update(sessionId, { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Grok (fake) here. " } });
  const edit = plannedEdit(session.cwd, text);
  if (edit.refused !== undefined) {
    await update(sessionId, { sessionUpdate: "agent_message_chunk", content: { type: "text", text: edit.refused } });
    await endTurn(id);
    return;
  }
  const callId = `call-${sessionId}-${session.turns}`;
  const toolCall = {
    toolCallId: callId, title: `Edit ${path.relative(session.cwd, edit.file)}`, kind: "edit",
    rawInput: { path: edit.file },
  };
  let allowed = true;
  if (/^\s*(slow\s+)?ask\b/iu.test(text)) {
    const verdict = await ask("session/request_permission", {
      sessionId, toolCall,
      options: [
        { optionId: "allow-once", name: "Allow", kind: "allow_once" },
        { optionId: "reject-once", name: "Reject", kind: "reject_once" },
      ],
    });
    allowed = verdict?.outcome?.outcome === "selected" && verdict.outcome.optionId === "allow-once";
  }
  if (!allowed) {
    await update(sessionId, { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Left the files as they were." } });
    await endTurn(id);
    return;
  }
  await update(sessionId, {
    sessionUpdate: "tool_call", ...toolCall, status: "in_progress",
    content: [{ type: "diff", path: edit.file, oldText: edit.before, newText: edit.after }],
  });
  fs.mkdirSync(path.dirname(edit.file), { recursive: true });
  fs.writeFileSync(edit.file, edit.after);
  await update(sessionId, { sessionUpdate: "tool_call_update", toolCallId: callId, status: "completed", rawOutput: "done" });
  await update(sessionId, {
    sessionUpdate: "agent_message_chunk",
    content: { type: "text", text: `Updated ${path.relative(session.cwd, edit.file)}.` },
  });
  await endTurn(id);
}

function handle(frame) {
  if (frame.method === undefined) {
    const resolve = waiting.get(frame.id);
    if (resolve !== undefined) {
      waiting.delete(frame.id);
      resolve(frame.result ?? null);
    }
    return;
  }
  const { id, method, params = {} } = frame;
  switch (method) {
    case "initialize":
      answer(id, {
        protocolVersion: 1,
        agentInfo: { name: "handsfree-fake-acp", version: VERSION },
        agentCapabilities: { loadSession: true },
        models: { availableModels: [MODEL], currentModelId: MODEL.modelId },
      });
      return;
    case "session/new": {
      nextSession += 1;
      const sessionId = `fake-grok-${process.pid}-${nextSession}`;
      sessions.set(sessionId, { cwd: path.resolve(params.cwd ?? process.cwd()), turns: 0 });
      answer(id, { sessionId, models: { availableModels: [MODEL], currentModelId: MODEL.modelId } });
      return;
    }
    case "session/load": case "session/resume": {
      const sessionId = String(params.sessionId ?? "");
      if (!sessions.has(sessionId)) {
        sessions.set(sessionId, { cwd: path.resolve(params.cwd ?? process.cwd()), turns: 0 });
      }
      answer(id, { sessionId, models: { availableModels: [MODEL], currentModelId: MODEL.modelId } });
      return;
    }
    case "session/prompt":
      void prompt(id, params);
      return;
    case "session/cancel":
      return;
    default:
      if (id !== undefined) fail(id, -32601, `method not found: ${String(method)}`);
  }
}

process.stdin.setEncoding("utf8");
process.stdin.on("data", chunk => {
  buffered += chunk;
  const lines = buffered.split("\n");
  buffered = lines.pop() ?? "";
  for (const line of lines) {
    if (line.trim() === "") continue;
    try { handle(JSON.parse(line)); } catch { /* a frame it cannot read is not answered */ }
  }
});
process.stdin.on("end", () => { process.exit(0); });

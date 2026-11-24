// Scripted Claude Code for hands-free sessions (HANDSFREE=1, see
// src/main/handsfree.ts). handsfree-fake-provider.sh runs it when Claude is
// started the way crates/engine-claude/src/launch.rs starts it (stream-json in
// and out); every other provider invocation keeps the shell fake. No account,
// network or provider state, and zero dependencies.
//
// It answers the adapter's initialize with a model list, honours --session-id,
// --resume, --fork-session, --resume-session-at and the working directory, and
// for each prompt appends Claude's transcript entries to
// $HOME/.claude/projects/<cwd with every non-alphanumeric as ->/<id>.jsonl in
// the shape crates/engine-claude/src/store.rs reads, streams the events
// crates/engine-claude/src/mapper.rs maps, and makes one deterministic edit in
// the working directory, through a Write or Edit tool call:
//   write <path> <text>    create or overwrite <path> with <text>
//   append <path> <text>   append <text> to <path> as a line
//   anything else          append the prompt to HANDSFREE-NOTES.md
// Three more words change the turn: `slow` waits about 3 s before answering (a
// working state to look at), `ask` asks the host's permission before the
// edit, as a can_use_tool control request, and `burst` streams its text the
// way Opus does through Claude Code: a dozen deltas at once, about once a second;
// `signedout` fails the turn the way a signed-out Claude Code does
// ("error": "authentication_failed"); `limit <seconds>` ends the turn the way a
// rate-limited Claude Code does: a rejected `rate_limit_event` whose
// `rate_limit_info.resetsAt` (Unix seconds) is that many seconds ahead, then the
// synthetic "You've hit your limit" reply and an error result; every prompt
// before that reset is refused the same way (T3 section 10). <text> runs to the end of its line;
// a <path> outside the working directory is refused. A tool-less chat
// (`--tools ""`) and plan mode only reply. An Away loop's prompt ("bounded
// Interface Away loop") ends its reply with the AWAY_RESULT line Away reads:
// a work round appends to HANDSFREE-NOTES.md and says continue; a review
// changes nothing and says continue before the last round, complete on it.
// Team tools (protocol 57): `team start <task>` starts a thread through the
// `iface_team` server its --mcp-config names (start_thread, then wait_thread)
// and answers with the started thread's answer (T3 section 7 adds `team
// schedule <minutes> <prompt>`: schedule_prompt to this thread); `team wait [<thread>]` waits
// for one (the last it started by default). Both stream the tool calls as
// Claude does: a tool_use block, then its tool_result.
// Visual replies (protocol 61): `visual <title>` calls `show_visual` with a
// small bar-chart page (an inline script draws it, following the colour
// scheme) and then answers in words, as Claude would beside a visual.
// Private secrets (protocol 67): `secret <name> [<reason>]` calls
// `request_secret` and waits for the owner; given, it reads the file it was
// pointed at and says how long the value is (never the value), declined, it
// says so.
// One-shot print calls (`-p --output-format json`, title.rs): a title from
// the first prompt, or (protocol 67, Write with AI) a commit message or a pull
// request's text naming the files in the input's <changes>.
import { randomBytes, randomUUID } from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";

const VERSION = "1.0.0-handsfree";
const NOTES = "HANDSFREE-NOTES.md";
const SLOW_MS = 3_000;
const STEP_MS = 25;
const BURST_PIECES = 12;
const BURST_MS = 850;
const CONTEXT_WINDOW = 200_000;
const MODELS = [
  { value: "default", displayName: "Default (fake)", description: "Hands-free fake Claude · scripted replies, no network" },
  {
    value: "opus", displayName: "Opus (fake)", description: "Hands-free fake Claude · scripted replies, no network",
    supportsEffort: true, supportedEffortLevels: ["low", "medium", "high"],
  },
  { value: "haiku", displayName: "Haiku (fake)", description: "Hands-free fake Claude · scripted replies, no network" },
];
const MODEL_IDS = { default: "claude-fake-sonnet", opus: "claude-fake-opus", haiku: "claude-fake-haiku" };
const VALUE_FLAGS = new Set([
  "--output-format", "--input-format", "--effort", "--model", "--settings", "--add-dir",
  "--permission-prompt-tool", "--session-id", "--resume", "--allowedTools", "--disallowedTools",
  "--tools", "--mcp-config", "--setting-sources", "--permission-mode", "--resume-session-at",
  "--system-prompt",
]);

function parseArguments(argv) {
  const flags = new Set();
  const values = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    const equals = argument.indexOf("=");
    if (argument.startsWith("--") && equals > 0) values.set(argument.slice(0, equals), argument.slice(equals + 1));
    else if (VALUE_FLAGS.has(argument)) values.set(argument, argv[++index] ?? "");
    else flags.add(argument);
  }
  return { flags, values };
}

/** store.rs `project_dir_name`: every character but ASCII letters and digits becomes `-`. */
const projectDirName = cwd => [...cwd].map(character => (/^[A-Za-z0-9]$/u.test(character) ? character : "-")).join("");

/** store.rs `default_projects_root`. */
function projectsRoot(env) {
  const config = env.CLAUDE_CONFIG_DIR || (env.HOME ? path.join(env.HOME, ".claude") : ".claude");
  return path.join(config, "projects");
}

function findTranscript(root, cwd, sessionId) {
  const file = `${sessionId}.jsonl`;
  const scoped = path.join(root, projectDirName(cwd), file);
  if (fs.existsSync(scoped)) return scoped;
  let projects = [];
  try { projects = fs.readdirSync(root).filter(name => !name.startsWith(".")).sort(); } catch { return undefined; }
  return projects.map(name => path.join(root, name, file)).find(candidate => fs.existsSync(candidate));
}

/** The transcript messages a resume continues from and a fork copies. */
function transcriptMessages(file) {
  return fs.readFileSync(file, "utf8").split("\n").flatMap(line => {
    try {
      const record = JSON.parse(line);
      return (record?.type === "user" || record?.type === "assistant")
        && typeof record.uuid === "string" && record.isSidechain !== true ? [record] : [];
    } catch { return []; }
  });
}

function gitBranch(directory) {
  try {
    let git = path.join(directory, ".git");
    if (fs.statSync(git).isFile()) {
      git = path.resolve(directory, fs.readFileSync(git, "utf8").replace(/^gitdir:\s*/u, "").trim());
    }
    const head = fs.readFileSync(path.join(git, "HEAD"), "utf8").trim();
    return head.startsWith("ref: refs/heads/") ? head.slice("ref: refs/heads/".length) : "HEAD";
  } catch {
    return undefined;
  }
}

const token = bytes => randomBytes(bytes).toString("hex");
const tokens = text => Math.max(1, Math.ceil(text.length / 4));

function promptText(content) {
  if (typeof content === "string") return content;
  return Array.isArray(content)
    ? content.filter(block => block?.type === "text" && typeof block.text === "string").map(block => block.text).join("\n\n")
    : "";
}

/** What a prompt asks for. `target` is undefined when no edit is possible. */
/** An Away loop's round, from its prompt; undefined for any other prompt. */
function awayRound(prompt) {
  if (!/bounded Interface Away loop/u.test(prompt)) return undefined;
  const [, iteration = "1", limit = "1"] = /Iteration (\d+)\/(\d+)/u.exec(prompt) ?? [];
  return { iteration: Number(iteration), limit: Number(limit), review: /fresh independent reviewer/u.test(prompt) };
}

/** The work round's edit: one line in the notes, naming the round. */
function NOTES_LINE_OF(away) {
  return `append ${NOTES} Away round ${away.iteration}: one more line.`;
}

/** The AWAY_RESULT line that ends an Away round's reply. */
function awayResult(away) {
  const last = away.iteration >= away.limit;
  const result = away.review
    ? { outcome: last ? "complete" : "continue",
      summary: last ? `Round ${away.iteration} checked; no justified work remains.`
        : `Round ${away.iteration} checked: the line is there; one more round is justified.`,
      checks: [`read ${NOTES}: round ${away.iteration} line present`] }
    : { outcome: "continue", summary: `Round ${away.iteration}: appended a line to ${NOTES}.`,
      checks: [`cat ${NOTES}: passed`] };
  return `AWAY_RESULT ${JSON.stringify(result)}`;
}

/** A `team start <task>`, `team wait [<thread>]` or `team schedule <minutes> <prompt>` prompt; undefined otherwise. */
function teamVerb(prompt) {
  const match = /^\s*team\s+(start|wait|schedule)\b[ \t]*([\s\S]*)$/iu.exec(prompt);
  if (match === null) return undefined;
  return { verb: match[1].toLowerCase(), rest: match[2].trim() };
}

/** A `visual <title>` prompt's title (default "Weekly signups"); undefined otherwise. */
function visualVerb(prompt) {
  const match = /^\s*visual\b[ \t]*([^\n]*)/iu.exec(prompt);
  if (match === null) return undefined;
  return match[1].trim() || "Weekly signups";
}

/** The page `visual <title>` shows: one self-contained document, a canvas
 * bar chart its inline script draws, light or dark as the frame says. */
function visualChartPage(title) {
  const safe = title.replace(/[<>&"]/gu, character => `&#${character.codePointAt(0)};`);
  return `<!doctype html><html><head><meta charset="utf-8"><title>${safe}</title><style>
:root{color-scheme:light dark;--fg:#1d1d1f;--bar:#2f6fde;--muted:#6b6b70}
@media (prefers-color-scheme: dark){:root{--fg:#ececf0;--bar:#7aa7ff;--muted:#9a9aa2}}
body{margin:0;padding:16px 20px;font:14px -apple-system,system-ui,sans-serif;color:var(--fg);background:transparent}
h1{margin:0 0 4px;font-size:16px}p{margin:0 0 12px;color:var(--muted)}canvas{width:100%;max-width:680px;height:300px}
</style></head><body><h1>${safe}</h1><p>Hands-free fake data, drawn by an inline script.</p>
<canvas id="chart" width="680" height="300" aria-label="Bar chart"></canvas>
<script>
const values = [12, 19, 15, 27, 31, 24, 36];
const canvas = document.getElementById("chart");
const context = canvas.getContext("2d");
const style = getComputedStyle(document.documentElement);
const width = canvas.width / values.length;
values.forEach((value, index) => {
  const height = value / 40 * (canvas.height - 30);
  context.fillStyle = style.getPropertyValue("--bar").trim();
  context.fillRect(index * width + 10, canvas.height - 20 - height, width - 20, height);
  context.fillStyle = style.getPropertyValue("--muted").trim();
  context.fillText("W" + (index + 1), index * width + width / 2 - 8, canvas.height - 4);
});
document.body.dataset.drawn = String(values.length);
</script></body></html>`;
}

/** A `secret <name> [<reason>]` prompt's name and reason; undefined otherwise. */
function secretVerb(prompt) {
  const match = /^\s*secret\s+(\S+)[ \t]*([^\n]*)/iu.exec(prompt);
  if (match === null) return undefined;
  return { name: match[1], reason: match[2].trim() || `The hands-free fake Claude needs ${match[1]}.` };
}

/** The `iface_team` socket the --mcp-config names, or undefined. */
function teamSocket(mcpConfig) {
  try {
    const server = JSON.parse(mcpConfig ?? "")?.mcpServers?.iface_team;
    const args = Array.isArray(server?.args) ? server.args : [];
    const at = args.indexOf("-U");
    return at >= 0 && typeof args[at + 1] === "string" ? args[at + 1] : undefined;
  } catch {
    return undefined;
  }
}

/** One MCP client over the newline JSON-RPC socket; `call` answers a tool's text and whether it is an error. */
function teamClient(socket) {
  const connection = net.createConnection(socket);
  const waiting = new Map();
  let buffered = "";
  let next = 1;
  let failure;
  const failAll = error => {
    failure = error;
    for (const reject of waiting.values()) reject.reject(error);
    waiting.clear();
  };
  connection.setEncoding("utf8");
  connection.on("data", chunk => {
    buffered += chunk;
    for (let newline = buffered.indexOf("\n"); newline >= 0; newline = buffered.indexOf("\n")) {
      const line = buffered.slice(0, newline);
      buffered = buffered.slice(newline + 1);
      let message;
      try { message = JSON.parse(line); } catch { continue; }
      const pending = waiting.get(message?.id);
      if (pending === undefined) continue;
      waiting.delete(message.id);
      pending.resolve(message);
    }
  });
  connection.on("error", error => { failAll(error); });
  connection.on("close", () => { failAll(new Error("The team tools socket closed.")); });
  const request = (method, params) => new Promise((resolve, reject) => {
    if (failure !== undefined) { reject(failure); return; }
    const id = next++;
    waiting.set(id, { resolve, reject });
    connection.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
  });
  return {
    async call(name, args) {
      const reply = await request("tools/call", { name, arguments: args });
      if (reply.error !== undefined) return { text: reply.error.message ?? "MCP error", error: true };
      const text = reply.result?.content?.find(block => block?.type === "text")?.text ?? "";
      return { text, error: reply.result?.isError === true };
    },
    initialize: () => request("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "handsfree-fake-claude", version: VERSION } }),
    close() { connection.destroy(); },
  };
}

function planTurn(prompt, canEdit) {
  const slow = /\bslow\b/iu.test(prompt);
  const ask = /\bask\b/iu.test(prompt);
  if (!canEdit) return { slow, ask: false };
  const explicit = /(?:^|\s)(write|append)\s+(\S+)[ \t]+([^\n]+)/iu.exec(prompt);
  if (explicit) {
    return { slow, ask, verb: explicit[1].toLowerCase(), target: explicit[2], text: explicit[3].trim() };
  }
  return { slow, ask, verb: "append", target: NOTES, text: prompt.replace(/\s+/gu, " ").trim() };
}

/** The one-shot answer to `input`: Write with AI's text, else a title. */
function oneShotAnswer(input, systemPrompt) {
  if (/commit messages/u.test(systemPrompt ?? "")) {
    const files = [...input.matchAll(/^ [AMDR] (\S+)/gmu)].map(match => match[1]);
    const named = files.length === 0 ? "the changes" : files.slice(0, 3).join(", ");
    const subject = files.length === 1 ? `Update ${files[0]}` : `Update ${files.length} files`;
    return /^Write a pull request/u.test(input)
      ? `${subject} (hands-free)\n\nWritten by the hands-free fake Claude.\n\n- Changes ${named}`
      : `${subject}\n\nWritten by the hands-free fake Claude from ${named}.`;
  }
  const prompt = /<first-prompt>\n([\s\S]*?)\n<\/first-prompt>/u.exec(input)?.[1] ?? "conversation";
  return `Hands-free ${prompt.split(/\s+/u).slice(0, 4).join(" ")}`;
}

/** `-p --output-format json`: reads stdin to its end and prints one result. */
function printMode(options) {
  let input = "";
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", chunk => { input += chunk; });
  process.stdin.on("end", () => {
    const result = oneShotAnswer(input, options.values.get("--system-prompt"));
    process.stdout.write(`${JSON.stringify({ type: "result", subtype: "success", is_error: false, result,
      session_id: randomUUID(), total_cost_usd: 0 })}\n`);
  });
}

function main() {
  const options = parseArguments(process.argv.slice(2));
  if (options.flags.has("-p")) {
    printMode(options);
    return;
  }
  const cwd = process.cwd();
  const root = projectsRoot(process.env);
  const state = {
    sessionId: options.values.get("--session-id") || randomUUID(),
    file: undefined,
    inherited: [],
    parent: null,
    startupError: undefined,
    persist: !options.flags.has("--no-session-persistence"),
    partial: options.flags.has("--include-partial-messages"),
    toolless: options.values.get("--tools") === "",
    model: options.values.get("--model") || "default",
    permissionMode: options.values.get("--permission-mode") || "default",
    initEmitted: false,
    turns: 0,
    contextTokens: 0,
    teamSocket: teamSocket(options.values.get("--mcp-config")),
    lastTeamThread: undefined,
    /** When a `limit <seconds>` turn's limit resets (ms); prompts before then are refused. */
    limitedUntil: undefined,
  };

  const resume = options.values.get("--resume");
  const resumeAt = options.values.get("--resume-session-at");
  if (resume) {
    const source = findTranscript(root, cwd, resume);
    if (source === undefined) {
      state.startupError = `No conversation found with session ID: ${resume}`;
    } else {
      let messages = transcriptMessages(source);
      if (resumeAt) {
        const cut = messages.findIndex(record => record.uuid === resumeAt);
        if (cut < 0) state.startupError = `No message found with message.uuid of: ${resumeAt}`;
        messages = messages.slice(0, cut + 1);
      }
      if (options.flags.has("--fork-session")) {
        // Claude writes the copy only once the fork's first prompt arrives.
        state.inherited = messages.map(record => ({ ...record, sessionId: state.sessionId }));
      } else {
        state.sessionId = resume;
        state.file = source;
      }
      state.parent = messages.at(-1)?.uuid ?? null;
    }
  }
  state.file ??= path.join(root, projectDirName(cwd), `${state.sessionId}.jsonl`);

  const emit = value => { process.stdout.write(`${JSON.stringify(value)}\n`); };
  const stream = event => {
    if (state.partial) emit({ type: "stream_event", event, session_id: state.sessionId, parent_tool_use_id: null, uuid: randomUUID() });
  };
  const modelId = () => MODEL_IDS[state.model] ?? state.model;

  function append(type, message, extra = {}) {
    const record = {
      parentUuid: state.parent, isSidechain: false, userType: "external", cwd, sessionId: state.sessionId,
      version: VERSION, gitBranch: gitBranch(cwd), type, message, ...extra, uuid: randomUUID(), timestamp: new Date().toISOString(),
    };
    state.parent = record.uuid;
    if (!state.persist) return record;
    fs.mkdirSync(path.dirname(state.file), { recursive: true, mode: 0o700 });
    const lines = [...state.inherited, record].map(value => `${JSON.stringify(value)}\n`).join("");
    state.inherited = [];
    fs.appendFileSync(state.file, lines, { mode: 0o600 });
    return record;
  }

  /**
   * Waits `ms`, or less when the turn is cancelled; resolves whether it may
   * go on. A waker the turn already had (an iface_team client's close) is
   * put back after the wait and still runs on a cancel during it.
   */
  function pause(turn, ms) {
    return new Promise(resolve => {
      if (turn.cancelled) { resolve(false); return; }
      const outer = turn.wake;
      const timer = setTimeout(() => { turn.wake = outer; resolve(!turn.cancelled); }, ms);
      turn.wake = () => { clearTimeout(timer); outer?.(); resolve(false); };
    });
  }

  function cancel(turn) {
    turn.cancelled = true;
    const wake = turn.wake;
    turn.wake = undefined;
    wake?.();
  }

  /** One assistant API message: streamed blocks, one transcript entry and event per block. */
  async function respond(turn, blocks, stopReason) {
    const id = `msg_${token(12)}`;
    const usage = { input_tokens: state.contextTokens, output_tokens: 1, service_tier: "standard" };
    stream({ type: "message_start", message: { id, type: "message", role: "assistant", model: modelId(), content: [], stop_reason: null, stop_sequence: null, usage } });
    for (const [index, block] of blocks.entries()) {
      stream({ type: "content_block_start", index, content_block: block.type === "text" ? { type: "text", text: "" } : { ...block, input: {} } });
      const pieces = block.type === "text" ? block.text.match(/\S+\s*/gu) ?? [block.text] : [JSON.stringify(block.input)];
      for (const [at, piece] of pieces.entries()) {
        const wait = !turn.burst ? STEP_MS : at % BURST_PIECES === 0 ? BURST_MS : 0;
        if (wait > 0 && !(await pause(turn, wait))) return false;
        stream({
          type: "content_block_delta", index,
          delta: block.type === "text" ? { type: "text_delta", text: piece } : { type: "input_json_delta", partial_json: piece },
        });
      }
      stream({ type: "content_block_stop", index });
      const message = { id, type: "message", role: "assistant", model: modelId(), content: [block], stop_reason: null, stop_sequence: null, usage };
      const record = append("assistant", message, { requestId: `req_${token(12)}` });
      emit({ type: "assistant", message, parent_tool_use_id: null, session_id: state.sessionId, uuid: record.uuid });
    }
    const output = blocks.reduce((sum, block) => sum + tokens(block.text ?? JSON.stringify(block.input)), 0);
    state.contextTokens += output;
    stream({ type: "message_delta", delta: { stop_reason: stopReason, stop_sequence: null }, usage: { output_tokens: output } });
    stream({ type: "message_stop" });
    return true;
  }

  /** The Write or Edit call that makes the planned edit, or a refusal. */
  function toolCall(plan) {
    const file = path.resolve(cwd, plan.target);
    if (!file.startsWith(`${cwd}${path.sep}`)) return { refused: `\`${plan.target}\` is outside the working directory, so I left it alone.` };
    const current = fs.existsSync(file) ? fs.readFileSync(file, "utf8") : "";
    const id = `toolu_${token(12)}`;
    if (plan.verb === "write" || current === "") {
      return { id, file, name: "Write", input: { file_path: file, content: `${plan.text}\n` }, existed: fs.existsSync(file) };
    }
    const separator = current.endsWith("\n") ? "" : "\n";
    return { id, file, name: "Edit", input: { file_path: file, old_string: current, new_string: `${current}${separator}${plan.text}\n` } };
  }

  function applyTool(call, input) {
    try {
      if (call.name === "Write") {
        fs.mkdirSync(path.dirname(call.file), { recursive: true });
        fs.writeFileSync(call.file, input.content);
        return { ok: true, text: call.existed ? `The file ${call.file} has been updated.` : `File created successfully at: ${call.file}` };
      }
      const current = fs.readFileSync(call.file, "utf8");
      if (!current.includes(input.old_string)) return { ok: false, text: "String to replace not found in file." };
      fs.writeFileSync(call.file, current.replace(input.old_string, () => input.new_string));
      return { ok: true, text: `The file ${call.file} has been updated.` };
    } catch (error) {
      return { ok: false, text: error instanceof Error ? error.message : String(error) };
    }
  }

  /** Asks the host through the stdio permission-prompt tool; the answer is its control response. */
  function askPermission(turn, call) {
    const requestId = randomUUID();
    emit({
      type: "control_request", request_id: requestId,
      request: { subtype: "can_use_tool", tool_name: call.name, input: call.input, permission_suggestions: [], tool_use_id: call.id },
    });
    return new Promise(resolve => {
      approvals.set(requestId, resolve);
      turn.wake = () => {
        approvals.delete(requestId);
        emit({ type: "control_cancel_request", request_id: requestId });
        resolve({ behavior: "deny", message: "Interrupted" });
      };
    });
  }

  async function runTurn(turn) {
    const started = Date.now();
    const content = turn.message?.message?.content;
    const prompt = promptText(content).trim();
    state.turns += 1;
    state.contextTokens += tokens(prompt);
    append("user", { role: "user", content: content ?? prompt });
    if (!state.initEmitted) {
      state.initEmitted = true;
      emit({
        type: "system", subtype: "init", cwd, session_id: state.sessionId,
        tools: state.toolless ? [] : ["Bash", "Edit", "Read", "Write"],
        mcp_servers: state.teamSocket === undefined ? [] : [{ name: "iface_team", status: "connected" }], model: modelId(),
        permissionMode: state.permissionMode, slash_commands: [], apiKeySource: "none", claude_code_version: VERSION,
        output_style: "default", agents: [], skills: [], plugins: [], uuid: randomUUID(),
      });
    }
    if (/\bsignedout\b/iu.test(prompt)) {
      // What Claude Code 2.1 streams when nobody is signed in.
      if (/\bslow\b/iu.test(prompt) && !(await pause(turn, SLOW_MS))) return;
      const text = "Not logged in · Please run /login";
      emit({ type: "assistant", error: "authentication_failed", parent_tool_use_id: null,
        session_id: state.sessionId, uuid: randomUUID(),
        message: { id: randomUUID(), type: "message", role: "assistant", model: "<synthetic>",
          content: [{ type: "text", text }], stop_reason: "stop_sequence" } });
      emit({ type: "result", subtype: "success", is_error: true, terminal_reason: "api_error",
        duration_ms: 0, duration_api_ms: 0, num_turns: 1, result: text, session_id: state.sessionId,
        total_cost_usd: 0, usage: { input_tokens: 0, output_tokens: 0 }, modelUsage: {},
        permission_denials: [], uuid: randomUUID() });
      return;
    }
    const limit = /\blimit\s+(\d+)\b/iu.exec(prompt);
    if (limit !== null) state.limitedUntil = Date.now() + Number(limit[1]) * 1_000;
    if (limit !== null || (state.limitedUntil !== undefined && Date.now() < state.limitedUntil)) {
      // What Claude Code streams when the account's usage limit refuses a turn.
      const resetsAt = Math.ceil(state.limitedUntil / 1_000);
      emit({ type: "rate_limit_event", session_id: state.sessionId, uuid: randomUUID(),
        rate_limit_info: { status: "rejected", resetsAt, rateLimitType: "five_hour", isUsingOverage: false,
          overageStatus: "rejected", overageDisabledReason: "org_level_disabled" } });
      const reset = new Date(resetsAt * 1_000);
      const text = `You've hit your limit · resets ${String(reset.getHours()).padStart(2, "0")}:${String(reset.getMinutes()).padStart(2, "0")}`;
      emit({ type: "assistant", error: "rate_limit", parent_tool_use_id: null,
        session_id: state.sessionId, uuid: randomUUID(),
        message: { id: randomUUID(), type: "message", role: "assistant", model: "<synthetic>",
          content: [{ type: "text", text }], stop_reason: "stop_sequence" } });
      emit({ type: "result", subtype: "success", is_error: true, terminal_reason: "api_error",
        duration_ms: 0, duration_api_ms: 0, num_turns: 1, result: text, session_id: state.sessionId,
        total_cost_usd: 0, usage: { input_tokens: 0, output_tokens: 0 }, modelUsage: {},
        permission_denials: [], uuid: randomUUID() });
      return;
    }
    const team = teamVerb(prompt);
    if (team !== undefined) {
      await runTeamTurn(turn, team, prompt, started);
      return;
    }
    const secret = secretVerb(prompt);
    if (secret !== undefined) {
      await runSecretTurn(turn, secret, prompt, started);
      return;
    }
    const visual = visualVerb(prompt);
    if (visual !== undefined) {
      await runVisualTurn(turn, visual, prompt, started);
      return;
    }
    const away = awayRound(prompt);
    const canEdit = !state.toolless && state.permissionMode !== "plan" && away?.review !== true;
    // An Away round keeps a `slow` its objective asks for.
    const plan = planTurn(away === undefined ? prompt
      : `${/\bslow\b/iu.test(prompt) ? "slow " : ""}${NOTES_LINE_OF(away)}`, canEdit);
    turn.burst = /\bburst\b/iu.test(prompt);
    if (plan.slow && !(await pause(turn, SLOW_MS))) return;

    let closing;
    const denials = [];
    const call = plan.target === undefined ? undefined : toolCall(plan);
    if (call === undefined && away?.review === true) {
      closing = `Reviewed round ${away.iteration}: the notes line is in the worktree.`;
    } else if (call === undefined) {
      const why = state.toolless ? "this chat has no tools" : "plan mode makes no edits";
      closing = `**Hands-free fake Claude**, turn ${state.turns}. You wrote:\n\n> ${prompt.replace(/\n/gu, "\n> ")}\n\nNo files changed: ${why}.`;
    } else if (call.refused !== undefined) {
      closing = call.refused;
    } else {
      const name = path.relative(cwd, call.file);
      const opening = plan.verb === "write" ? `I'll write \`${name}\`.` : `I'll append a line to \`${name}\`.`;
      const toolUse = { type: "tool_use", id: call.id, name: call.name, input: call.input };
      if (!(await respond(turn, [{ type: "text", text: opening }, toolUse], "tool_use"))) return;
      let verdict = { behavior: "allow" };
      if (plan.ask) {
        verdict = await askPermission(turn, call);
        turn.wake = undefined;
        if (turn.cancelled) return;
      }
      const allowed = verdict?.behavior === "allow";
      const result = allowed
        ? applyTool(call, verdict.updatedInput && typeof verdict.updatedInput === "object" ? { ...call.input, ...verdict.updatedInput } : call.input)
        : { ok: false, text: typeof verdict?.message === "string" && verdict.message ? verdict.message : "Permission denied." };
      if (!allowed) denials.push({ tool_name: call.name, tool_use_id: call.id, tool_input: call.input });
      const toolResult = { type: "tool_result", tool_use_id: call.id, content: result.text, ...(result.ok ? {} : { is_error: true }) };
      const record = append("user", { role: "user", content: [toolResult] }, { toolUseResult: result.ok ? { type: call.name === "Write" ? "create" : "update", filePath: call.file } : `Error: ${result.text}` });
      emit({ type: "user", message: { role: "user", content: [toolResult] }, parent_tool_use_id: null, session_id: state.sessionId, uuid: record.uuid });
      closing = result.ok
        ? (plan.verb === "write" ? `Wrote \`${name}\`.` : `Appended a line to \`${name}\`.`)
        : `I didn't change \`${name}\`: ${result.text}`;
    }
    if (away !== undefined) closing += `\n${awayResult(away)}`;
    if (!(await respond(turn, [{ type: "text", text: closing }], "end_turn"))) return;
    emit({
      type: "result", subtype: "success", is_error: false, duration_ms: Date.now() - started, duration_api_ms: Date.now() - started,
      num_turns: call?.id === undefined ? 1 : 2, result: closing, stop_reason: "end_turn", session_id: state.sessionId, total_cost_usd: 0,
      usage: { input_tokens: tokens(prompt), cache_creation_input_tokens: 0, cache_read_input_tokens: 0, output_tokens: tokens(closing), service_tier: "standard" },
      modelUsage: {}, permission_denials: denials, uuid: randomUUID(),
    });
  }

  /** One MCP tool call streamed as Claude does: its tool_use, then its tool_result. */
  async function mcpTool(turn, client, name, input) {
    const id = `toolu_${token(12)}`;
    const toolUse = { type: "tool_use", id, name: `mcp__iface_team__${name}`, input };
    if (!(await respond(turn, [toolUse], "tool_use"))) return undefined;
    let result;
    try {
      result = await client.call(name, input);
    } catch (error) {
      result = { text: error instanceof Error ? error.message : String(error), error: true };
    }
    const toolResult = { type: "tool_result", tool_use_id: id, content: [{ type: "text", text: result.text }],
      ...(result.error ? { is_error: true } : {}) };
    const record = append("user", { role: "user", content: [toolResult] }, { toolUseResult: result.text });
    emit({ type: "user", message: { role: "user", content: [toolResult] }, parent_tool_use_id: null, session_id: state.sessionId, uuid: record.uuid });
    let value;
    try { value = JSON.parse(result.text); } catch { value = undefined; }
    return { ...result, value };
  }

  /** `visual <title>`: `show_visual` with a small chart, then a written answer. */
  async function runVisualTurn(turn, title, prompt, started) {
    let closing;
    if (state.teamSocket === undefined) {
      closing = "Visuals are not available in this session.";
    } else {
      const client = teamClient(state.teamSocket);
      turn.wake = () => { client.close(); };
      try {
        await client.initialize();
        const shown = await mcpTool(turn, client, "show_visual", { title, html: visualChartPage(title) });
        if (shown === undefined) return;
        closing = shown.error ? `I could not show the visual: ${shown.text}`
          : `Above is **${title}**: signups rose in five of the last seven weeks.`;
      } catch (error) {
        closing = `Team tools failed: ${error instanceof Error ? error.message : String(error)}`;
      } finally {
        turn.wake = undefined;
        client.close();
      }
    }
    if (turn.cancelled) return;
    if (!(await respond(turn, [{ type: "text", text: closing }], "end_turn"))) return;
    emit({
      type: "result", subtype: "success", is_error: false, duration_ms: Date.now() - started, duration_api_ms: Date.now() - started,
      num_turns: 1, result: closing, stop_reason: "end_turn", session_id: state.sessionId, total_cost_usd: 0,
      usage: { input_tokens: tokens(prompt), cache_creation_input_tokens: 0, cache_read_input_tokens: 0, output_tokens: tokens(closing), service_tier: "standard" },
      modelUsage: {}, permission_denials: [], uuid: randomUUID(),
    });
  }

  /** `secret <name>`: `request_secret`, then a written answer that never holds the value. */
  async function runSecretTurn(turn, secret, prompt, started) {
    let closing;
    if (state.teamSocket === undefined) {
      closing = "Secrets are not available in this session.";
    } else {
      const client = teamClient(state.teamSocket);
      turn.wake = () => { client.close(); };
      try {
        await client.initialize();
        const asked = await mcpTool(turn, client, "request_secret", { name: secret.name, reason: secret.reason });
        if (asked === undefined) return;
        if (asked.error) closing = `I could not ask for ${secret.name}: ${asked.text}`;
        else if (asked.value?.declined === true) closing = `You declined ${secret.name}, so I'll go on without it.`;
        else {
          const file = String(asked.value?.path ?? "");
          let length = 0;
          try { length = fs.readFileSync(file, "utf8").length; } catch { length = -1; }
          closing = length < 0 ? `I could not read ${secret.name} from \`${file}\`.`
            : `I have ${secret.name} (${length} characters) from \`${file}\`; I won't print it.`;
        }
      } catch (error) {
        closing = `Team tools failed: ${error instanceof Error ? error.message : String(error)}`;
      } finally {
        turn.wake = undefined;
        client.close();
      }
    }
    if (turn.cancelled) return;
    if (!(await respond(turn, [{ type: "text", text: closing }], "end_turn"))) return;
    emit({
      type: "result", subtype: "success", is_error: false, duration_ms: Date.now() - started, duration_api_ms: Date.now() - started,
      num_turns: 1, result: closing, stop_reason: "end_turn", session_id: state.sessionId, total_cost_usd: 0,
      usage: { input_tokens: tokens(prompt), cache_creation_input_tokens: 0, cache_read_input_tokens: 0, output_tokens: tokens(closing), service_tier: "standard" },
      modelUsage: {}, permission_denials: [], uuid: randomUUID(),
    });
  }

  /** `team start <task>` / `team wait [<thread>]` through the `iface_team` server. */
  async function runTeamTurn(turn, team, prompt, started) {
    let closing;
    if (state.teamSocket === undefined) {
      closing = "Team tools are not available in this session.";
    } else {
      const client = teamClient(state.teamSocket);
      turn.wake = () => { client.close(); };
      try {
        await client.initialize();
        if (team.verb === "schedule") {
          // T3 section 7: a prompt to this same thread, `<minutes>` from now.
          const parsed = /^(\d+)\s+([\s\S]+)$/u.exec(team.rest);
          const minutes = parsed === null ? 60 : Number(parsed[1]);
          const scheduled = await mcpTool(turn, client, "schedule_prompt",
            { prompt: parsed === null ? (team.rest || "Check again.") : parsed[2], when: { kind: "once", inMinutes: minutes } });
          if (scheduled === undefined) return;
          closing = scheduled.error ? `I could not schedule it: ${scheduled.text}`
            : `Scheduled ${scheduled.value?.id ?? "a prompt"} for ${scheduled.value?.nextAtIso ?? "later"}.`;
        }
        let thread = team.verb === "wait" ? (team.rest || state.lastTeamThread) : undefined;
        if (team.verb === "start") {
          const begun = await mcpTool(turn, client, "start_thread",
            { task: team.rest || "Say hello.", requestId: `handsfree-${token(6)}` });
          if (begun === undefined) return;
          if (begun.error) closing = `I could not start a thread: ${begun.text}`;
          thread = begun.value?.thread;
          if (typeof thread === "string") state.lastTeamThread = thread;
        }
        if (closing === undefined && team.verb !== "schedule" && typeof thread !== "string") {
          closing = "There is no thread to wait for.";
        }
        if (closing === undefined) {
          const waited = await mcpTool(turn, client, "wait_thread", { thread, timeoutSec: 600 });
          if (waited === undefined) return;
          const result = waited.value ?? {};
          closing = waited.error ? `I could not wait for ${thread}: ${waited.text}`
            : `The thread "${result.title ?? thread}" is ${result.status ?? "unknown"}`
              + (typeof result.answer === "string" ? `. It answered: ${result.answer}` : ".");
        }
      } catch (error) {
        closing = `Team tools failed: ${error instanceof Error ? error.message : String(error)}`;
      } finally {
        turn.wake = undefined;
        client.close();
      }
    }
    if (turn.cancelled) return;
    if (!(await respond(turn, [{ type: "text", text: closing }], "end_turn"))) return;
    emit({
      type: "result", subtype: "success", is_error: false, duration_ms: Date.now() - started, duration_api_ms: Date.now() - started,
      num_turns: 1, result: closing, stop_reason: "end_turn", session_id: state.sessionId, total_cost_usd: 0,
      usage: { input_tokens: tokens(prompt), cache_creation_input_tokens: 0, cache_read_input_tokens: 0, output_tokens: tokens(closing), service_tier: "standard" },
      modelUsage: {}, permission_denials: [], uuid: randomUUID(),
    });
  }

  const approvals = new Map();
  const queue = [];
  let running;
  let closed = false;

  async function drain() {
    while (queue.length > 0 && !closed) {
      running = { message: queue.shift(), cancelled: false, wake: undefined };
      try {
        await runTurn(running);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        emit({ type: "result", subtype: "error_during_execution", is_error: true, errors: [message], session_id: state.sessionId, uuid: randomUUID() });
      }
      running = undefined;
    }
  }

  const answer = (requestId, response) => emit({ type: "control_response", response: { subtype: "success", request_id: requestId, response } });
  const refuse = (requestId, error) => emit({ type: "control_response", response: { subtype: "error", request_id: requestId, error } });

  function control(requestId, request) {
    switch (request?.subtype) {
      case "initialize":
        if (state.startupError !== undefined) {
          refuse(requestId, state.startupError);
          finish(1);
          return;
        }
        answer(requestId, { commands: [], models: MODELS, agents: [], output_style: "default", available_output_styles: ["default"], pid: process.pid });
        return;
      case "interrupt":
        queue.length = 0;
        if (running !== undefined) cancel(running);
        answer(requestId, {});
        return;
      case "set_model":
        state.model = typeof request.model === "string" && request.model ? request.model : "default";
        answer(requestId, {});
        return;
      case "set_permission_mode":
        if (typeof request.mode === "string") state.permissionMode = request.mode;
        answer(requestId, {});
        return;
      case "get_context_usage":
        answer(requestId, {
          categories: [{ name: "Messages", tokens: state.contextTokens }], totalTokens: state.contextTokens, maxTokens: CONTEXT_WINDOW,
          percentage: (state.contextTokens / CONTEXT_WINDOW) * 100, isAutoCompactEnabled: true,
        });
        return;
      default:
        answer(requestId, {});
    }
  }

  function handle(line) {
    let frame;
    try { frame = JSON.parse(line); } catch { return; }
    switch (frame?.type) {
      case "control_request":
        control(frame.request_id, frame.request);
        return;
      case "control_response": {
        const response = frame.response ?? {};
        const resolve = approvals.get(response.request_id);
        if (resolve === undefined) return;
        approvals.delete(response.request_id);
        resolve(response.subtype === "success" ? response.response : { behavior: "deny", message: response.error });
        return;
      }
      case "user":
        queue.push(frame);
        if (running === undefined) void drain();
        return;
      default:
    }
  }

  /** Ends at stdin EOF: queued and running turns stop where they are. */
  function finish(code) {
    if (closed) return;
    closed = true;
    queue.length = 0;
    if (running !== undefined) cancel(running);
    process.exitCode = code;
    process.stdin.destroy();
  }

  process.stdout.on("error", () => { process.exit(0); });
  let buffered = "";
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", chunk => {
    buffered += chunk;
    for (let newline = buffered.indexOf("\n"); newline >= 0 && !closed; newline = buffered.indexOf("\n")) {
      const line = buffered.slice(0, newline).trim();
      buffered = buffered.slice(newline + 1);
      if (line) handle(line);
    }
  });
  process.stdin.on("end", () => { finish(0); });
}

main();

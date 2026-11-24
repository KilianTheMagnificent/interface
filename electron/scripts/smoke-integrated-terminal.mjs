import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { access, chmod, mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  RustHostClient, RUST_HOST_PROTOCOL_VERSION, RUST_HOST_PUSH_OPERATIONS, RUST_HOST_REQUEST_OPERATIONS,
} from "../dist-main/main/rust-host-client.js";
import {
  TerminalDataDirection,
  TerminalDataKind,
} from "../dist-main/main/terminal-data-transport.js";
import { cargoBinary } from "./cargo-target.mjs";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
// Not necessarily `<repoRoot>/target`: `CARGO_TARGET_DIR` or a user-level cargo
// config can move the target directory, so cargo is asked where it builds.
const defaultHost = cargoBinary(repoRoot, "debug", "interface-host");
const hostPath = process.argv[2] ?? defaultHost;
const timeoutMs = 15_000;

function delay(milliseconds) {
  return new Promise(resolve => { setTimeout(resolve, milliseconds); });
}

async function waitFor(read, description) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const result = await read();
    if (result !== undefined && result !== false) return result;
    if (Date.now() >= deadline) throw new Error(`Timed out waiting for ${description}.`);
    await delay(20);
  }
}

function terminalFromSnapshot(snapshot, id) {
  assert.equal(typeof snapshot, "object");
  assert.notEqual(snapshot, null);
  assert.ok(Array.isArray(snapshot.terminals));
  return snapshot.terminals.find(terminal => terminal.id === id);
}

await access(hostPath);
const temporary = await mkdtemp(join(tmpdir(), "interface-terminal-smoke-"));
const dataRoot = join(temporary, "data");
const testHome = join(temporary, "home");
const workspacePath = join(testHome, "Desktop/Projects");
const testShell = join(temporary, "test-login-shell");
const fakeClaude = join(temporary, "fake-claude");
const fakeOpencode = join(testHome, ".local/bin/opencode");
const fakeOmp = join(testHome, ".local/bin/omp");
const fakeMuse = join(testHome, ".local/bin/muse");
const fakeKimi = join(testHome, ".kimi-code/bin/kimi");
const unavailableProvider = "/usr/bin/false";
await Promise.all([
  mkdir(dataRoot, { recursive: true, mode: 0o700 }),
  mkdir(join(testHome, ".local/bin"), { recursive: true, mode: 0o700 }),
  mkdir(join(testHome, ".kimi-code/bin"), { recursive: true, mode: 0o700 }),
  mkdir(workspacePath, { recursive: true, mode: 0o700 }),
  mkdir(join(testHome, ".claude"), { recursive: true, mode: 0o700 }),
  mkdir(join(testHome, ".grok"), { recursive: true, mode: 0o700 }),
]);
await Promise.all([
  writeFile(fakeOmp, "#!/bin/sh\nprintf 'OMP_READY:%s:%s\\n' \"$PWD\" \"$#\"\nwhile IFS= read -r line; do [ \"$line\" = exit ] && exit 0; done\n", { mode: 0o700 }),
  writeFile(fakeMuse, "#!/bin/sh\nprintf 'MUSE_READY:%s:%s\\n' \"$PWD\" \"$#\"\nwhile IFS= read -r line; do [ \"$line\" = exit ] && exit 0; done\n", { mode: 0o700 }),
  writeFile(fakeKimi, "#!/bin/sh\nprintf 'KIMI_READY:%s:%s\\n' \"$PWD\" \"$#\"\nwhile IFS= read -r line; do [ \"$line\" = exit ] && exit 0; done\n", { mode: 0o700 }),
  writeFile(fakeOpencode, "#!/bin/sh\nprintf 'OPENCODE_READY:%s:%s:%s\\n' \"$PWD\" \"$#\" \"$*\"\nwhile IFS= read -r line; do [ \"$line\" = exit ] && exit 0; done\n", { mode: 0o700 }),
  writeFile(testShell, "#!/bin/sh\nexec /bin/sh -i\n", { mode: 0o700 }),
  writeFile(fakeClaude, [
    "#!/bin/sh",
    "printf '__IFACE_FAKE_CLAUDE_READY__:%s\\n' \"$PWD\"",
    "while IFS= read -r line; do",
    "  if [ \"$line\" = '__IFACE_FAKE_CLAUDE_EXIT__' ]; then",
    "    printf '__IFACE_FAKE_CLAUDE_BYE__\\n'",
    "    exit 0",
    "  fi",
    "  printf '__IFACE_FAKE_CLAUDE_ECHO__:%s\\n' \"$line\"",
    "done",
    "exit 1",
    "",
  ].join("\n"), { mode: 0o700 }),
]);
await Promise.all([chmod(testShell, 0o700), chmod(fakeClaude, 0o700)]);
const workspace = await realpath(workspacePath);

const hostOptions = {
  command: hostPath,
  cwd: repoRoot,
  env: {
    PATH: `${join(testHome, ".local/bin")}:${process.env.PATH ?? "/usr/bin:/bin:/usr/sbin:/sbin"}`,
    SHELL: testShell,
    INTERFACE_HOST_TEST_DATA_ROOT: dataRoot,
    INTERFACE_HOST_TEST_HOME: testHome,
    CODEX_EXECUTABLE: unavailableProvider,
    CLAUDE_CODE_EXECUTABLE: fakeClaude,
    CLAUDE_CONFIG_DIR: join(testHome, ".claude"),
    GROK_EXECUTABLE: unavailableProvider,
    GROK_HOME: join(testHome, ".grok"),
    QWEN_EXECUTABLE: unavailableProvider,
  },
};
let host = new RustHostClient(hostOptions);

async function invoke(operation, payload) {
  try {
    return await host.invoke(operation, payload);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`${operation} failed: ${message}`, { cause: error });
  }
}

/** Types `text` into one terminal generation as input frame `sequence`. */
function sendText(transport, terminalId, generation, sequence, text) {
  transport.send({
    terminalId,
    generation: BigInt(generation),
    sequence: BigInt(sequence),
    direction: TerminalDataDirection.ElectronToHost,
    kind: TerminalDataKind.Data,
    payload: Buffer.from(text),
  });
}

/** The host's snapshot of each terminal once every one of them has exited. */
function waitAllExited(ids, description) {
  return waitFor(async () => {
    const snapshot = await invoke("iface:listTerminals", {});
    const terminals = ids.map(id => terminalFromSnapshot(snapshot, id));
    return terminals.every(terminal => terminal?.state === "exited") ? terminals : undefined;
  }, description);
}

/** The host's snapshot of `id` once it has exited. */
async function waitExited(id, description) {
  const [terminal] = await waitAllExited([id], description);
  return terminal;
}

try {
  const ready = await host.start();
  assert.equal(ready.version, RUST_HOST_PROTOCOL_VERSION);
  assert.equal(ready.operations, RUST_HOST_REQUEST_OPERATIONS);
  assert.equal(ready.pushes, RUST_HOST_PUSH_OPERATIONS);

  const launchProfiles = await invoke("iface:listTerminalLaunchProfiles", {});
  assert.deepEqual(
    launchProfiles.map(profile => profile.id),
    ["shell", "claude", "codex", "grok", "gemini", "omp", "opencode", "muse", "kimi"],
  );
  assert.equal(launchProfiles.find(profile => profile.id === "muse")?.label, "Muse Code");
  assert.equal(launchProfiles.find(profile => profile.id === "kimi")?.label, "Kimi Code");
  assert.equal(launchProfiles[0].available, true);
  assert.equal(launchProfiles.find(profile => profile.id === "claude")?.available, true);
  assert.equal(launchProfiles.find(profile => profile.id === "gemini")?.label, "Antigravity");
  for (const profile of launchProfiles) {
    assert.ok(Object.keys(profile).every(key => [
      "id", "engine", "label", "available", "unavailableReason",
    ].includes(key)));
  }

  const transport = host.terminalDataTransport;
  assert.notEqual(transport, undefined);

  const output = new Map();
  const eof = new Set();
  const lifecycle = [];
  transport.onFrame(frame => {
    const key = `${frame.terminalId}:${frame.generation.toString()}`;
    if (frame.kind === TerminalDataKind.Eof) {
      eof.add(key);
      return;
    }
    const chunks = output.get(key) ?? [];
    chunks.push(Buffer.from(frame.payload));
    output.set(key, chunks);
  });
  host.onPush(frame => {
    if (frame.operation === "iface:terminalLifecycle") lifecycle.push(frame.payload);
  });

  const ompId = randomUUID();
  const omp = await invoke("iface:createTerminal", {
    id: ompId, cwd: workspace, size: { columns: 100, rows: 30 },
    title: "OMP", launch: { version: 1, profileId: "omp" },
  });
  assert.equal(omp.launch.profileId, "omp");
  assert.equal(omp.launchSession, undefined);
  await waitFor(() => Buffer.concat(output.get(`${ompId}:1`) ?? []).toString()
    .includes(`OMP_READY:${workspace}:0`), "OMP launch in the selected folder without injected flags");
  await invoke("iface:closeTerminal", { id: ompId, generation: 1 });
  await waitExited(ompId, "OMP exit");
  await invoke("iface:dismissTerminal", { id: ompId });

  const museId = randomUUID();
  const muse = await invoke("iface:createTerminal", {
    id: museId, cwd: workspace, size: { columns: 100, rows: 30 },
    title: "Muse Code", launch: { version: 1, profileId: "muse" },
  });
  assert.equal(muse.launch.profileId, "muse");
  assert.equal(muse.launchSession, undefined);
  await waitFor(() => Buffer.concat(output.get(`${museId}:1`) ?? []).toString()
    .includes(`MUSE_READY:${workspace}:0`), "Muse Code launch in the selected folder without injected flags");
  await invoke("iface:closeTerminal", { id: museId, generation: 1 });
  await waitExited(museId, "Muse Code exit");
  await invoke("iface:dismissTerminal", { id: museId });

  const kimiId = randomUUID();
  const kimi = await invoke("iface:createTerminal", {
    id: kimiId, cwd: workspace, size: { columns: 100, rows: 30 },
    title: "Kimi Code", launch: { version: 1, profileId: "kimi" },
  });
  assert.equal(kimi.launch.profileId, "kimi");
  assert.equal(kimi.launchSession, undefined);
  await waitFor(() => Buffer.concat(output.get(`${kimiId}:1`) ?? []).toString()
    .includes(`KIMI_READY:${workspace}:0`), "Kimi Code launch in the selected folder without injected flags");
  await invoke("iface:closeTerminal", { id: kimiId, generation: 1 });
  await waitExited(kimiId, "Kimi Code exit");
  await invoke("iface:dismissTerminal", { id: kimiId });

  const opencodeId = randomUUID();
  const opencode = await invoke("iface:createTerminal", {
    id: opencodeId, cwd: workspace, size: { columns: 100, rows: 30 },
    title: "OpenCode", launch: { version: 1, profileId: "opencode" },
  });
  assert.equal(opencode.launch.profileId, "opencode");
  assert.equal(opencode.launchSession, undefined);
  // The host launches OpenCode with `--auto` (auto-approved permissions) and
  // nothing else: no model or other injected flag.
  await waitFor(() => Buffer.concat(output.get(`${opencodeId}:1`) ?? []).toString()
    .includes(`OPENCODE_READY:${workspace}:1:--auto`), "OpenCode launch in the selected folder with only --auto");
  await invoke("iface:closeTerminal", { id: opencodeId, generation: 1 });
  await waitExited(opencodeId, "OpenCode exit");
  await invoke("iface:dismissTerminal", { id: opencodeId });

  const terminalA = randomUUID();
  const terminalB = randomUUID();
  const [createdA, createdB] = await Promise.all([
    invoke("iface:createTerminal", {
      id: terminalA,
      cwd: workspace,
      size: { columns: 80, rows: 24 },
      title: "Smoke A",
    }),
    invoke("iface:createTerminal", {
      id: terminalB,
      cwd: workspace,
      size: { columns: 100, rows: 30 },
      title: "Smoke B",
    }),
  ]);
  assert.equal(createdA.generation, 1);
  assert.equal(createdB.generation, 1);

  const resizedB = await invoke("iface:resizeTerminal", {
    id: terminalB,
    generation: 1,
    size: { columns: 92, rows: 27 },
  });
  assert.deepEqual(resizedB.size, { columns: 92, rows: 27 });
  const renamedB = await invoke("iface:renameTerminal", {
    id: terminalB,
    generation: 1,
    title: "Smoke B renamed",
  });
  assert.equal(renamedB.title, "Smoke B renamed");

  sendText(transport, terminalA, 1, 0, "printf 'terminal-a:%s\\n' \"$PWD\"; exit\n");
  sendText(transport, terminalB, 1, 0, "printf 'terminal-b:%s\\n' \"$PWD\"\n");

  const textFor = (id, generation) => Buffer.concat(
    output.get(`${id}:${generation.toString()}`) ?? [],
  ).toString("utf8");
  await waitFor(
    () => textFor(terminalA, 1).includes(`terminal-a:${workspace}`),
    "terminal A output",
  );
  await waitFor(
    () => textFor(terminalB, 1).includes(`terminal-b:${workspace}`),
    "terminal B output",
  );
  await waitFor(
    () => eof.has(`${terminalA}:1`),
    "terminal A EOF",
  );
  const exitedA = await waitExited(terminalA, "terminal A exit");
  assert.equal(exitedA.exit?.code, 0);

  await invoke("iface:closeTerminal", {
    id: terminalB,
    generation: 1,
  });
  await waitExited(terminalB, "terminal B forced close");

  const restartedA = await invoke("iface:restartTerminal", {
    id: terminalA,
    generation: 1,
  });
  assert.equal(restartedA.generation, 2);
  sendText(transport, terminalA, 2, 0, "printf 'terminal-a-restarted\\n'\n");
  await waitFor(
    () => textFor(terminalA, 2).includes("terminal-a-restarted"),
    "restarted terminal output",
  );
  await invoke("iface:closeTerminal", {
    id: terminalA,
    generation: 2,
  });
  await waitExited(terminalA, "restarted terminal close");

  await Promise.all([
    invoke("iface:dismissTerminal", { id: terminalA }),
    invoke("iface:dismissTerminal", { id: terminalB }),
  ]);

  const batchIds = Array.from({ length: 6 }, () => randomUUID());
  const batch = await Promise.all(batchIds.map((id, index) => invoke("iface:createTerminal", {
    id,
    cwd: workspace,
    size: { columns: 80, rows: 24 },
    title: `Shell ${index + 1}`,
    launch: { version: 1, profileId: "shell" },
  })));
  assert.deepEqual(batch.map(terminal => terminal.title),
    Array.from({ length: 6 }, (_, index) => `Shell ${index + 1}`));
  assert.ok(batch.every(terminal => terminal.generation === 1
    && terminal.launch?.version === 1
    && terminal.launch.profileId === "shell"));

  for (const [index, id] of batchIds.entries()) {
    sendText(transport, id, 1, 0, `printf 'terminal-batch-${index + 1}\\n'; exit\n`);
  }
  await Promise.all(batchIds.map((id, index) => waitFor(
    () => textFor(id, 1).includes(`terminal-batch-${index + 1}`),
    `terminal batch member ${index + 1} output`,
  )));
  await waitAllExited(batchIds, "six-terminal shell batch exit");
  await Promise.all(batchIds.map(id => invoke("iface:dismissTerminal", { id })));

  const claudeBatchIds = Array.from({ length: 6 }, () => randomUUID());
  const claudeBatch = await Promise.all(claudeBatchIds.map((id, index) => invoke(
    "iface:createTerminal",
    {
      id,
      cwd: workspace,
      size: { columns: 80, rows: 24 },
      title: `Claude ${index + 1}`,
      launch: { version: 1, profileId: "claude" },
    },
  )));
  assert.deepEqual(claudeBatch.map(terminal => terminal.title),
    Array.from({ length: 6 }, (_, index) => `Claude ${index + 1}`));
  assert.ok(claudeBatch.every(terminal => terminal.generation === 1
    && terminal.state === "running"
    && terminal.launch?.version === 1
    && terminal.launch.profileId === "claude"));

  await Promise.all(claudeBatchIds.map((id, index) => waitFor(
    () => textFor(id, 1).includes(`__IFACE_FAKE_CLAUDE_READY__:${workspace}`),
    `Claude batch member ${index + 1} readiness`,
  )));
  for (const [index, id] of claudeBatchIds.entries()) {
    sendText(transport, id, 1, 0, `claude-batch-input-${index + 1}\n`);
  }
  await Promise.all(claudeBatchIds.map((id, index) => waitFor(
    () => textFor(id, 1).includes(`__IFACE_FAKE_CLAUDE_ECHO__:claude-batch-input-${index + 1}`),
    `Claude batch member ${index + 1} unique echo`,
  )));
  for (const id of claudeBatchIds) {
    sendText(transport, id, 1, 1, "__IFACE_FAKE_CLAUDE_EXIT__\n");
  }
  await Promise.all(claudeBatchIds.map((id, index) => waitFor(
    () => textFor(id, 1).includes("__IFACE_FAKE_CLAUDE_BYE__")
      && eof.has(`${id}:1`),
    `Claude batch member ${index + 1} clean exit`,
  )));
  const exitedClaudeBatch = await waitAllExited(claudeBatchIds,
    "six-terminal Claude batch exit");
  assert.ok(exitedClaudeBatch.every(terminal => terminal?.exit?.code === 0));
  await Promise.all(claudeBatchIds.map(id => invoke("iface:dismissTerminal", { id })));

  const finalSnapshot = await invoke("iface:listTerminals", {});
  assert.deepEqual(finalSnapshot.terminals, []);
  assert.ok(lifecycle.length >= 10);

  // A picked project is not the host's default root. After a cold restart a
  // new terminal ID must use the retained host proof, without reopening the
  // old pane or asking the picker to grant the same directory again.
  const pickedPath = join(testHome, "chosen project");
  await mkdir(pickedPath);
  const pickedRoot = await realpath(pickedPath);
  assert.equal(await invoke("iface:pickDirectory", { selectedPath: pickedRoot }), pickedRoot);
  const saved = await invoke("iface:createTerminal", {
    id: randomUUID(), cwd: pickedRoot, title: "Saved root",
    launch: { version: 1, profileId: "shell" },
  });
  await invoke("iface:setTerminalWorkspaces", { workspaces: [{
    id: "saved-root", name: "Saved root",
    createdAt: "2026-09-08T00:00:00Z", updatedAt: "2026-09-08T00:00:00Z",
    terminals: [{ id: saved.id, title: saved.title, cwd: saved.cwd, launch: saved.launch }],
  }] });
  await host.stop();
  host = new RustHostClient(hostOptions);
  await host.start();
  assert.deepEqual((await invoke("iface:listTerminals", {})).terminals, []);
  const restartedTransport = host.terminalDataTransport;
  assert.ok(restartedTransport);
  const restartedOutput = new Map();
  restartedTransport.onFrame(frame => {
    if (frame.kind !== TerminalDataKind.Data) return;
    const prior = restartedOutput.get(frame.terminalId) ?? "";
    restartedOutput.set(frame.terminalId, prior + Buffer.from(frame.payload).toString());
  });
  for (const profileId of ["shell", "claude"]) {
    const fresh = await invoke("iface:createTerminal", {
      id: randomUUID(), cwd: pickedRoot, title: `Fresh ${profileId}`,
      launch: { version: 1, profileId },
    });
    assert.notEqual(fresh.id, saved.id);
    assert.equal(fresh.state, "running");
    assert.equal(fresh.root, pickedRoot);
    if (profileId === "shell") {
      sendText(restartedTransport, fresh.id, 1, 0,
        "printf '__SAVED_ROOT_CWD__:%s\\n' \"$PWD\"\n");
    }
    const marker = profileId === "shell" ? "__SAVED_ROOT_CWD__" : "__IFACE_FAKE_CLAUDE_READY__";
    await waitFor(() => restartedOutput.get(fresh.id)?.includes(`${marker}:${pickedRoot}`),
      `fresh ${profileId} PTY in the saved root after host restart`);
  }

  process.stdout.write(
    `Integrated terminal smoke passed: two lifecycle PTYs, one six-terminal shell batch, one six-terminal Claude batch, restart, forced close, ${lifecycle.length.toString()} lifecycle events, fresh shell and Claude PTYs in a saved root after cold host restart.\n`,
  );
} finally {
  await host.stop();
  assert.ok(temporary.startsWith(join(tmpdir(), "interface-terminal-smoke-")));
  await rm(temporary, { recursive: true, force: true });
}

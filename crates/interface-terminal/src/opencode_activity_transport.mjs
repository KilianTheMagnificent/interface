// Transport only: copy OpenCode TUI state into the private v1 snapshot.
// Rust owns working/waiting/idle, approval precedence and completion detection.
import { writeFileSync, renameSync, unlinkSync } from "node:fs";
import { join } from "node:path";

export const id = "interface-activity-v1";
export async function tui(api) {
  let disposed = false;
  let previous = "";
  let sentAt = 0;
  let family = [];
  let familyRoot;
  let refreshing = false;
  let recordedSession;
  let pickerPending = process.env.INTERFACE_OPENCODE_RESUME_PICKER === "1";
  const token = process.env.INTERFACE_OPENCODE_CAPTURE_TOKEN;
  const receiptsRoot = process.env.INTERFACE_OPENCODE_RECEIPTS_ROOT;
  const receiptFile = receiptsRoot && /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(token ?? "")
    ? join(receiptsRoot, token, ".interface-opencode-session.json") : undefined;
  const recordSession = session => {
    if (!receiptFile || !session || session.id === recordedSession) return;
    // This is a provider observation, not the canonical workspace store.
    // Rust validates its generation/process/directory before saving a binding.
    const temporary = `${receiptFile}.pending`;
    try {
      writeFileSync(temporary, JSON.stringify({ version: 1, captureToken: token,
        processId: process.pid, cwd: api.state.path.directory, sessionId: session.id }),
      { mode: 0o600, flag: "wx" });
      renameSync(temporary, receiptFile);
      recordedSession = session.id;
    } catch {
      try { unlinkSync(temporary); } catch { /* Retry the next sample. */ }
    }
  };
  const refresh = async () => {
    const route = api.route.current;
    const selected = route.name === "session" ? route.params?.sessionID : undefined;
    const session = selected ? api.state.session.get(selected) : undefined;
    const root = session?.parentID ?? selected;
    if (!root || refreshing) return;
    refreshing = true;
    try {
      const response = await api.client.session.children({ sessionID: root });
      if (!disposed && !response.error) {
        familyRoot = root;
        family = (response.data ?? []).slice(0, 30).map(item => item.id);
      }
    } catch { /* A later refresh retries this read-only snapshot. */ }
    finally { refreshing = false; }
  };
  const sample = () => {
    if (disposed || !api.state.ready) return;
    if (pickerPending) {
      try {
        api.keymap.dispatchCommand("session.list");
        pickerPending = false;
      } catch { /* Command registration may still be loading. */ }
    }
    const route = api.route.current;
    const selected = route.name === "session" ? route.params?.sessionID : undefined;
    const session = selected ? api.state.session.get(selected) : undefined;
    recordSession(session);
    const root = session?.parentID ?? selected;
    if (root !== familyRoot) void refresh();
    const ids = [...new Set([selected, root, ...(root === familyRoot ? family : [])].filter(Boolean))];
    const rows = ids.map(sessionID => {
      const item = api.state.session.get(sessionID);
      return [sessionID, item?.parentID ?? "-", api.state.session.status(sessionID)?.type ?? "idle",
        api.state.session.permission(sessionID).length, api.state.session.question(sessionID).length].join(",");
    });
    const snapshot = [selected ?? "-", ...rows].join("|");
    const now = Date.now();
    if (snapshot === previous && now - sentAt < 1000) return;
    if (snapshot.length > 8192 || /[\x00-\x1f\x7f]/u.test(snapshot)) return;
    try {
      api.renderer.setTerminalTitle(`interface-opencode-state:v1:${snapshot}`);
      previous = snapshot;
      sentAt = now;
    } catch { /* A closed PTY must not interrupt the provider. */ }
  };
  const removeCreated = api.event.on("session.created", () => { void refresh(); });
  const timer = setInterval(sample, 250);
  // Periodic readback also covers reconnects and children created during loading.
  const familyTimer = setInterval(() => { void refresh(); }, 2000);
  api.lifecycle.onDispose(() => {
    disposed = true;
    clearInterval(timer);
    clearInterval(familyTimer);
    removeCreated();
  });
  sample();
}

export default { id, tui };

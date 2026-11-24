import { startTaskResultPolling } from "./task-result-polling";
import { useSharedDraft } from "../renderer/use-shared-draft";
import type { TaskEvidence, SharedDraft } from "../shared/harness";
import { useCallback, useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import type { TerminalSummary } from "../shared/ipc";
import type { RemoteWorkspaceSnapshotV2, RemoteApprovalObservationV4, RemoteApprovalResponseV4, RemoteApprovalSnapshotV4, RemoteApprovalV4 } from "../shared/remote";
import { terminalPromptInputReady, terminalControlScreen } from "../renderer/terminal-prompt";
import { hasPendingInput, recordPendingInput, clearPendingInput } from "./pending-input";
import { RemoteConversation } from "./RemoteConversation";
import { PROVIDER_LOGO_CSS, ProviderLogo } from "../renderer/components/ProviderLogo";
import { TERMINAL_ANSI_PALETTES } from "../renderer/terminal-palettes";
import { decodeOutput, RemoteFailure, request } from "./protocol";
import { outputSocket, NativeOutputSocket, usesNativeTransport } from './native-transport';
import { startRosterPolling, type RosterPoller } from './roster-polling';
import { equalValue } from "../renderer/stable-values";

function Brand() {
  return <div className="remote-brand"><img src="/icon.svg" alt="" /><span>Interface</span><span className="remote-badge">Remote</span></div>;
}

export function RemoteApp() {
  const native = new URLSearchParams(location.search);
  const nativeSession = native.get("native") === "1";
  const nativeId = native.get("terminal");
  const nativeGeneration = Number(native.get("generation"));
  const [paired, setPaired] = useState<boolean | null>(null);
  const [workspaces, setWorkspaces] = useState<RemoteWorkspaceSnapshotV2["workspaces"]>([]);
  const terminals = workspaces.flatMap(workspace => workspace.terminals);
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<string | null>(nativeSession ? nativeId : null);
  const [error, setError] = useState("");
  const [code, setCode] = useState("");
  const [pending, setPending] = useState(false);
  const [online, setOnline] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const selectedTerminal = terminals.find(t => t.id === selected && (!nativeSession || t.generation === nativeGeneration));
  // A native session showing its terminal needs the list only for that
  // terminal, which its output and its stream report on (see roster-polling).
  const quietRoster = useRef(false);
  quietRoster.current = nativeSession && selectedTerminal !== undefined;
  const roster = useRef<RosterPoller | null>(null);
  useEffect(() => {
    const poller = startRosterPolling({
      request, quiet: () => quietRoster.current,
      workspaces: next => {
        // Keep the roster's identity while nothing changed, so the open
        // session's summary does not re-render its screen every poll.
        setWorkspaces(current => equalValue(current, next) ? current : next);
        setPaired(true); setOnline(true); setError("");
      },
      failed: error => {
        setOnline(false);
        if (error instanceof RemoteFailure && error.status === 401) { setPaired(false); setSelected(null); }
        else setError(error instanceof Error ? error.message : "The Mac is unavailable.");
      },
    });
    roster.current = poller;
    return () => { poller(); if (roster.current === poller) roster.current = null; };
  }, [refresh]);
  const rosterStale = useCallback(() => { roster.current?.stale(); }, []);
  const rosterRefresh = useCallback(() => { roster.current?.refresh(); }, []);
  const unpair = useCallback(() => { setPaired(false); setSelected(null); }, []);
  const pair = async () => {
    setPending(true); setError("");
    try {
      await request("/api/v1/pair", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code: code.trim(), name: "iPhone" }) });
      setCode(""); setPaired(true); setRefresh(value => value + 1);
    } catch (error) { setError(error instanceof Error ? error.message : "Pairing failed."); }
    finally { setPending(false); }
  };
  if (selectedTerminal) return <TerminalScreen key={`${selectedTerminal.id}:${selectedTerminal.generation}:${selectedTerminal.launchSession?.id ?? ""}:${selectedTerminal.launchSession?.resume ?? false}`}
    nativeSession={nativeSession} summary={selectedTerminal} workspace={workspaces.find(w => w.terminals.some(t => t.id === selectedTerminal.id))} onSelect={setSelected} online={online} onBack={() => setSelected(null)} onUnpair={unpair}
    onOutput={rosterStale} onDropped={rosterRefresh} />;
  if (nativeSession) return <main className="remote-app"><p className="remote-empty">{paired === false ? "Interface is locked. Return to Workspaces and unlock again." : error || (paired ? "This session has changed or closed. Return to Workspaces and choose it again." : "Opening your session…")}</p></main>;
  return <main className="remote-app">
    <header className="remote-header"><Brand /><span className="remote-connection"><i className={online ? "is-online" : ""} />{online ? "Mac connected" : "Connecting"}</span></header>
    {paired === false ? <section className="remote-pair">
      <span className="remote-eyebrow">YOUR WORK, WITH YOU</span><h1>Connect to<br />your Mac.</h1>
      <p>Open Interface on your Mac. In Settings → iPhone remote access, choose Pair iPhone.</p>
      <form onSubmit={e => { e.preventDefault(); void pair(); }}>
        <label htmlFor="pair-code">Pairing code</label><input id="pair-code" value={code} onChange={e => setCode(e.target.value)} placeholder="XXXX-XXXX-XXXX-XXXX" autoCapitalize="characters" autoCorrect="off" spellCheck={false} autoComplete="off" maxLength={32} />
        <button className="remote-primary" disabled={pending || !code.trim()}>{pending ? "Connecting…" : "Connect iPhone"}<span aria-hidden="true">↗</span></button>
      </form>
      <p className="remote-note">Your sessions continue on your Mac. This phone controls them while Interface is open and your Mac is awake.</p>
    </section> : <section className="remote-sessions">
      <div className="remote-section-head"><h1>Workspaces</h1><span className="remote-count">{terminals.length}</span></div>
      <label className="mobile-filter"><span className="codicon codicon-search" aria-hidden="true" /><input aria-label="Filter sessions" placeholder="Filter sessions" value={filter} onChange={e => setFilter(e.target.value)} /></label>
      {paired === null && <p className="remote-empty">Connecting to your Mac…</p>}
      {paired && terminals.length === 0 && <div className="remote-empty"><h2>No sessions open</h2><p>Open a workspace in Interface on your Mac. It will appear here.</p></div>}
      <div className="remote-session-list">{workspaces.map(workspace => {
        const visible = workspace.terminals.filter(t => `${workspace.name} ${t.title} ${t.cwd}`.toLocaleLowerCase().includes(filter.toLocaleLowerCase()));
        if (!visible.length) return null;
        return <section className="mobile-workspace" key={workspace.id}>
          <h2><span className="codicon codicon-folder" aria-hidden="true" />{workspace.name}<span>{visible.length}</span></h2>
          <p className="mobile-project-path">{workspace.root.split('/').filter(Boolean).slice(-2).join('/')}</p>
          {visible.map(terminal => <button className="remote-session" key={terminal.id} onClick={() => setSelected(terminal.id)}>
            <span className={`mobile-state-dot is-${terminal.state}`} />
            <span className="remote-session-title">{terminal.title || 'Terminal'}</span>
            <span className="remote-session-state">{terminal.state}</span><span aria-hidden="true">›</span>
          </button>)}
        </section>;
      })}</div>

    </section>}
    {error && <p className="remote-error" role="alert">{error}</p>}
  </main>;
}

function TerminalScreen({ summary, workspace, onSelect, online, onBack, onUnpair, onOutput, onDropped, nativeSession = false }: { nativeSession?: boolean; summary: TerminalSummary; workspace?: RemoteWorkspaceSnapshotV2['workspaces'][number]; onSelect: (id: string) => void; online: boolean; onBack: () => void; onUnpair: () => void;
  /** Output reached this view; its summary may have changed with it. */
  onOutput: () => void;
  /** A stream that had opened dropped: the terminal may have closed or restarted. */
  onDropped: () => void }) {
  const supportsConversation = summary.launch?.profileId === 'claude' || summary.launch?.profileId === 'codex';
  const [view, setView] = useState<'conversation' | 'terminal'>(supportsConversation ? 'conversation' : 'terminal');
  const mount = useRef<HTMLDivElement>(null);
  const emulator = useRef<Terminal | null>(null);
  const lease = useRef<string | null>(null);
  const leaseTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const claiming = useRef(false);
  const [claimPending, setClaimPending] = useState(false);
  const transmitting = useRef(false);
  const [controlling, setControlling] = useState(false);
  const [status, setStatus] = useState("Connecting…");
  const [error, setError] = useState("");
  const [gap, setGap] = useState(false);
  const { draft, setDraft, draftStatus, reloadDraft } = useSharedDraft({
    read: async () => await (await request(`/api/v1/terminals/${summary.id}/${summary.generation}/draft`)).json() as SharedDraft,
    save: async (expectedRevision, text) => await (await request(`/api/v1/terminals/${summary.id}/${summary.generation}/draft`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ expectedRevision, text }) })).json() as SharedDraft,
  });
  const messageField = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (!nativeSession || !messageField.current) return;
    const field = messageField.current;
    field.style.height = '0px';
    field.style.height = `${Math.min(132, Math.max(28, field.scrollHeight))}px`;
  }, [draft, nativeSession]);
  const [sending, setSending] = useState(false);
  const [inputReceipt, setInputReceipt] = useState('No input sent from this view.');
  const [taskResults, setTaskResults] = useState<TaskEvidence[]>([]);
  const [resultStatus, setResultStatus] = useState('Reading saved results…');
  useEffect(() => startTaskResultPolling({
    terminalId: summary.id, generation: summary.generation, request,
    results: next => setTaskResults(previous => equalValue(previous, next) ? previous : next),
    status: next => setResultStatus(next === 'ready'
      ? 'Saved results from your Mac. Checks apply to the recorded candidate.'
      : 'Mac results unavailable. Previously confirmed results remain below.'),
  }), [summary.id, summary.generation]);
  const [promptReady, setPromptReady] = useState(false);
  const [historyAvailable, setHistoryAvailable] = useState(false);
  const outputCursor = useRef(0);
  const output = useRef<WebSocket | NativeOutputSocket | null>(null);
  const showTerminalView = useCallback(() => { setView('terminal'); }, []);
  const continueOnMac = useCallback(() => { setError('Continue this request in Interface on your Mac.'); }, []);
  const [observation, setObservation] = useState<RemoteApprovalObservationV4 | null>(null);
  // Native output arrives in polls, and the Mac answers an approval
  // observation only for its latest cursor: ask once a poll comes back empty,
  // not after every page while output is still flowing.
  const polledOutput = usesNativeTransport();
  const [settledCursor, setSettledCursor] = useState<number | null>(null);
  const [approval, setApproval] = useState<{ view: RemoteApprovalV4; observed: RemoteApprovalObservationV4 } | null>(null);
  const target = `/api/v1/terminals/${summary.id}/${summary.generation}`;
  const pendingKey = `interface.remote.pending.${summary.id}.${summary.generation}`;
  const [uncertain, setUncertain] = useState(() => hasPendingInput(pendingKey));
  const sendRef = useRef<(data: Uint8Array, response?: RemoteApprovalResponseV4) => Promise<boolean>>(async () => false);
  sendRef.current = async (data, response) => {
    if (!lease.current || transmitting.current || claiming.current || uncertain || !online || status !== "Live terminal" || document.hidden) return false;
    transmitting.current = true; setSending(true); setError(""); setInputReceipt("Sending to your Mac…");
    const command = crypto.randomUUID();
    try {
      // Record uncertainty before transmission. Reloading cannot silently erase it.
      await recordPendingInput(pendingKey, command);
      await request(response ? `/api/v4/terminals/${summary.id}/${summary.generation}/approval/respond` : `${target}/input`, {
        method: "POST", headers: { "Content-Type": response ? "application/json" : "application/octet-stream", "X-Interface-Control": lease.current, "X-Interface-Command": command },
        body: response ? JSON.stringify(response) : new Uint8Array(data).buffer });
      // Its echo is due: a backed-off native output poll reads again now.
      if (output.current instanceof NativeOutputSocket) output.current.wake();
      await clearPendingInput(pendingKey); setInputReceipt("Mac accepted the input. Provider execution is confirmed by its conversation."); return true;
    } catch (error) {
      if (error instanceof RemoteFailure && error.replaySafe) {
        try { await clearPendingInput(pendingKey); } catch { setUncertain(true); }
      } else setUncertain(true);
      setInputReceipt(error instanceof RemoteFailure && error.replaySafe ? "Input was not accepted." : "Delivery is unknown. Inspect before repeating.");
      if (error instanceof RemoteFailure && error.status === 401) onUnpair();
      lease.current = null; setControlling(false);
      setError(error instanceof Error ? error.message : "Delivery is unknown. Check the session on your Mac before sending more.");
      return false;
    } finally { transmitting.current = false; setSending(false); }
  };
  useEffect(() => {
    if (!mount.current) return;
    const terminal = new Terminal({ cols: summary.size.columns, rows: summary.size.rows, scrollback: 4000,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace", fontSize: 12, cursorBlink: false,
      // Porcelain Night's screen, text and 16 colours: the phone view is always dark.
      theme: { ...TERMINAL_ANSI_PALETTES.dark, background: "#0d0d0c", foreground: "#e4e2de", cursor: "#e4e2de" },
      allowProposedApi: false,
    });
    terminal.open(mount.current); emulator.current = terminal;
    // The explicit composer and key strip serialize input. The browser's terminal
    // textarea is disabled so rapid key events cannot overtake command receipts.
    terminal.options.disableStdin = true;
    let stopped = false;
    let socket: WebSocket | NativeOutputSocket | null = null;
    let connecting = false;
    let timer: ReturnType<typeof setTimeout>;
    let cursor = 0;
    let writes = Promise.resolve();
    const connect = async () => {
      if (connecting || (socket && socket.readyState < WebSocket.CLOSING)) return;
      connecting = true;
      await writes;
      connecting = false;
      if (stopped || document.hidden) return;
      const connection = outputSocket(`${target}/stream?cursor=${cursor}`);
      socket = connection; output.current = connection; socket.binaryType = "arraybuffer";
      let opened = false;
      socket.onopen = () => { opened = true; if (!stopped) setStatus("Live terminal"); };
      if (connection instanceof NativeOutputSocket) connection.onsettled = () => { if (!stopped) setSettledCursor(cursor); };
      socket.onmessage = (event: { data: unknown }) => {
        if (!(event.data instanceof ArrayBuffer)) return;
        writes = writes.then(async () => {
          if (stopped) return;
          const frame = decodeOutput(event.data as ArrayBuffer);
          if (frame.gap) { terminal.reset(); setGap(true); cursor = frame.start; }
          if (frame.start !== cursor) throw new Error("Terminal output lost its position. Return to the session list and reopen it.");
          await new Promise<void>(resolve => terminal.write(frame.bytes, resolve));
          cursor = frame.end;
          outputCursor.current = cursor;
          onOutput();
          if (nativeSession) {
            setApproval(null);
            const screen = terminalControlScreen(terminal.buffer.active, terminal.rows, terminal.modes.bracketedPasteMode);
            setObservation(screen ? { version: 4, cursor, screen } : null);
          }
          const engine = summary.launch?.profileId;
          setPromptReady((engine === 'claude' || engine === 'codex')
            && terminalPromptInputReady(terminal.buffer.active, engine, terminal.modes.bracketedPasteMode));
        }).catch(error => { if (!stopped) { setError(String(error)); connection.close(); } });
        return writes;
      };
      socket.onclose = () => {
        if (output.current === connection) output.current = null;
        if (!stopped && socket === connection) {
          socket = null; setStatus("Reconnecting…"); clearTimeout(timer); timer = setTimeout(() => { void connect(); }, 1200);
          if (opened) onDropped();
        }
      };
    };
    const visibility = () => {
      lease.current = null; setControlling(false);
      clearTimeout(leaseTimer.current);
      if (document.hidden) { socket?.close(); void request(`${target}/control`, { method: "DELETE", keepalive: true }).catch(() => {}); }
      else { clearTimeout(timer); void connect(); }
    };
    document.addEventListener("visibilitychange", visibility);
    void connect();
    return () => {
      stopped = true; clearTimeout(timer); socket?.close(); document.removeEventListener("visibilitychange", visibility);
      clearTimeout(leaseTimer.current);
      void request(`${target}/control`, { method: "DELETE", keepalive: true }).catch(() => {});
      terminal.dispose(); emulator.current = null; lease.current = null;
    };
  }, [target]);
  useEffect(() => {
    if (!nativeSession || !supportsConversation || !online || status !== 'Live terminal' || !observation || uncertain) { setApproval(null); return; }
    if (polledOutput && settledCursor !== observation.cursor) return;
    let stopped = false;
    const abort = new AbortController();
    const timer = setTimeout(() => {
      void request(`/api/v4/terminals/${summary.id}/${summary.generation}/approval`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(observation), signal: abort.signal,
      }).then(async response => {
        const result: RemoteApprovalSnapshotV4 = await response.json();
        if (!stopped && result.version === 4 && outputCursor.current === observation.cursor)
          setApproval(result.approval ? { view: result.approval, observed: observation } : null);
      }).catch(() => { if (!stopped) setApproval(null); });
    }, polledOutput ? 0 : 180);
    return () => { stopped = true; clearTimeout(timer); abort.abort(); };
  }, [nativeSession, supportsConversation, online, status, observation, uncertain, target, polledOutput, settledCursor]);
  useEffect(() => { emulator.current?.resize(summary.size.columns, summary.size.rows); }, [summary.size.columns, summary.size.rows]);
  useEffect(() => {
    const viewport = window.visualViewport;
    if (!viewport) return;
    const resize = () => document.documentElement.style.setProperty("--remote-height", `${viewport.height}px`);
    resize(); viewport.addEventListener("resize", resize);
    return () => { viewport.removeEventListener("resize", resize); document.documentElement.style.removeProperty("--remote-height"); };
  }, []);
  const takeControl = async () => {
    if (claiming.current || document.hidden) return false;
    claiming.current = true;
    setClaimPending(true);
    setError("");
    const started = performance.now();
    try {
      const result: { lease: string } = await (await request(`${target}/control`, { method: "POST" })).json();
      if (!emulator.current || document.hidden) { void request(`${target}/control`, { method: "DELETE" }).catch(() => {}); return false; }
      lease.current = result.lease; setControlling(true);
      clearTimeout(leaseTimer.current);
      leaseTimer.current = setTimeout(() => { lease.current = null; setControlling(false); }, Math.max(0, 30_000 - (performance.now() - started)));
      return true;
    }
    catch (error) { setError(error instanceof Error ? error.message : "Could not enable replies."); return false; }
    finally { claiming.current = false; setClaimPending(false); }
  };
  const sendDraft = async () => {
    const terminal = emulator.current;
    if (!draft.trim() || !terminal || transmitting.current || claiming.current || uncertain) return;
    if (nativeSession && (!historyAvailable || !supportsConversation || !online || status !== 'Live terminal')) return;
    if (view === 'conversation') {
      const engine = summary.launch?.profileId;
      if ((engine !== 'claude' && engine !== 'codex')
        || !terminalPromptInputReady(terminal.buffer.active, engine, terminal.modes.bracketedPasteMode)) {
        setError(nativeSession ? 'A prompt or menu needs attention on your Mac before you can reply.' : 'Open Terminal view to handle the current prompt or menu.'); return;
      }
    }
    const text = draft.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
    if (/[\u0000-\u0008\u000b-\u001f\u007f]/.test(text)) { setError(nativeSession ? "Remove control characters from your message." : "Use the terminal key buttons for control characters."); return; }
    // A deliberate phone send acquires the same short, Mac-revocable lease.
    // No background takeover or automatic retry of a submitted message.
    if (nativeSession) {
      if (!await takeControl()) return;
      const engine = summary.launch?.profileId;
      if (!emulator.current || (engine !== 'claude' && engine !== 'codex')
        || !terminalPromptInputReady(terminal.buffer.active, engine, terminal.modes.bracketedPasteMode)) {
        setError('The session changed while connecting. Check it on your Mac before replying.'); return;
      }
    }
    const bytes = terminal.modes.bracketedPasteMode ? `\x1b[200~${text}\x1b[201~\r` : `${text}\r`;
    if (await sendRef.current(new TextEncoder().encode(bytes))) setDraft("");
  };
  const canSend = (nativeSession || controlling) && online && status === "Live terminal" && !claimPending && !sending && !uncertain;
  const respondToApproval = async (decision: 'allowOnce' | 'deny') => {
    if (!approval || !canSend || transmitting.current || claiming.current) return;
    const selected = approval;
    if (!await takeControl()) return;
    if (outputCursor.current !== selected.observed.cursor) { setError('The request changed. Review it again before responding.'); return; }
    const response: RemoteApprovalResponseV4 = { ...selected.observed, approvalId: selected.view.id, decision };
    if (await sendRef.current(new Uint8Array(), response)) setApproval(null);
  };
  const taskDetails = <details className="remote-input-note"><summary>Task results and input receipt</summary><p>{draftStatus} <button onClick={reloadDraft}>Reload shared draft</button></p><p role="status">{inputReceipt}</p><p>{resultStatus}</p>
        {taskResults.map(result => <details key={result.id}><summary>{result.objective}</summary><p>{result.root}</p><p>Candidate: {result.candidate}</p><ul>{result.checks.map((check, index) => <li key={index}>{check.name}: {check.status}</li>)}</ul><ul>{result.findings.map((finding, index) => <li key={index}>{finding}</li>)}</ul></details>)}
      </details>;
  if (nativeSession) return <main className="remote-terminal-page is-native-session phone-conversation-page">
    {(!online || status !== 'Live terminal') && <div className="phone-session-status" role="status">
      <span className="phone-connection-dot" />
      <span>Reconnecting to your Mac…</span>
    </div>}
    <div className="mobile-session-content is-conversation">
      {supportsConversation ? <RemoteConversation terminal={summary} visible refined onAvailable={setHistoryAvailable}
        onTerminal={continueOnMac} onUnpair={onUnpair} outputCursor={outputCursor} />
        : <section className="phone-unavailable" aria-label="Conversation unavailable">
          <span className="codicon codicon-comment-discussion" aria-hidden="true" />
          <h1>This session lives on your Mac</h1>
          <p>Phone conversations are available for Claude and Codex sessions. You can keep using this session in Interface on your Mac.</p>
        </section>}
      {/* Transport-only emulator tracks prompt safety; it is never a phone view. */}
      <div className="remote-terminal-scroll" aria-hidden="true" inert><div ref={mount} className="remote-emulator" /></div>
    </div>
    {supportsConversation && <div className={`remote-composer phone-composer${approval ? ' has-approval' : ''}`}>
      {error && <p className="remote-error" role="alert">{error}</p>}
      {uncertain && <div className="remote-warning" role="alert">Your last message may have arrived. Check the conversation on your Mac before sending again.
        <button onClick={() => { void clearPendingInput(pendingKey).then(() => setUncertain(false), () => setError('The last message could not be checked. Reopen the app.')); }}>I checked on my Mac</button>
      </div>}
      {approval && <section className="phone-approval" aria-label="Approval request">
        <div className="phone-approval-heading"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="M12 3 4 6v6c0 5 8 9 8 9s8-4 8-9V6Z" /><path d="m8 12 3 3 5-6" /></svg><h2>Needs your approval</h2><span className="phone-approval-provider">{summary.launch?.profileId === 'claude' ? 'Claude' : 'Codex'}</span></div>
        <p>{approval.view.title}</p>
        <pre tabIndex={0} aria-label="Request details">{approval.view.detail}</pre>
        {!approval.view.canAllowOnce && <p className="phone-composer-hint">Approve this request on your Mac. You can decline it here.</p>}
        <div className="phone-approval-actions">
          <button disabled={!canSend} onClick={() => { void respondToApproval('deny'); }}>Decline</button>
          <button className="phone-allow" disabled={!canSend || !approval.view.canAllowOnce} onClick={() => { void respondToApproval('allowOnce'); }}>Allow once</button>
        </div>
      </section>}
      {historyAvailable && !promptReady && !approval && online && status === 'Live terminal' && <p className="phone-composer-hint">Waiting for the session to accept a reply.</p>}
      <form className="phone-reply-form" hidden={!!approval} onSubmit={e => { e.preventDefault(); void sendDraft(); }}>
        <textarea ref={messageField} aria-label="Message" value={draft} onChange={e => setDraft(e.target.value)} placeholder={`Message ${summary.launch?.profileId === 'claude' ? 'Claude' : 'Codex'}…`}
          rows={1} autoCorrect="on" autoCapitalize="sentences" spellCheck maxLength={12000} />
        <button className="remote-send" disabled={!canSend || !draft.trim() || !promptReady || !historyAvailable || !!approval}
          aria-label="Send message">{sending || claimPending ? <span className="codicon codicon-loading codicon-modifier-spin" /> : <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M12 19V5m-6 6 6-6 6 6" /></svg>}</button>
      </form>
      {(taskResults.length > 0 || inputReceipt !== '') && taskDetails}
      <div className="phone-composer-footer">
        <span className="phone-composer-provider"><style>{PROVIDER_LOGO_CSS}</style><ProviderLogo engine={summary.launch?.profileId === 'claude' ? 'claude' : 'codex'} label="" size={13} />{summary.launch?.profileId === 'claude' ? 'Claude' : 'Codex'}</span>
        <span className="phone-composer-caption" role="status">{!online ? 'Mac unreachable' : sending ? 'Sending…' : claimPending ? 'Connecting…' : 'On your Mac'}</span>
      </div>
    </div>}
  </main>;
  return <main className={`remote-terminal-page${nativeSession ? " is-native-session" : ""}`}>
    {!nativeSession && <header className="remote-terminal-head"><button className="remote-back" onClick={onBack} aria-label="Back to sessions">←</button><div><h1>{summary.title || "Terminal"}</h1><p>{summary.launch?.profileId ?? "terminal"} · {status}</p></div><span className="remote-live-dot" /></header>}
    {!nativeSession && workspace && workspace.terminals.length > 1 && <nav className="mobile-session-tabs" aria-label="Workspace sessions">{workspace.terminals.map(t => <button key={t.id} aria-current={t.id === summary.id ? 'page' : undefined} onClick={() => onSelect(t.id)}>{t.title || 'Terminal'}</button>)}</nav>}
    <div className="mobile-view-switch" role="group" aria-label="Session view"><button aria-pressed={view === 'conversation'} disabled={!supportsConversation} onClick={() => setView('conversation')}>Conversation</button><button aria-pressed={view === 'terminal'} onClick={() => setView('terminal')}>Terminal</button></div>
    <div className="remote-control-bar"><span>{controlling ? "iPhone controls input" : "Viewing your Mac"}</span><button disabled={!online || sending || uncertain || claimPending} onClick={() => { void takeControl(); }}>{claimPending ? "Taking control…" : controlling ? "Renew control" : "Take control"}</button></div>
    {gap && <p className="remote-warning">Earlier output is outside the retained history. The terminal view was reset.</p>}
    <div className={`mobile-session-content is-${view}`}>
      {supportsConversation && <RemoteConversation terminal={summary} visible={view === 'conversation'} onTerminal={showTerminalView} onUnpair={onUnpair} outputCursor={outputCursor} />}
      <div className="remote-terminal-scroll" aria-hidden={view !== 'terminal'}><div ref={mount} className="remote-emulator" /></div>
    </div>
    <div className="remote-composer">
      <div className="remote-keys" hidden={view !== 'terminal'}>{[["Esc", "\x1b"], ["Tab", "\t"], ["↑", "\x1b[A"], ["↓", "\x1b[B"], ["Enter", "\r"], ["Ctrl-C", "\x03"]].map(([label, data]) => <button key={label} disabled={!canSend} onClick={() => { void sendRef.current(new TextEncoder().encode(data)); }}>{label}</button>)}</div>
      {error && <p className="remote-error" role="alert">{error}</p>}
      {uncertain && <div className="remote-warning">The last input may have arrived. Check the terminal before continuing.<button onClick={() => { void clearPendingInput(pendingKey).then(() => setUncertain(false), () => setError("Allow site storage before sending terminal input.")); }}>I checked the terminal</button></div>}
      {view === 'conversation' && !promptReady && <p className="remote-input-note">Open Terminal view to handle the current prompt or menu.</p>}
      <form onSubmit={e => { e.preventDefault(); void sendDraft(); }}>
        <textarea aria-label="Terminal input" value={draft} onChange={e => setDraft(e.target.value)} placeholder={controlling ? "Type into the current prompt…" : "Take control to send instructions"} rows={2} autoCorrect="off" autoCapitalize="off" spellCheck={false} maxLength={12000} />
        <button className="remote-send" disabled={!canSend || !draft || (view === 'conversation' && !promptReady)} aria-label="Send input">{sending ? "…" : "↑"}</button>
      </form>
      {taskDetails}
      <p className="remote-input-note">Typing on your Mac takes control back. Control expires after 30 seconds.</p>
    </div>
  </main>;
}

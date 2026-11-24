import { useEffect, useRef, useState } from "react";
import type { TerminalFastModeV1 } from "../../shared/ipc";
import { FastBolt } from "./FastBolt";

/** A CLI acknowledgement, not an optimistic preference. Key by generation so
 * a replacement process cannot inherit a pending click. */
export function TerminalFastModeToggle({ mode, onToggle }: {
  mode: TerminalFastModeV1;
  onToggle: () => void;
}) {
  const [pending, setPending] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const attempt = useRef<{ enabled: boolean; model: string } | null>(null);
  useEffect(() => {
    if (attempt.current === null) return;
    if (mode.enabled !== attempt.current.enabled || mode.model !== attempt.current.model) {
      attempt.current = null;
      setPending(false);
      setNotice(null);
    }
  }, [mode.enabled, mode.model]);
  useEffect(() => {
    if (!pending) return;
    const timer = window.setTimeout(() => {
      attempt.current = null;
      setPending(false);
      setNotice("Fast mode change was not confirmed. Check the CLI mode before trying again.");
    }, 3000);
    return () => { window.clearTimeout(timer); };
  }, [pending]);

  const label = mode.enabled ? "Turn off Fast mode" : "Turn on Fast mode";
  const title = pending ? "Waiting for Fast mode to update…"
    : !mode.ready ? "Fast mode can be changed when the terminal is ready"
    : notice ?? `${label} · ${mode.model}`;
  return <button type="button"
    className={`terminal-pane-action terminal-fast-mode${mode.enabled ? " is-on" : ""}`}
    aria-label={label} aria-pressed={mode.enabled} aria-busy={pending || undefined}
    disabled={pending || !mode.ready} title={title}
    onClick={() => {
      if (attempt.current !== null || !mode.ready) return;
      attempt.current = { enabled: mode.enabled, model: mode.model };
      setNotice(null);
      setPending(true);
      try {
        onToggle();
      } catch {
        attempt.current = null;
        setPending(false);
        setNotice("Fast mode could not be changed. Check the terminal connection.");
      }
    }}>
    <FastBolt />
    {notice !== null && <span className="sr-only" role="status">{notice}</span>}
  </button>;
}

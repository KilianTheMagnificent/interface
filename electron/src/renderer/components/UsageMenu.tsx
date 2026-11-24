import { useEffect, useId, useRef, type RefObject } from "react";
import {
  barWidth, formatReset, usageAccountKey, usageFreshness, usageRowLabel, usageStripItems,
  usageStripSummary, usageStripTone, type UsageEngineRef, type UsageEngineView,
} from "../usage-view";
import "../theme.css";

export interface UsageMenuState {
  status: "idle" | "loading" | "ready" | "error";
  views: UsageEngineView[];
  error: string | null;
  readAt: number | null;
}

export function UsageMenu({ state, now, open, engines, onOpenChange, onRefresh, anchorRef }: {
  state: UsageMenuState;
  now: number;
  open: boolean;
  engines: readonly UsageEngineRef[];
  onOpenChange: (open: boolean) => void;
  onRefresh: () => void;
  /** Opened from the window's ··· menu (owner, 2026-09-26): no trigger of its
      own, and Escape hands focus back to that button. The panel still hangs
      from this component's box, which App places at the toolbar's end. */
  anchorRef?: RefObject<HTMLElement | null>;
}) {
  const root = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const panelId = `${useId()}-usage-panel`;
  const items = usageStripItems(state.views, state.readAt, now, engines);
  const summary = usageStripSummary(items);
  const loading = state.status === "loading";
  const fresh = usageFreshness(state.readAt, now);

  useEffect(() => {
    if (!open) return;
    const onPointer = (event: PointerEvent): void => {
      if (root.current?.contains(event.target as Node) === true) return;
      onOpenChange(false);
    };
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      // A disclosure does not trap focus, so the only focus this close has to
      // repair is focus it is about to destroy: Refresh, inside the panel.
      // Dismissing by pointer elsewhere must NOT pull focus back here.
      const active = document.activeElement;
      if (active instanceof Node && panelRef.current?.contains(active) === true) {
        (anchorRef?.current ?? triggerRef.current)?.focus();
      }
      onOpenChange(false);
    };
    window.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [open, onOpenChange]);

  return (
    <div className="usage-menu" ref={root}>
      {/* A disclosure, not a dialog. The panel is read-only usage plus one
          Refresh button, it never takes focus on open and it traps nothing —
          announcing "dialog" described a surface this component does not
          implement. As a disclosure the panel is simply the next thing in tab
          order after its own trigger, which is where a keyboard user already
          expects Refresh to be, and Escape hands focus back only when it is
          about to be destroyed with the panel. */}
      {anchorRef === undefined && <button
        ref={triggerRef}
        type="button"
        className={open ? "usage-menu-trigger is-open" : "usage-menu-trigger"}
        aria-label="Provider usage"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        title={summary === ""
          ? "Plan usage for every provider"
          : fresh.text === ""
            ? summary
            : `${summary} — ${fresh.text}${fresh.stale ? ". Open to read it again." : ""}`}
        onClick={() => { onOpenChange(!open); }}
      >
        <span className="codicon codicon-dashboard" aria-hidden="true" />
        <span className="usage-menu-label">Usage</span>
      </button>}

      {open && (
        <div ref={panelRef} id={panelId} className="usage-menu-panel is-floating"
          role="group" aria-label="Provider usage details">
          <div className="usage-head">
            <span className="usage-title">Usage</span>
            {state.readAt !== null && (
              <span className="usage-read">
                {fresh.text}
                {fresh.stale && " — may have moved since"}
              </span>
            )}
            <span className="usage-head-actions">
              <button type="button" className="btn" disabled={loading} onClick={onRefresh}>
                {loading ? "Reading…" : "Refresh"}
              </button>
            </span>
          </div>

          <div className="usage-body">
            {loading && state.views.length === 0 && (
              <p className="usage-unknown" role="status">Asking each engine…</p>
            )}

            {state.status === "error" && (
              <p className="usage-note" role="alert">{state.error}</p>
            )}

            {state.status === "ready" && state.views.length === 0 && (
              <p className="usage-unknown">No engine reported any plan usage.</p>
            )}

            {state.views.map(view => (
              <UsageProvider key={usageAccountKey(view.engine, view.account)} view={view} now={now} />
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

/** The strip's tone on the bar: one set of cut points for both. */
function usageFillClass(percent: number): string {
  const tone = usageStripTone(percent);
  return tone === "ok" ? "usage-bar-fill" : `usage-bar-fill is-${tone}`;
}

function UsageProvider({ view, now }: { view: UsageEngineView; now: number }) {
  // "Claude", or "Claude (Work)" for another account's windows (protocol 60).
  const engineLabel = usageRowLabel(view);
  const labelId = `${useId()}-usage-provider`;
  return (
    <section className={view.available ? "usage-provider" : "usage-provider is-unavailable"}
      data-engine={view.engine} data-account={view.account} aria-labelledby={labelId}>
      <div className="usage-provider-head">
        <span id={labelId} className="usage-engine">{engineLabel}</span>
        {view.plan !== "" && <span className="usage-plan">{view.plan} plan</span>}
        {view.costText !== "" && (
          <span className="usage-cost">{view.costText} this session</span>
        )}
      </div>

      {!view.available ? (
        <p className="usage-unknown">Not reported.</p>
      ) : view.windows.length === 0 ? (
        <p className="usage-unknown">No limit windows reported.</p>
      ) : view.windows.map((limit, i) => {
        const resetText = formatReset(limit.resetsAt, now);
        return (
          <div className="usage-window" key={`${view.engine}:${limit.label}:${i}`}>
            <div className="usage-window-head">
              <span className="usage-window-label">{limit.label}</span>
              {resetText !== "" && <span className="usage-reset">{resetText}</span>}
              <span className={limit.percent === null
                ? "usage-pct usage-pct-unknown"
                : "usage-pct"}
              >
                {limit.percent === null ? limit.percentText : `${limit.percentText} used`}
              </span>
            </div>
            {limit.percent !== null && (
              <div
                className="usage-bar"
                role="img"
                aria-label={`${limit.label}: ${limit.percentText} used`}
              >
                <div
                  className={usageFillClass(limit.percent)}
                  style={{ width: `${barWidth(limit.percent)}%` }}
                />
              </div>
            )}
          </div>
        );
      })}

      {view.note !== "" && <p className="usage-note">{view.note}</p>}
    </section>
  );
}

import {
  createContext, useContext, useLayoutEffect, useMemo, useRef, useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import {
  TERMINAL_VIEW_ATTACHMENT_EVENT, TERMINAL_VIEW_DETACHMENT_EVENT,
} from "../terminal-view-attachment";
import type { TerminalPaneRenderContext } from "./TerminalPane";

interface Entry {
  readonly mount: HTMLDivElement;
  context: TerminalPaneRenderContext;
  slot: HTMLDivElement | null;
}

interface Pool {
  attach(key: string, context: TerminalPaneRenderContext, slot: HTMLDivElement): void;
  detach(key: string, slot: HTMLDivElement): void;
}

/** A pooled view's context: its slot's, plus whether its terminal is mid-turn. */
export interface PooledTerminalRenderContext extends TerminalPaneRenderContext {
  readonly working: boolean;
}

const PoolContext = createContext<Pool | null>(null);
const fill = { height: "100%", minHeight: 0, minWidth: 0 };

/**
 * The workspace owns slots; this pool owns emulators for the lifetime of their
 * PTY generation. A bounded byte tail cannot reconstruct a terminal's screen,
 * scrollback, parser and input modes after an arbitrary workspace switch.
 * Park visited views in a hidden, inert container and keep consuming output.
 */
export function TerminalViewPool({ streamKeys, workingKeys, renderTerminal, children }: {
  readonly streamKeys: ReadonlySet<string>;
  /** Streams whose terminal is mid-turn. Handed to each view here, so the
   *  renderer every pane holds need not change when any terminal's does. */
  readonly workingKeys: ReadonlySet<string>;
  readonly renderTerminal: (context: PooledTerminalRenderContext) => ReactNode;
  readonly children: ReactNode;
}) {
  const entries = useRef(new Map<string, Entry>());
  const parking = useRef<HTMLDivElement>(null);
  const [, invalidate] = useState(0);
  const pool = useMemo<Pool>(() => ({
    attach(key, context, slot) {
      let entry = entries.current.get(key);
      if (entry === undefined) {
        const mount = document.createElement("div");
        Object.assign(mount.style, fill);
        entry = { mount, context, slot };
        entries.current.set(key, entry);
      }
      entry.context = context;
      entry.slot = slot;
      if (entry.mount.parentElement !== slot) {
        slot.appendChild(entry.mount);
        for (const host of entry.mount.querySelectorAll(".terminal-xterm")) {
          host.dispatchEvent(new Event(TERMINAL_VIEW_ATTACHMENT_EVENT));
        }
      }
      invalidate(value => value + 1);
    },
    detach(key, slot) {
      const entry = entries.current.get(key);
      if (entry === undefined || entry.slot !== slot) return;
      entry.slot = null;
      parking.current?.appendChild(entry.mount);
      // Hidden renderers become eviction candidates for the bounded GPU cache.
      for (const host of entry.mount.querySelectorAll(".terminal-xterm")) {
        host.dispatchEvent(new Event(TERMINAL_VIEW_DETACHMENT_EVENT));
      }
      invalidate(value => value + 1);
    },
  }), []);

  useLayoutEffect(() => {
    for (const [key, entry] of entries.current) {
      if (streamKeys.has(key)) continue;
      entry.mount.remove();
      entries.current.delete(key);
    }
  }, [streamKeys]);

  return <PoolContext.Provider value={pool}>
    {children}
    <div ref={parking} hidden inert aria-hidden="true" data-terminal-parking="" />
    {[...entries.current].filter(([key]) => streamKeys.has(key)).map(([key, entry]) =>
      createPortal(renderTerminal({ ...entry.context,
        active: entry.slot !== null && entry.context.active,
        working: workingKeys.has(key),
      }), entry.mount, key))}
  </PoolContext.Provider>;
}

/** Moving or removing a layout slot never disposes its terminal generation. */
export function TerminalViewSlot({ streamKey, context }: {
  readonly streamKey: string;
  readonly context: TerminalPaneRenderContext;
}) {
  const pool = useContext(PoolContext);
  const slot = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (pool === null || slot.current === null) return;
    pool.attach(streamKey, context, slot.current);
  }, [pool, streamKey, context.terminalId, context.paneId, context.active]);
  useLayoutEffect(() => {
    const element = slot.current;
    return () => { if (element !== null) pool?.detach(streamKey, element); };
  }, [pool, streamKey]);
  return <div ref={slot} style={fill} />;
}

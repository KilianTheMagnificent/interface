import {
  TERMINAL_LAYOUT_DRAGGING, TERMINAL_LAYOUT_SLIDING, TERMINAL_PANE_LAYOUT_EVENT, TerminalFitThrottle,
} from "../terminal-pane-motion";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal, type ITerminalOptions } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { terminalAnsiPalette, terminalMinimumContrast } from "../terminal-palettes";
import {
  forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef,
  useState,
  type DragEvent as ReactDragEvent,
} from "react";
import {
  altScreenWheelCell, altScreenWheelLines, altScreenWheelSequence,
} from "../terminal-alt-scroll";
import {
  TERMINAL_PROGRAM_COPY_KEY, TERMINAL_REVERSE_TAB, terminalCenterKeyAction,
  terminalCopyGoesToProgram, terminalInputForKey, terminalOpensSearch,
  terminalReverseTabGoesToProgram,
} from "../terminal-keys";
import { installTerminalKittyKeyboard } from "../terminal-kitty-keyboard";
import { DropIndicator } from "./DropIndicator";
import { nextDropHoverCount, promptDropAccepts } from "../prompt-drop";
import { promptPasteAction, type PastedFile } from "../prompt-paste";
import { droppedTextAsPaste } from "../terminal-drop";
import { terminalControlScreen } from "../terminal-prompt";
import { installTerminalCursorPlacement, type CursorProfile } from "../terminal-cursor";
import { installTerminalCaret, type TerminalCaret } from "../terminal-caret";
import { limitTerminalAnnouncements } from "../terminal-announcements";
import { TerminalWebglRecovery, terminalWebglPool } from "../terminal-webgl-pool";
import { installCanvasFontGuard } from "../canvas-font";
import { cellSnapLetterSpacing, terminalAdvance } from "../terminal-cell";
import { terminalOutputPacer } from "../terminal-output-pacing";
import {
  TERMINAL_VIEW_ATTACHMENT_EVENT, TERMINAL_VIEW_COMMAND_EVENT, TERMINAL_VIEW_DETACHMENT_EVENT,
  TERMINAL_VIEW_PARKING_SELECTOR, type TerminalViewCommand,
} from "../terminal-view-attachment";
import { provideTerminalBufferLinks } from "../terminal-links";
import "../terminal.css";

export interface TerminalViewDrop {
  readonly files: readonly File[];
}

export interface TerminalViewHandle {
  write(data: string | Uint8Array, callback?: () => void): void;
  /** Allow geometry observation only after any buffered PTY bytes are restored. */
  completeSetup(): void;
  /** Fitted cell grid, or null before the emulator exists. */
  size(): { columns: number; rows: number } | null;
  focus(): void;
  fit(): void;
  clear(): void;
  getSelection(): string;
  copySelection(): boolean;
  paste(text: string): void;
  requestPaste(): Promise<boolean>;
  findNext(query: string): boolean;
  findPrevious(query: string): boolean;
  /** Visible text on the cursor's row, trailing blank cells stripped. */
  currentLine(): string;
  /** Every rendered line: full normal buffer, plus the visible alternate
      screen when a fullscreen TUI is active. Newest last. */
  readLines(): string[];
  /** The last `limit` non-blank lines of the active buffer, newest last. */
  tailLines(limit: number): string[];
  controlScreen(): ReturnType<typeof terminalControlScreen>;
}

export interface TerminalViewProps {
  readonly terminalId: string;
  readonly active: boolean;
  readonly cursorProfile?: CursorProfile;
  readonly working?: boolean;
  /** Geometry at which the PTY produced any output buffered before mount. */
  readonly initialSize?: { readonly columns: number; readonly rows: number };
  readonly options?: ITerminalOptions;
  readonly className?: string;
  readonly ariaLabel?: string;
  readonly onData: (data: string) => void;
  readonly onBinary?: (data: string) => void;
  readonly onResize?: (size: { columns: number; rows: number }) => void;
  readonly onTitleChange?: (title: string) => void;
  /** Clipboard authority remains with the caller; this component supplies selected text. */
  readonly onCopy?: (text: string) => void | Promise<void>;
  /** Observes text accepted by xterm from keyboard or the pane's Paste. */
  readonly onPaste?: (text: string) => void;
  /** Caller-owned clipboard read used by the pane menu's Paste. */
  readonly onPasteRequest?: () => string | null | Promise<string | null>;
  /** Pasted images. They carry no text, so xterm's own paste would drop them. */
  readonly onPasteFiles?: (files: readonly PastedFile[]) => void;
  readonly onDrop?: (drop: TerminalViewDrop) => void;
  readonly cwd?: string;
  readonly onOpenLink?: (link: string) => void;
  readonly onClear?: () => void;
  readonly onReady?: (handle: TerminalViewHandle | null) => void;
}

// xterm's WebGL renderer writes its glyph font with a leading space, which
// Chromium 150 refuses: without this every upright glyph is 10px sans-serif.
installCanvasFontGuard();

/** How far back a buffer preview looks for its last non-blank lines. */
export const TAIL_SCAN_ROWS = 200;

export const TERMINAL_SCREEN_READER_MODE = true;
/**
 * Rows are the font's own cell, as in the owner's Rex terminal (owner,
 * 2026-11-15; 1.2× from 2026-10-02 until then): MonoLisa at 11.5px is 16px a
 * row. xterm's custom glyphs (on by default) draw box drawing and block
 * elements across the whole cell, so the CLIs' frames stay joined.
 */
export const TERMINAL_LINE_HEIGHT = 1;

/** The size a pane reads before Settings has answered, and Rex's size. */
const TERMINAL_FONT_SIZE_FALLBACK = "11.5";

function cssValue(styles: CSSStyleDeclaration, name: string, fallback: string): string {
  const value = styles.getPropertyValue(name).trim();
  return value === "" ? fallback : value;
}

/** Search highlights are xterm decorations, a proposed API in xterm 6. */
function terminalRuntimeOptions(supplied: ITerminalOptions | undefined): ITerminalOptions {
  return { ...supplied, allowProposedApi: true, allowTransparency: true };
}

function interfaceOptions(host: HTMLElement, supplied: ITerminalOptions | undefined,
    initialSize: TerminalViewProps["initialSize"]): ITerminalOptions {
  const styles = getComputedStyle(host);
  const terminalBackground = cssValue(styles, "--terminal-content-bg",
    cssValue(styles, "--editor-bg", "#1e1e1e"));
  const glass = cssValue(styles, "--terminal-glass-active", "0") === "1";
  return {
    altClickMovesCursor: true,
    // A blinking cursor is a CSS animation on the DOM renderer, and it costs
    // 120-170 style recalculations a second for as long as a pane is visible.
    cursorBlink: false,
    // Drawn thin and short by terminal-caret.ts while the pane has focus.
    // Claude never sends DECSCUSR and Codex asks for the default, so this is
    // the cursor they show; a program asking for a block still gets one.
    cursorStyle: "bar",
    screenReaderMode: TERMINAL_SCREEN_READER_MODE,
    scrollback: 5_000,
    // xterm 6 and FitAddon share this width for the scrollbar gutter.
    overviewRuler: { width: 8 },
    ...terminalFont(styles),
    lineHeight: TERMINAL_LINE_HEIGHT,
    // Through glass xterm's background is transparent, which it measures as
    // black: a floor there would lighten text on a light screen.
    minimumContrastRatio: glass ? 1 : terminalMinimumContrast(styles),
    theme: {
      // The theme's own 16 colours; before 2026-09-28 every theme left xterm's.
      ...terminalAnsiPalette(document.documentElement.dataset.theme, styles),
      background: cssValue(styles, "--terminal-glass-active", "0") === "1" ? "#00000000" : terminalBackground,
      foreground: cssValue(styles, "--fg", "#d4d4d4"),
      cursor: cssValue(styles, "--selection-accent", "#4057b2"),
      cursorAccent: terminalBackground,
      selectionBackground: cssValue(styles, "--selection", "#264f78"),
      overviewRulerBorder: "#00000000",
    },
    ...(initialSize === undefined ? {} : {
      cols: initialSize.columns,
      rows: initialSize.rows,
    }),
    ...terminalRuntimeOptions(supplied),
  };
}

/**
 * The face, size, weight and cell width a pane draws in. The weight is the
 * theme's `--terminal-text-weight` (bold, as in the owner's Rex terminal), so
 * SGR bold has no heavier face to move to and stays the same weight.
 */
function terminalFont(styles: CSSStyleDeclaration):
    Required<Pick<ITerminalOptions, "fontFamily" | "fontSize" | "fontWeight" | "fontWeightBold" | "letterSpacing">> {
  const fontFamily = cssValue(styles, "--font-terminal",
    cssValue(styles, "--font-mono", "ui-monospace, monospace"));
  const fontSize = Number.parseFloat(
    cssValue(styles, "--terminal-reading-size", TERMINAL_FONT_SIZE_FALLBACK));
  const weight = Number.parseInt(cssValue(styles, "--terminal-text-weight", "400"), 10);
  const fontWeight = weight >= 100 && weight <= 900 ? weight : 400;
  return {
    fontFamily, fontSize, fontWeight,
    fontWeightBold: Math.max(fontWeight, 700),
    letterSpacing: cellSnapLetterSpacing(terminalAdvance(fontFamily, fontSize), window.devicePixelRatio),
  };
}

function safeTitle(value: string): string {
  return value.replace(/[\u0000-\u001f\u007f]/gu, " ").trim().slice(0, 256);
}

/** The members `terminalTailLines` reads off an xterm buffer. */
export interface TerminalTailBuffer {
  readonly length: number;
  getLine(y: number): {
    translateToString(trimRight: boolean): string;
    /** xterm wrapped this row on from the one above it. */
    readonly isWrapped?: boolean;
  } | undefined;
}

/**
 * The last `limit` non-blank rows of a buffer, oldest first. Walks BACKWARDS
 * from the end and stops after TAIL_SCAN_ROWS rows: a cleared alternate screen
 * is thousands of blank rows, and a forward scan would read the whole
 * scrollback to find the handful at the bottom that carry text.
 */
export function terminalTailLines(buffer: TerminalTailBuffer, limit: number): string[] {
  if (limit < 1) return [];
  const lines: string[] = [];
  const stopAt = Math.max(0, buffer.length - TAIL_SCAN_ROWS);
  for (let y = buffer.length - 1; y >= stopAt && lines.length < limit; y -= 1) {
    let text = buffer.getLine(y)?.translateToString(true) ?? "";
    // A row xterm wrapped onto continues the one above it: read them as one
    // line, the upper rows untrimmed so a space at the wrap point survives.
    while (buffer.getLine(y)?.isWrapped === true && y > stopAt) {
      y -= 1;
      text = (buffer.getLine(y)?.translateToString(false) ?? "") + text;
    }
    text = text.trimEnd();
    if (text !== "") lines.unshift(text);
  }
  return lines;
}

export function terminalHostCanFit(host: Pick<HTMLElement,
    "offsetParent" | "clientWidth" | "clientHeight"> | null): boolean {
  return host !== null && host.offsetParent !== null
    && host.clientWidth > 0 && host.clientHeight > 0;
}

/** Hosts whose last fit found them measurable, so a redraw is only owed after a hidden spell. */
const fittedHosts = new WeakSet<HTMLElement>();

function tryFit(addon: FitAddon | null, host: HTMLElement | null, terminal: Terminal | null): void {
  if (!terminalHostCanFit(host)) {
    if (host !== null) fittedHosts.delete(host);
    return;
  }
  const returning = host !== null && !fittedHosts.has(host);
  if (host !== null) fittedHosts.add(host);
  try { addon?.fit(); } catch { /* A later layout frame or observer retries the fit. */ }
  // FitAddon is a no-op when cols/rows did not change, but Chromium discards
  // canvas bitmaps while the workspace surface is display:none. Redraw on the
  // first fit and whenever the host was unmeasurable last time — every other
  // fit lands on a live canvas that xterm already keeps painted.
  if (returning && terminal !== null && terminal.rows > 0) {
    try { terminal.refresh(0, terminal.rows - 1); } catch { /* Same retry path as a failed fit. */ }
  }
}

export const TerminalView = forwardRef<TerminalViewHandle, TerminalViewProps>(
  function TerminalView({ terminalId, active, options, className, ariaLabel, onData, onBinary,
      initialSize, cursorProfile, working = false,
      onResize, onTitleChange, onCopy, onPaste, onPasteRequest, onPasteFiles, onDrop,
      cwd, onOpenLink, onClear, onReady }, forwardedRef) {
    const hostRef = useRef<HTMLDivElement>(null);
    const terminalRef = useRef<Terminal | null>(null);
    const cursorState = useRef({ profile: cursorProfile, enabled: active && !working });
    cursorState.current = { profile: cursorProfile, enabled: active && !working };
    const fitRef = useRef<FitAddon | null>(null);
    const fitReadyRef = useRef(false);
    const searchRef = useRef<SearchAddon | null>(null);
    const webglRef = useRef<{ slot(): void; park(): void; show(active: boolean): void;
      ensure(): void } | null>(null);
    const activeRef = useRef(active);
    activeRef.current = active;
    const caretRef = useRef<TerminalCaret | null>(null);
    const callbacks = useRef({
      onData, onBinary, onResize, onTitleChange, onCopy, onPaste,
      onPasteRequest, onPasteFiles, onDrop, cwd, onOpenLink, onClear, onReady,
    });
    const [searchOpen, setSearchOpen] = useState(false);
    const [searchQuery, setSearchQuery] = useState("");
    const [searchFound, setSearchFound] = useState<boolean | null>(null);
    const [hasSelection, setHasSelection] = useState(false);
    const [dropHover, setDropHover] = useState(0);
    const pendingControlWrites = useRef(0);
    const readControlScreen = useMemo(() => () => {
      const terminal = terminalRef.current;
      return terminal ? terminalControlScreen(terminal.buffer.active, terminal.rows,
        terminal.modes.bracketedPasteMode) : null;
    }, []);
    // A remounted terminal is not ready until every retained PTY chunk has
    // crossed xterm's asynchronous write buffer and geometry can be fitted.
    // Exposing that fact on the terminal section lets callers avoid switching
    // it away midway through restoration without coupling to xterm internals.
    const [ready, setReady] = useState(false);

    useLayoutEffect(() => {
      callbacks.current = {
        onData, onBinary, onResize, onTitleChange, onCopy, onPaste,
        onPasteRequest, onPasteFiles, onDrop, cwd, onOpenLink, onClear, onReady,
      };
    });

    const handle = useMemo<TerminalViewHandle>(() => ({
      write(data, callback) {
        const terminal = terminalRef.current;
        if (terminal === null) { callback?.(); return; }
        pendingControlWrites.current += 1;
        terminalOutputPacer().write(terminal, data, () => { pendingControlWrites.current -= 1; callback?.(); });
      },
      completeSetup() {
        fitReadyRef.current = true;
        setReady(true);
        tryFit(fitRef.current, hostRef.current, terminalRef.current);
      },
      size() {
        const terminal = terminalRef.current;
        if (terminal === null || terminal.cols < 1 || terminal.rows < 1) return null;
        return { columns: terminal.cols, rows: terminal.rows };
      },
      focus() { terminalRef.current?.focus(); },
      fit() {
        if (fitReadyRef.current) tryFit(fitRef.current, hostRef.current, terminalRef.current);
      },
      clear() {
        terminalRef.current?.clear();
        callbacks.current.onClear?.();
      },
      getSelection() { return terminalRef.current?.getSelection() || ""; },
      copySelection() {
        const selection = terminalRef.current?.getSelection() || "";
        if (selection === "" || callbacks.current.onCopy === undefined) return false;
        void callbacks.current.onCopy(selection);
        return true;
      },
      paste(text) {
        if (text === "") return;
        callbacks.current.onPaste?.(text);
        terminalRef.current?.paste(text);
      },
      async requestPaste() {
        const request = callbacks.current.onPasteRequest;
        if (request === undefined) return false;
        const text = await request();
        if (text === null || text === "") return false;
        callbacks.current.onPaste?.(text);
        terminalRef.current?.paste(text);
        return true;
      },
      findNext(query) { return searchRef.current?.findNext(query) ?? false; },
      findPrevious(query) { return searchRef.current?.findPrevious(query) ?? false; },
      currentLine() {
        const terminal = terminalRef.current;
        if (terminal === null) return "";
        const buffer = terminal.buffer.active;
        const line = buffer.getLine(buffer.baseY + buffer.cursorY);
        return line?.translateToString(true) ?? "";
      },
      controlScreen() { return fitReadyRef.current && pendingControlWrites.current === 0 ? readControlScreen() : null; },
      readLines() {
        const terminal = terminalRef.current;
        if (terminal === null) return [];
        const lines: string[] = [];
        const normal = terminal.buffer.normal;
        for (let y = 0; y < normal.length; y++) {
          lines.push(normal.getLine(y)?.translateToString(true) ?? "");
        }
        const active = terminal.buffer.active;
        if (active.type === "alternate") {
          for (let y = 0; y < active.length; y++) {
            lines.push(active.getLine(y)?.translateToString(true) ?? "");
          }
        }
        return lines;
      },
      tailLines(limit) {
        const terminal = terminalRef.current;
        if (terminal === null) return [];
        return terminalTailLines(terminal.buffer.active, limit);
      },
    }), []);
    useImperativeHandle(forwardedRef, () => handle, [handle]);

    useEffect(() => {
      const host = hostRef.current;
      if (host === null) return;
      fitReadyRef.current = false;
      // A view created out of view starts without screen-reader mode; see
      // syncReader below.
      const terminal = new Terminal({
        ...interfaceOptions(host, options, initialSize),
        linkHandler: {
          allowNonHttpProtocols: true,
          activate: (_event, text) => {
            callbacks.current.onOpenLink?.(text);
          },
        },
        screenReaderMode: TERMINAL_SCREEN_READER_MODE && activeRef.current });
      const fit = new FitAddon();
      const search = new SearchAddon({ highlightLimit: 1_000 });
      terminal.loadAddon(fit);
      terminal.loadAddon(search);
      terminal.open(host);
      const stopAnnouncements = limitTerminalAnnouncements(terminal.element!);
      // WebGL replaces the DOM renderer's per-row element rebuild, and it can
      // only be loaded once the terminal owns an element. A machine with no
      // usable GL context must still get a terminal, and a context lost later
      // must not blank one: disposing the addon drops xterm back to the DOM
      // renderer, which paints the same buffer and the same decorations.
      //
      // Retain recent workspaces' GPU renderers as well as their emulators.
      // Rebuilding every pane's shaders on every switch blocks the new frame.
      // The shared budget evicts old parked renderers, never visible ones, and
      // gives back a renderer left out of view past its idle window.
      let webgl: WebglAddon | null = null;
      let webglContext: WebGL2RenderingContext | null = null;
      let webglUnavailable = false;
      let slotted = true;
      // `active`: in a slot is not enough; a background tab or a hidden
      // surface is out of view too, and its renderer is parked like one.
      let shown = activeRef.current;
      let lostInSlot = false;
      // A context lost to sleep, a display change or a GPU process restart
      // must not leave a visible pane on the DOM renderer, whose text is
      // visibly thinner, until its next workspace switch. Reload it with a
      // bounded backoff; once that is spent, the next slot starts it again.
      const recovery = new TerminalWebglRecovery();
      let recovering = false;
      let reloadTimer: ReturnType<typeof setTimeout> | undefined;
      const webglOwner = {};
      const syncWebglPool = (): void => {
        if (slotted && shown) terminalWebglPool.slot(webglOwner);
        else terminalWebglPool.park(webglOwner);
      };
      const disposeWebgl = (): void => {
        const addon = webgl;
        const context = webglContext;
        webgl = null;
        webglContext = null;
        addon?.dispose();
        // Addon disposal removes the canvas but leaves its GL context alive
        // until GC. Return it explicitly before another pane claims the slot.
        context?.getExtension("WEBGL_lose_context")?.loseContext();
      };
      const scheduleReload = (): void => {
        const delay = recovery.lost(performance.now());
        if (delay === null) { lostInSlot = true; return; }
        clearTimeout(reloadTimer);
        reloadTimer = setTimeout(() => { reloadTimer = undefined; ensureWebgl(); }, delay);
      };
      const webglHooks = {
        // A hidden window gave back the renderer of a pane on screen.
        restore: () => { ensureWebgl(); },
        onScreen: () => (typeof host.checkVisibility === "function"
          ? host.checkVisibility({ visibilityProperty: true }) : terminalHostCanFit(host)),
      };
      const loadWebgl = (): void => {
        if (!terminalWebglPool.acquire(webglOwner, disposeWebgl, webglHooks)) {
          // A visible pane refused a slot waits for the next one to free up.
          terminalWebglPool.wait(webglOwner, () => { ensureWebgl(); });
          return;
        }
        let addon: WebglAddon | null = null;
        try {
          addon = new WebglAddon();
          addon.onContextLoss(() => {
            if (webgl !== addon) return;
            recovering = true;
            terminalWebglPool.release(webglOwner);
            scheduleReload();
          });
          terminal.loadAddon(addon);
          webgl = addon;
          recovering = false;
          // Loaded while out of view (under a dialog, say): idle like one.
          if (!shown) terminalWebglPool.park(webglOwner);
          // The link overlay also owns a (2D) canvas. Find the existing GL
          // context through public DOM APIs, without xterm private fields.
          webglContext = [...host.querySelectorAll("canvas")]
            .map(canvas => canvas.getContext("webgl2")).find(context => context !== null) ?? null;
        } catch {
          addon?.dispose();
          terminalWebglPool.release(webglOwner);
          // A reload can fail while the GPU process restarts, which spends
          // the retry budget. Only a first load failing means no WebGL here.
          if (recovering) scheduleReload();
          else webglUnavailable = true; // No WebGL here; the DOM renderer stays.
        }
      };
      const ensureWebgl = (): void => {
        if (webgl === null && slotted && !lostInSlot && !webglUnavailable
          && terminalHostCanFit(host)) loadWebgl();
      };
      // Screen-reader mode costs a string and an event for every printed
      // character, and a view out of view never renders, so nothing reads or
      // flushes what it would announce. A view out of view past the renderer's
      // idle window (or created out of view) turns it off; coming back into
      // view turns it on before that commit paints. A focused pane, and one
      // still painting under a dialog, keeps it.
      let cancelReaderIdle: (() => void) | null = null;
      const syncReader = (): void => {
        if (!TERMINAL_SCREEN_READER_MODE) return;
        if (slotted && shown) {
          cancelReaderIdle?.();
          cancelReaderIdle = null;
          if (terminal.options.screenReaderMode !== true) terminal.options.screenReaderMode = true;
          return;
        }
        if (cancelReaderIdle !== null || terminal.options.screenReaderMode !== true) return;
        const idle = (): void => {
          cancelReaderIdle = null;
          if (slotted && shown) return;
          if (host.contains(document.activeElement) || webglHooks.onScreen()) {
            cancelReaderIdle = terminalWebglPool.later(idle);
            return;
          }
          terminal.options.screenReaderMode = false;
        };
        cancelReaderIdle = terminalWebglPool.later(idle);
      };
      // Not in a hidden tab or the parking container: a renderer nobody can see
      // would only hold GPU memory until its idle window gave it back.
      ensureWebgl();
      webglRef.current = {
        slot() {
          slotted = true;
          lostInSlot = false;
          recovery.reset();
          syncWebglPool();
          syncReader();
        },
        park() {
          slotted = false;
          syncWebglPool();
          syncReader();
        },
        show(active) {
          shown = active;
          syncWebglPool();
          syncReader();
        },
        ensure: ensureWebgl,
      };
      terminalRef.current = terminal;
      fitRef.current = fit;
      searchRef.current = search;
      const kittyKeyboard = installTerminalKittyKeyboard(terminal);
      const disposables = [
        kittyKeyboard,
        terminal.onData(data => { callbacks.current.onData(data); }),
        terminal.onBinary(data => { callbacks.current.onBinary?.(data); }),
        terminal.onResize(({ cols, rows }) => {
          callbacks.current.onResize?.({ columns: cols, rows });
        }),
        terminal.onTitleChange(title => {
          const safe = safeTitle(title);
          if (safe !== "") callbacks.current.onTitleChange?.(safe);
        }),
        terminal.onSelectionChange(() => { setHasSelection(terminal.hasSelection()); }),
        terminal.registerLinkProvider({
          provideLinks(bufferLineNumber, callback) {
            callback(provideTerminalBufferLinks(
              terminal.buffer.active,
              bufferLineNumber,
              callbacks.current.cwd,
              callbacks.current.onOpenLink,
            ));
          },
        }),
      ];
      // xterm 6 swallows alt-screen trackpad wheel when consumeWheelEvent
      // floors to 0 lines; own every alt-screen wheel event and send the TUI
      // sequence ourselves, at xterm's line rate.
      const wheel = { partial: 0 };
      terminal.attachCustomWheelEventHandler(event => {
        if (terminal.buffer.active.type !== "alternate") return true;
        const host = terminal.element;
        const bounds = host === undefined
          ? { left: 0, top: 0, width: 0, height: 0 }
          : host.getBoundingClientRect();
        const lines = altScreenWheelLines(
          event, { rows: terminal.rows, cellHeight: bounds.height / Math.max(1, terminal.rows) },
          wheel,
        );
        event.preventDefault();
        if (lines === 0) return false;
        const sequence = altScreenWheelSequence({
          deltaY: lines,
          shiftKey: event.shiftKey, altKey: event.altKey, ctrlKey: event.ctrlKey,
        }, {
          bufferType: "alternate",
          mouseTrackingMode: terminal.modes.mouseTrackingMode,
          applicationCursorKeys: terminal.modes.applicationCursorKeysMode,
          cols: terminal.cols,
          rows: terminal.rows,
          ...altScreenWheelCell(
            event.clientX, event.clientY, bounds, terminal.cols, terminal.rows,
          ),
        });
        if (sequence !== null) terminal.input(sequence.repeat(Math.abs(lines)));
        return false;
      });
      terminal.attachCustomKeyEventHandler(event => {
        if (terminalReverseTabGoesToProgram(event)) {
          // Unconditional, unlike ⌘Backspace below: in screen-reader mode
          // xterm would leave this uncancelled and focus would leave the pane.
          event.preventDefault();
          event.stopPropagation();
          if (event.type === "keydown") terminal.input(TERMINAL_REVERSE_TAB);
          return false;
        }
        if (terminalCopyGoesToProgram(event, {
          hasSelection: terminal.hasSelection(),
          mouseTracking: terminal.modes.mouseTrackingMode !== "none",
          readsKeyReports: kittyKeyboard.readsKeyReports(),
        })) {
          event.preventDefault();
          event.stopPropagation();
          if (event.type === "keydown") terminal.input(TERMINAL_PROGRAM_COPY_KEY);
          return false;
        }
        const input = terminalInputForKey(event);
        if (input !== null) {
          // Screen-reader mode deliberately leaves the helper textarea's native
          // deletion intact so assistive technology can announce the edit.
          if (terminal.options.screenReaderMode !== true) event.preventDefault();
          event.stopPropagation();
          if (event.type === "keydown") terminal.input(input);
          return false;
        }
        if (terminalOpensSearch(event, /Mac/i.test(navigator.platform))) {
          if (event.type === "keydown") setSearchOpen(true);
          return false;
        }
        if (terminalCenterKeyAction(event) !== null) return false;
        return true;
      });
      const copy = (): void => {
        const selection = terminal.getSelection();
        if (selection !== "") void callbacks.current.onCopy?.(selection);
      };
      const paste = (event: ClipboardEvent): void => {
        const text = event.clipboardData?.getData("text/plain") ?? "";
        if (text !== "") {
          callbacks.current.onPaste?.(text);
          return;
        }
        // A screenshot is bytes with no text half, so xterm pastes nothing at
        // all. Same rule as the composers: text always wins, images otherwise.
        const receive = callbacks.current.onPasteFiles;
        const images = promptPasteAction(event.clipboardData, receive !== undefined);
        if (images === null || receive === undefined) return;
        event.preventDefault();
        receive(images);
      };
      const dragover = (event: DragEvent): void => {
        if (callbacks.current.onDrop === undefined) return;
        event.preventDefault();
        if (event.dataTransfer !== null) event.dataTransfer.dropEffect = "copy";
      };
      const drop = (event: DragEvent): void => {
        const receive = callbacks.current.onDrop;
        if (receive === undefined || event.dataTransfer === null) return;
        event.preventDefault();
        setDropHover(0);
        const files = Array.from(event.dataTransfer.files);
        // Dragged text can come from any web page. It arrives as a paste, so
        // a shell that asked for bracketed paste shows it and waits for
        // Return, and it never carries control characters of its own.
        const text = files.length === 0
          ? droppedTextAsPaste(event.dataTransfer.getData("text/plain")) : "";
        if (text !== "") {
          callbacks.current.onPaste?.(text);
          terminal.paste(text);
          return;
        }
        receive({ files });
      };
      // Under a dragged divider the observer's fits trail the pointer at most
      // every TERMINAL_DRAG_FIT_INTERVAL_MS; the release's layout event fits exactly.
      const dragFits = new TerminalFitThrottle(() => {
        if (!fitReadyRef.current) return;
        ensureWebgl();
        tryFit(fit, host, terminal);
      });
      let layoutEpoch = 0;
      const paneLayout = (): void => {
        dragFits.cancel();
        const epoch = ++layoutEpoch;
        // Coalesce slot and boundary notifications after this React commit,
        // before the browser paints the new canvas dimensions.
        queueMicrotask(() => {
          if (epoch !== layoutEpoch || !fitReadyRef.current || !terminalHostCanFit(host)) return;
          ensureWebgl();
          tryFit(fit, host, terminal);
          // Moving a persistent canvas between slots can discard its bitmap
          // without changing rows/columns. FitAddon alone then does no repaint.
          terminal.refresh(0, terminal.rows - 1);
        });
      };
      // The pane header's Find, and its ··· menu's Copy and Paste (owner,
      // 2026-11-15: nothing floats over the terminal's text any more).
      const command = (event: Event): void => {
        const name = (event as CustomEvent<TerminalViewCommand>).detail;
        if (name === "find") setSearchOpen(open => !open);
        else if (name === "copy") handle.copySelection();
        else if (name === "paste") void handle.requestPaste();
      };
      host.addEventListener(TERMINAL_VIEW_COMMAND_EVENT, command);
      host.addEventListener(TERMINAL_PANE_LAYOUT_EVENT, paneLayout);
      host.addEventListener("copy", copy);
      host.addEventListener("paste", paste);
      host.addEventListener("dragover", dragover);
      host.addEventListener("drop", drop);
      const disposeCursorPlacement = installTerminalCursorPlacement(terminal, () => cursorState.current);
      const caret = installTerminalCaret(terminal);
      caretRef.current = caret;
      const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => {
        // A sliding ancestor refits once it settles, via the pane layout event.
        if (!fitReadyRef.current || host.closest(`[${TERMINAL_LAYOUT_SLIDING}]`) !== null) return;
        if (host.closest(`[${TERMINAL_LAYOUT_DRAGGING}]`) !== null) {
          dragFits.request();
          return;
        }
        dragFits.cancel();
        ensureWebgl();
        tryFit(fit, host, terminal);
      });
      observer?.observe(host);
      // Replay is data setup, not paint work. requestAnimationFrame can be
      // suspended indefinitely while a packaged window is occluded, which
      // would leave a mounted xterm blank until the window became foreground.
      // The terminal is already open here; hidden fits safely no-op and the
      // ResizeObserver retries them when the host gains an area.
      let disposed = false;
      queueMicrotask(() => {
        if (disposed) return;
        const ready = callbacks.current.onReady;
        if (ready === undefined) handle.completeSetup();
        else ready(handle);
      });
      return () => {
        disposed = true;
        disposeCursorPlacement();
        caret.dispose();
        caretRef.current = null;
        stopAnnouncements();
        observer?.disconnect();
        dragFits.cancel();
        host.removeEventListener(TERMINAL_VIEW_COMMAND_EVENT, command);
        host.removeEventListener(TERMINAL_PANE_LAYOUT_EVENT, paneLayout);
        layoutEpoch++;
        host.removeEventListener("copy", copy);
        host.removeEventListener("paste", paste);
        host.removeEventListener("dragover", dragover);
        host.removeEventListener("drop", drop);
        for (const disposable of disposables) disposable.dispose();
        callbacks.current.onReady?.(null);
        webglRef.current = null;
        cancelReaderIdle?.();
        terminalOutputPacer().drop(terminal);
        clearTimeout(reloadTimer);
        terminalWebglPool.release(webglOwner);
        terminal.dispose();
        fitReadyRef.current = false;
        terminalRef.current = null;
        fitRef.current = null;
        searchRef.current = null;
      };
      // A terminal id denotes one process stream. Replacing it must dispose the
      // emulator rather than carrying buffer and input handlers into another id.
    }, [handle, terminalId]);

    useEffect(() => {
      if (terminalRef.current !== null && options !== undefined) {
        terminalRef.current.options = terminalRuntimeOptions(options);
      }
    }, [options]);

    useEffect(() => {
      const host = hostRef.current;
      const terminal = terminalRef.current;
      if (host === null || terminal === null) return;
      let pending: number | null = null;
      // Parked out of view, a pane sits outside the workspace and screen that
      // set its text size and colours: it would read the fallback size and the
      // window's text colour, and keep them when its workspace came back. It
      // keeps what it shows and repaints on its way back instead.
      let stale = false;
      const paint = (): void => {
        pending = null;
        stale = host.closest(TERMINAL_VIEW_PARKING_SELECTOR) !== null;
        if (stale) return;
        const next = interfaceOptions(host, options, undefined);
        if (caretRef.current === null) terminal.options.theme = next.theme;
        else caretRef.current.setTheme(next.theme ?? {});
        // Setting it clears xterm's glyph atlas, so only when the screen turned.
        if (terminal.options.minimumContrastRatio !== next.minimumContrastRatio) {
          terminal.options.minimumContrastRatio = next.minimumContrastRatio;
        }
        // Theme changes must preserve the emulator, PTY, selection and buffer.
        // Only font changes need another fit; a palette switch needs no resize.
        const metricsChanged = terminal.options.fontFamily !== next.fontFamily
          || terminal.options.fontSize !== next.fontSize
          || terminal.options.fontWeight !== next.fontWeight
          || terminal.options.letterSpacing !== next.letterSpacing;
        if (metricsChanged) {
          terminal.options.fontFamily = next.fontFamily;
          terminal.options.fontSize = next.fontSize;
          terminal.options.fontWeight = next.fontWeight;
          terminal.options.fontWeightBold = next.fontWeightBold;
          terminal.options.letterSpacing = next.letterSpacing;
          if (fitReadyRef.current) handle.fit();
        }
      };
      // Read inherited colors after the new palette has reached the host.
      // Coalesce rapid choices and cancel work when this emulator is disposed.
      const retheme = (): void => {
        if (pending === null) pending = window.requestAnimationFrame(paint);
      };
      // Synchronously, so the return's fit already measures the new cells.
      const returned = (): void => { if (stale) paint(); };
      document.documentElement.addEventListener("interface-theme-change", retheme);
      const transparencyMedia = window.matchMedia("(prefers-reduced-transparency: reduce)");
      transparencyMedia.addEventListener("change", retheme);
      host.addEventListener(TERMINAL_VIEW_ATTACHMENT_EVENT, returned);
      // Reconcile after an options update cancels the previous pending frame.
      retheme();
      return () => {
        document.documentElement.removeEventListener("interface-theme-change", retheme);
        transparencyMedia.removeEventListener("change", retheme);
        host.removeEventListener(TERMINAL_VIEW_ATTACHMENT_EVENT, returned);
        if (pending !== null) window.cancelAnimationFrame(pending);
      };
    }, [handle, options, terminalId]);

    useLayoutEffect(() => {
      const host = hostRef.current;
      if (host === null) return;
      let epoch = 0;
      const attached = (): void => {
        const expected = ++epoch;
        webglRef.current?.slot();
        queueMicrotask(() => {
          const terminal = terminalRef.current;
          if (expected !== epoch || !fitReadyRef.current || terminal === null || !terminalHostCanFit(host)) return;
          // Before the fit and repaint, so the slot's first frame is WebGL's.
          webglRef.current?.ensure();
          tryFit(fitRef.current, host, terminal);
          // Reparenting can discard a canvas without changing its cell grid.
          terminal.refresh(0, terminal.rows - 1);
        });
      };
      const parked = (): void => {
        epoch++;
        webglRef.current?.park();
      };
      host.addEventListener(TERMINAL_VIEW_ATTACHMENT_EVENT, attached);
      host.addEventListener(TERMINAL_VIEW_DETACHMENT_EVENT, parked);
      return () => {
        epoch++;
        host.removeEventListener(TERMINAL_VIEW_ATTACHMENT_EVENT, attached);
        host.removeEventListener(TERMINAL_VIEW_DETACHMENT_EVENT, parked);
      };
    }, [terminalId]);

    useLayoutEffect(() => {
      webglRef.current?.show(active);
      if (!active) return;
      // A renderer given back while out of view reloads before this commit
      // paints, so a returning tab or surface never shows the DOM renderer.
      webglRef.current?.ensure();
      const frame = window.requestAnimationFrame(() => {
        handle.fit();
      });
      return () => { window.cancelAnimationFrame(frame); };
    }, [active, handle]);

    useEffect(() => {
      const search = searchRef.current;
      if (!searchOpen || search === null) return;
      if (searchQuery === "") {
        search.clearDecorations();
        setSearchFound(null);
        return;
      }
      setSearchFound(search.findNext(searchQuery, { incremental: true }));
    }, [searchOpen, searchQuery]);

    const find = (direction: "next" | "previous"): void => {
      const search = searchRef.current;
      if (search === null || searchQuery === "") return;
      setSearchFound(direction === "next"
        ? search.findNext(searchQuery) : search.findPrevious(searchQuery));
    };
    const closeSearch = (): void => {
      setSearchOpen(false);
      setSearchFound(null);
      searchRef.current?.clearDecorations();
      terminalRef.current?.focus();
    };

    useEffect(() => {
      if (dropHover === 0) return undefined;
      const clear = (): void => { setDropHover(0); };
      window.addEventListener("dragend", clear, { once: true });
      window.addEventListener("drop", clear, { once: true });
      return () => {
        window.removeEventListener("dragend", clear);
        window.removeEventListener("drop", clear);
      };
    }, [dropHover]);

    const acceptAttachmentDrag = (event: ReactDragEvent<HTMLElement>): boolean => {
      if (callbacks.current.onDrop === undefined || event.dataTransfer === null) return false;
      return promptDropAccepts(Array.from(event.dataTransfer.types));
    };

    return (
      <section className={`terminal-view${active ? " is-active" : ""}${
        dropHover > 0 ? " is-drop-target" : ""}${searchOpen ? " has-search" : ""}${
        className === undefined ? "" : ` ${className}`}`}
        data-terminal-id={terminalId}
        data-terminal-ready={ready ? "" : undefined}
        data-terminal-selection={hasSelection && onCopy !== undefined ? "" : undefined}
        aria-label={ariaLabel ?? "Interactive terminal"}
        onDragEnter={event => {
          if (!acceptAttachmentDrag(event)) return;
          event.preventDefault();
          setDropHover(count => nextDropHoverCount(count, "dragenter"));
        }}
        onDragOver={event => {
          if (callbacks.current.onDrop === undefined) return;
          event.preventDefault();
          if (event.dataTransfer !== null) event.dataTransfer.dropEffect = "copy";
        }}
        onDragLeave={event => {
          if (!acceptAttachmentDrag(event)) return;
          setDropHover(count => nextDropHoverCount(count, "dragleave"));
        }}
        onDragEnd={() => { setDropHover(0); }}
        onDrop={event => {
          if (callbacks.current.onDrop === undefined) return;
          event.preventDefault();
          setDropHover(0);
        }}>
        {searchOpen && (
          /* A row above the text, never over it: the screen gives it its
             height while it is open. */
          <div className="terminal-search" role="search">
            <label className="terminal-sr-only" htmlFor={`${terminalId}-terminal-search`}>
              Search terminal output
            </label>
            <input id={`${terminalId}-terminal-search`} value={searchQuery} autoFocus
              placeholder="Find" aria-label="Search terminal output"
              onChange={event => { setSearchQuery(event.currentTarget.value); }}
              onKeyDown={event => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  closeSearch();
                } else if (event.key === "Enter") {
                  event.preventDefault();
                  find(event.shiftKey ? "previous" : "next");
                }
              }} />
            <span className="terminal-search-result" role="status">
              {searchFound === false ? "No match" : searchFound === true ? "Match" : ""}
            </span>
            <button type="button" aria-label="Previous match" title="Previous match"
              disabled={searchQuery === ""} onClick={() => { find("previous"); }}>
              <span className="codicon codicon-chevron-up" aria-hidden="true" />
            </button>
            <button type="button" aria-label="Next match" title="Next match"
              disabled={searchQuery === ""} onClick={() => { find("next"); }}>
              <span className="codicon codicon-chevron-down" aria-hidden="true" />
            </button>
            <button type="button" aria-label="Close terminal search" title="Close"
              onClick={closeSearch}>
              <span className="codicon codicon-close" aria-hidden="true" />
            </button>
          </div>
        )}
        <div ref={hostRef} className="terminal-xterm" />
        {dropHover > 0 && (
          <DropIndicator label="Drop to insert path" />
        )}
      </section>
    );
  },
);

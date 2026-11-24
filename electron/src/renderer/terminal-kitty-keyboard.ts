import type { IDisposable, Terminal } from "@xterm/xterm";

export type TerminalScreen = "normal" | "alternate";

/** Kitty's "disambiguate escape codes" flag: the program reads CSI u key reports. */
const KITTY_DISAMBIGUATE = 1;
/** The five flags the protocol defines. */
const KITTY_FLAGS = 0b11111;
const MAX_KITTY_STACK = 16;

/**
 * The kitty keyboard flags a program has pushed, one stack per screen as the
 * protocol specifies. xterm 6 does not implement the protocol and never
 * encodes a key this way itself; Interface only needs to know whether the
 * program in front can read a CSI u report before it sends one.
 */
export class KittyKeyboardStacks {
  private readonly stacks: Record<TerminalScreen, number[]> = { normal: [], alternate: [] };

  /**
   * `CSI > flags u` pushes, `CSI < count u` pops, `CSI = flags ; mode u`
   * replaces (1), adds (2) or removes (3) flags on the top entry. An omitted
   * or zero count or mode takes its default, as xterm reports both as 0.
   */
  apply(screen: TerminalScreen, prefix: string, values: readonly number[]): void {
    const stack = this.stacks[screen];
    const flags = (values[0] ?? 0) & KITTY_FLAGS;
    if (prefix === ">") {
      stack.push(flags);
      if (stack.length > MAX_KITTY_STACK) stack.shift();
    } else if (prefix === "<") {
      stack.length = Math.max(0, stack.length - Math.max(1, values[0] ?? 1));
    } else if (prefix === "=") {
      const current = stack.at(-1) ?? 0;
      const mode = values[1] ?? 1;
      const next = mode === 2 ? current | flags : mode === 3 ? current & ~flags : flags;
      if (stack.length === 0) stack.push(next);
      else stack[stack.length - 1] = next;
    }
  }

  /** A program entering the alternate screen does not inherit a stale stack. */
  enterAlternate(): void { this.stacks.alternate.length = 0; }

  reset(): void {
    this.stacks.normal.length = 0;
    this.stacks.alternate.length = 0;
  }

  readsKeyReports(screen: TerminalScreen): boolean {
    return ((this.stacks[screen].at(-1) ?? 0) & KITTY_DISAMBIGUATE) !== 0;
  }

  /** The pushes that rebuild one screen's stack in a fresh emulator. */
  restoreSequence(screen: TerminalScreen): string {
    return this.stacks[screen].map(flags => `\x1b[>${flags}u`).join("");
  }
}

export interface TerminalKittyKeyboard extends IDisposable {
  /** Whether the program on the active screen asked for kitty key reports. */
  readsKeyReports(): boolean;
}

/**
 * Follow the kitty keyboard requests xterm itself ignores. Parser hooks run
 * before xterm's own handlers and return false, so its behavior is unchanged.
 */
export function installTerminalKittyKeyboard(terminal: Terminal): TerminalKittyKeyboard {
  const stacks = new KittyKeyboardStacks();
  const screen = (): TerminalScreen =>
    terminal.buffer.active.type === "alternate" ? "alternate" : "normal";
  const parser = terminal.parser;
  const disposables = (["<", "=", ">"] as const).map(prefix =>
    parser.registerCsiHandler({ prefix, final: "u" }, params => {
      stacks.apply(screen(), prefix,
        params.map(param => typeof param === "number" ? param : param[0] ?? 0));
      return false;
    }));
  disposables.push(
    parser.registerEscHandler({ final: "c" }, () => { stacks.reset(); return false; }),
    terminal.buffer.onBufferChange(buffer => {
      if (buffer.type === "alternate") stacks.enterAlternate();
    }),
  );
  return {
    readsKeyReports: () => stacks.readsKeyReports(screen()),
    dispose() { for (const disposable of disposables) disposable.dispose(); },
  };
}

/**
 * Process-local readiness fence for the private terminal MessagePort.
 *
 * A random token binds one preload acknowledgement to one document transfer.
 * Sender/frame authorization remains an Electron-main responsibility; this
 * class prevents a late acknowledgement from an earlier document from making
 * the current document appear ready.
 */
export class TerminalDataBridgeGate {
  #pendingToken: string | null = null;
  #ready = false;
  #failure: string | null = null;

  get isReady(): boolean {
    return this.#ready;
  }

  get failure(): string | null {
    return this.#failure;
  }

  /** Begin one transfer. A failed document cannot silently recover in place. */
  begin(token: string): boolean {
    if (this.#failure !== null || token === "") return false;
    this.#pendingToken = token;
    this.#ready = false;
    return true;
  }

  /** Accept only the exact token minted for the current document transfer. */
  acknowledge(token: string): boolean {
    if (this.#failure !== null || this.#pendingToken === null
        || token !== this.#pendingToken) return false;
    this.#pendingToken = null;
    this.#ready = true;
    return true;
  }

  /** Preserve the first/root failure; later derived errors cannot replace it. */
  fail(message: string): string {
    const normalized = message.trim();
    if (this.#failure === null) {
      this.#failure = normalized === ""
        ? "The integrated terminal data channel failed."
        : normalized;
    }
    this.#pendingToken = null;
    this.#ready = false;
    return this.#failure;
  }

  /** A full main-frame navigation creates a fresh preload and handshake. */
  resetDocument(): void {
    this.#pendingToken = null;
    this.#ready = false;
    this.#failure = null;
  }

  /** Refuse process creation until the current preload owns and acknowledges its port. */
  requireReady(): void {
    if (this.#ready) return;
    throw new Error(this.#failure
      ?? "The integrated terminal data channel is still connecting. Interface did not start a terminal.");
  }
}

export interface PreventableWindowCloseEvent {
  preventDefault(): void;
}

export interface WindowLifecycleEffects {
  isQuitting(): boolean;
  setQuitting(value: boolean): void;
  isShutdownStarted(): boolean;
  isWindowDestroyed(): boolean;
  shouldHideOnClose(): boolean;
  hideWindow(): void;
  defer(task: () => void): void;
  closeWindow(): void;
  beginShutdown(): void;
}

export interface WindowLifecycleCoordinator {
  onCloseRequested(event: PreventableWindowCloseEvent): void;
  onUnloadCancelled(): void;
  onWebContentsDestroyed(): void;
  onWindowClosed(): void;
}

/**
 * Coordinates background hiding and the native close retry without starting
 * backend shutdown before the renderer can veto an unload with unsaved work.
 */
export function createWindowLifecycleCoordinator(
  effects: WindowLifecycleEffects,
): WindowLifecycleCoordinator {
  let pendingRetry: object | null = null;

  const requestShutdown = (markQuitting: boolean): void => {
    if (markQuitting) effects.setQuitting(true);
    if (!effects.isShutdownStarted()) effects.beginShutdown();
  };

  return {
    onCloseRequested(event) {
      if (!effects.isQuitting() && effects.shouldHideOnClose()) {
        event.preventDefault();
        effects.hideWindow();
        return;
      }
      // Once quitting is set, the requested close is allowed through to the
      // durable renderer-unload and backend-shutdown path.
      if (effects.isQuitting()) return;
      event.preventDefault();
      if (pendingRetry !== null) return;

      const retry = {};
      pendingRetry = retry;
      effects.defer(() => {
        if (pendingRetry !== retry) return;
        pendingRetry = null;
        if (effects.isWindowDestroyed()) {
          requestShutdown(true);
          return;
        }
        effects.setQuitting(true);
        effects.closeWindow();
      });
    },
    onUnloadCancelled() {
      // Invalidating the token also cancels an overlapping deferred retry.
      pendingRetry = null;
      effects.setQuitting(false);
    },
    onWebContentsDestroyed() {
      requestShutdown(true);
    },
    onWindowClosed() {
      requestShutdown(false);
    },
  };
}

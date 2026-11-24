import { describe, expect, it } from 'vitest';
import fixture from '../fixtures/terminal-controls-v13.json';
import { CHANNELS, type TerminalControlsRequestV1, type TerminalControlsStateV1 } from './ipc';
import { encodeRendererPayload } from '../main/ipc-payload';
import { RUST_HOST_PROTOCOL_VERSION } from '../main/rust-host-client';

describe('native terminal settings protocol v13', () => {
  it('sends a versioned generation-specific selection and renders only confirmed values', () => {
    const request: TerminalControlsRequestV1 = { version: 1, id: fixture.request.id, generation: 2,
      screen: fixture.request.screen, selection: { kind: 'effort', value: 'xhi' } };
    const state: TerminalControlsStateV1 = { version: 1, model: 'gpt-6-astra', effort: 'high',
      ready: true, pending: true, error: null };
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixture.protocolVersion);
    expect(encodeRendererPayload(CHANNELS.advanceTerminalControls, [request])).toEqual(request);
    expect(request).toEqual(fixture.request);
    expect(state).toEqual(fixture.state);
  });
});

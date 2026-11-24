import { describe, expect, it } from 'vitest';
import fixture from '../fixtures/harness-v20.json';
import { CHANNELS } from './ipc';
import type { HarnessCommand, HarnessState, TerminalServerLink } from './harness';
import { encodeRendererPayload } from '../main/ipc-payload';
describe('harness v20 boundary', () => {
  it('encodes the shared Rust commands without changing field identity', () => {
    for (const command of fixture.commands) expect(encodeRendererPayload(CHANNELS.harnessControl, [command as HarnessCommand])).toEqual(command);
    expect(encodeRendererPayload(CHANNELS.inspectTerminalServers, [fixture.serverRequest])).toEqual(fixture.serverRequest);
    const state = fixture.state as HarnessState;
    const links = fixture.serverLinks as TerminalServerLink[];
    expect(state.attention[0].target).toEqual({ kind: 'terminal', terminalId: fixture.serverRequest.id, generation: 2 });
    expect(links[0].readiness).toContain('not verified');
  });
});

import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import test from 'node:test';

test('the architecture facts match the host handshake constants', () => {
  const electron = readFileSync(new URL('../src/main/rust-host-client.ts', import.meta.url), 'utf8');
  const rust = readFileSync(new URL('../../apps/interface-host/src/protocol.rs', import.meta.url), 'utf8');
  const facts = readFileSync(new URL('../../docs/PROTOCOL_FACTS.md', import.meta.url), 'utf8');
  const terminal = readFileSync(new URL('../src/main/terminal-data-transport.ts', import.meta.url), 'utf8');
  const rustTerminal = readFileSync(new URL('../../apps/interface-host/src/terminal_transport.rs', import.meta.url), 'utf8');
  const constant = name => Number(electron.match(new RegExp(`export const ${name} = (\\d+);`))?.[1]);
  const version = constant('RUST_HOST_PROTOCOL_VERSION');
  const requests = constant('RUST_HOST_REQUEST_OPERATIONS');
  const pushes = constant('RUST_HOST_PUSH_OPERATIONS');
  const terminalVersion = Number(terminal.match(/export const TERMINAL_DATA_VERSION = (\d+);/)?.[1]);
  assert.equal(version, Number(rust.match(/pub const PROTOCOL_VERSION: u8 = (\d+);/)?.[1]));
  assert.equal(terminalVersion, Number(rustTerminal.match(/pub const TERMINAL_DATA_VERSION: u8 = (\d+);/)?.[1]));
  for (const [label, value] of [['Host protocol version', version], ['Request operations', requests],
    ['Push operations', pushes], ['Desktop channels', requests + pushes], ['Terminal data protocol', terminalVersion]]) {
    assert(Number.isInteger(value) && value > 0, label);
    assert(facts.includes(`| ${label} | ${value} |`), `${label} documentation drifted`);
  }
});

import { expect, it } from "vitest";
import fixture from "../fixtures/terminal-delegation-v12.json";
import type { ReadTerminalDelegation, TerminalDelegationView } from "./delegation";
import { CHANNELS } from "./ipc";
import { RUST_HOST_PROTOCOL_VERSION } from "../main/rust-host-client";

it("matches the independently deserialized Rust v12 delegation request and projection", () => {
  const request: ReadTerminalDelegation = fixture.request;
  const view = { ...fixture.view, version: 3 } as TerminalDelegationView;
  expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixture.protocolVersion);
  expect(CHANNELS.readTerminalDelegation).toBe(fixture.channel);
  expect(request).toEqual(fixture.request);
  expect(view).toEqual(fixture.view);
  expect(view.workflows[0].tasks[0].terminal?.launch?.profileId).toBe("opencode");
  expect(view.workflows[0].tasks[0].usage?.costUsd).toBeNull();
  expect(view.detail?.result?.changes[0].before).toBe("Before\n");
});

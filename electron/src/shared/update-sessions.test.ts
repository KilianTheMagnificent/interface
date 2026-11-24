import { describe, expect, it } from "vitest";
import fixture from "../fixtures/update-sessions-v52.json";
import { isUpdateSessionsView } from "./update-sessions";

describe("cooperative update protocol", () => {
  it("matches the shared protocol 52 checkpoint fixture and rejects invalid progress", () => {
    expect(isUpdateSessionsView(fixture.view)).toBe(true);
    expect(isUpdateSessionsView({ ...fixture.view, phase: "forced" })).toBe(false);
    expect(isUpdateSessionsView({ ...fixture.view, sessions: [{ id: "one", phase: "ready" }] })).toBe(false);
  });
});

import { describe, expect, it } from "vitest";
import type { EngineEvent } from "../shared/engine";
import { ENGINE_CHOICES, UNKNOWN_ENGINE_CHOICES, engineChoices } from "./engine-choices";

describe("engineChoices", () => {
  it("offers the cold-start table before an engine reports, and nothing for an unknown one", () => {
    expect(engineChoices("codex", [], {})).toMatchObject({
      models: [], permissionModes: ENGINE_CHOICES.codex.permissionModes });
    expect(engineChoices("acp:grok", [], {}).models).toEqual(ENGINE_CHOICES["acp:grok"].models);
    expect(engineChoices("someone-new", [], {})).toMatchObject(UNKNOWN_ENGINE_CHOICES);
  });

  it("prefers a catalogue a live session reported, and ignores streamed text", () => {
    const catalogue: EngineEvent = { kind: "models.available", models: [
      { value: "codex-fast", displayName: "Codex Fast", supportsEffort: true, effortLevels: ["low", "high"] }] };
    const before = { "codex#a": [catalogue] };
    const after = { "codex#a": [catalogue, { kind: "message.delta", text: "streamed" } as EngineEvent] };
    const choices = engineChoices("codex", [], before);
    expect(choices.models).toEqual(["codex-fast"]);
    expect(choices.efforts).toEqual(["low", "high"]);
    expect(engineChoices("codex", [], after)).toEqual(choices);
    // Another engine's sessions are not this engine's catalogue.
    expect(engineChoices("claude", [], before).models).toEqual(ENGINE_CHOICES.claude.models);
  });
});

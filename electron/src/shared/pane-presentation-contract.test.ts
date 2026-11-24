import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { isPanePresentation } from "./appearance";

describe("pane presentation v1 on appearance surface v2", () => {
  it("agrees with the Rust contract on valid and invalid requests", () => {
    const fixture = JSON.parse(readFileSync(new URL("../../../docs/fixtures/pane-presentation-v1.json", import.meta.url), "utf8"));
    for (const value of fixture.accepted) expect(isPanePresentation(value)).toBe(true);
    for (const value of fixture.rejected) expect(isPanePresentation(value)).toBe(false);
  });
});

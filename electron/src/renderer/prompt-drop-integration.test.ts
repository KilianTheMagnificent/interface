import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const main = readFileSync(new URL("./main.tsx", import.meta.url), "utf8");

describe("chat file drop wiring", () => {
  it("grants dropped paths before attaching them to launch or live composers", () => {
    expect(app).toContain("window.iface.grantPromptContextPaths");
    expect(app).toContain("window.iface.pathForFile");
    expect(app).toContain("onDroppedPaths");
    expect(app).toMatch(/grantPromptContextPaths\([\s\S]*?addContextFor/);
  });

  it("stops the window from navigating when files are dropped outside the composer", () => {
    expect(main).toContain('addEventListener("dragover"');
    expect(main).toContain('addEventListener("drop"');
    expect(main).toContain("promptDropAccepts");
    expect(main).toContain("preventDefault");
  });
});

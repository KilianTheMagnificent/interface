import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const terminalCenterSource = readFileSync(
  new URL("./components/IntegratedTerminalCenter.tsx", import.meta.url), "utf8",
);

describe("truncated agent redraw integration", () => {
  it("observes every delivered PTY output event for the active pulse", () => {
    const acceptData = terminalCenterSource.slice(
      terminalCenterSource.indexOf("const acceptData = useCallback"),
      terminalCenterSource.indexOf("useEffect(() => {", terminalCenterSource.indexOf(
        "const acceptData = useCallback",
      )),
    );
    // The router buffers and writes each chunk (terminal-stream-router.test);
    // everything it routes reaches the pulse with its parser boundary.
    expect(acceptData).toMatch(
      /streams\.deliver\(event, terminalById\(event\.terminalId\), complete\);[\s\S]*?redrawPulse\.noteOutput\(output\.key, event\.data, output\.boundary\)/u,
    );
  });

  it("routes fitted geometry into an active redraw pulse", () => {
    const scheduleResize = terminalCenterSource.slice(
      terminalCenterSource.indexOf("const scheduleResize = useCallback"),
      terminalCenterSource.indexOf("const kickRedraw = useCallback"),
    );
    expect(scheduleResize).toContain("if (redrawPulse.updateTarget(key, size)) return;");
    expect(scheduleResize.indexOf("redrawPulse.updateTarget"))
      .toBeLessThan(scheduleResize.indexOf("resizeQueue.schedule"));
  });

  it("cancels routine geometry before starting the separated resize pulse", () => {
    const kickRedraw = terminalCenterSource.slice(
      terminalCenterSource.indexOf("const kickRedraw = useCallback"),
      terminalCenterSource.indexOf("const insertPaths = useCallback"),
    );
    expect(kickRedraw).toContain("resizeQueue.cancelAndWait(key);");
    // OpenCode typed into a shell pane is recognised by its own resize query.
    expect(kickRedraw).toContain(
      'terminal.launch?.profileId === "opencode" || streams.queriedPixelSize(key)\n'
      + '        ? "opentui-resize" : "output"');
    expect(kickRedraw.indexOf("resizeQueue.cancelAndWait(key)"))
      .toBeLessThan(kickRedraw.indexOf("redrawPulse.start"));
  });

  it("fences late redraw work when a terminal view unmounts", () => {
    expect(terminalCenterSource).toMatch(
      /if \(handle === null\) \{[\s\S]*?resizeQueue\.cancel\(key\);[\s\S]*?redrawPulse\.cancel\(key\);/u,
    );
  });
});

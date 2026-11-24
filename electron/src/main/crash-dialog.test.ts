import { describe, expect, it } from "vitest";
import { chooseRelaunch, crashDialog } from "./crash-dialog";

describe("crash alerts offer Relaunch (review D2, 2026-10-14)", () => {
  it("makes Relaunch the default answer when the backend stops", () => {
    const alert = crashDialog("backend", "Interface", "The host exited with code 101.", true);
    expect(alert.options.message).toBe("Interface backend stopped");
    expect(alert.options.buttons).toEqual(["Relaunch", "OK"]);
    expect(alert.options.defaultId).toBe(0);
    expect(alert.options.cancelId).toBe(1);
    expect(alert.options.detail).toContain("The host exited with code 101.");
    expect(alert.options.detail).toContain("Relaunch Interface to recover.");
    expect(chooseRelaunch(alert, 0)).toBe(true);
    // OK, Escape and a closed alert keep the window as it was.
    expect(chooseRelaunch(alert, 1)).toBe(false);
  });

  it("offers Relaunch or Quit when the window stops", () => {
    const alert = crashDialog("window", "Interface Dev", "", true);
    expect(alert.options.message).toBe("Interface Dev window stopped");
    expect(alert.options.buttons).toEqual(["Relaunch", "Quit"]);
    expect(chooseRelaunch(alert, 0)).toBe(true);
    expect(chooseRelaunch(alert, 1)).toBe(false);
  });

  it("never relaunches for a harness that answers every dialog with 0", () => {
    for (const kind of ["backend", "window"] as const) {
      const alert = crashDialog(kind, "Interface", "gone", false);
      expect(alert.options.buttons).toEqual(["OK"]);
      expect(alert.relaunch).toBeUndefined();
      expect(chooseRelaunch(alert, 0)).toBe(false);
    }
    expect(crashDialog("backend", "Interface", "gone", false).options.detail)
      .toBe("gone\n\nQuit and reopen Interface to recover.");
  });
});

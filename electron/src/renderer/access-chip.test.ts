import { describe, expect, it } from "vitest";
import { accessSelectOptions } from "./access-chip";

const MODES = ["default", "acceptEdits", "plan", "bypassPermissions"];

describe("the access picker's rows", () => {
  it("keeps the pane header's CLI status wording on the closed trigger", () => {
    const rows = accessSelectOptions(MODES, "acceptEdits", false);
    expect(rows.map(row => row.value)).toEqual(MODES);
    expect(rows.find(row => row.value === "acceptEdits")?.triggerLabel)
      .toBe("›› accept edits on");
    expect(rows.find(row => row.value === "bypassPermissions")?.triggerLabel)
      .toBe("! bypass on");
  });

  it("lets the Threads chip say the row's own short label instead", () => {
    const rows = accessSelectOptions(MODES, "acceptEdits", false, "label");
    expect(rows.every(row => row.triggerLabel === undefined)).toBe(true);
    expect(rows.find(row => row.value === "acceptEdits")?.label).toBe("Accept edits");
    // The provider's exact token is still what a choice sends.
    expect(rows.map(row => row.value)).toEqual(MODES);
  });

  it("marks an inherited policy as current and not re-pickable", () => {
    const rows = accessSelectOptions(MODES, "plan", true, "label");
    const plan = rows.find(row => row.value === "plan");
    expect(plan).toMatchObject({ disabled: true, meta: "Current" });
    expect(rows).toHaveLength(MODES.length);
  });

  it("puts an inherited policy the provider did not list first, so the trigger never lies", () => {
    const labelled = accessSelectOptions(MODES, "dontAsk", true, "label");
    expect(labelled[0]).toMatchObject({
      value: "dontAsk", label: "Deny unapproved", disabled: true, meta: "Current",
    });
    // The header keeps its old wording for the same row.
    const status = accessSelectOptions(MODES, "dontAsk", true);
    expect(status[0]).toMatchObject({ value: "dontAsk", label: "dontAsk",
      triggerLabel: "! don't ask" });
    expect(accessSelectOptions(MODES, "", true, "label")[0].label).toBe("Unknown policy");
  });
});

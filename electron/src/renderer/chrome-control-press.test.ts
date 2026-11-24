import { createElement } from "react";
import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

/* renderToStaticMarkup never fires an onClick, so a markup assertion alone
   cannot tell a wired control from a decorative one. The picker is replaced
   with a recorder that keeps the exact props the real AppSelect would receive,
   which makes the callback the menu would invoke on selection reachable — and
   pressing it is what proves the control reaches the preference writer. */

interface RecordedSelect {
  label: string;
  value: string;
  options: readonly { value: string; label: string; description?: string }[];
  onChange: (value: string) => void;
}

const recorded: RecordedSelect[] = [];

vi.mock("./components/AppSelect", () => ({
  AppSelect: (props: RecordedSelect): ReactElement => {
    recorded.push(props);
    return createElement("span", { "data-app-select": props.label }, props.value);
  },
}));

const { SettingsPage } = await import("./components/SettingsPage");
const { DEFAULT_APP_PREFERENCES } = await import("./settings");
type SettingsPageProps = Parameters<typeof SettingsPage>[0];

const props = (overrides: Partial<SettingsPageProps> = {}): SettingsPageProps => ({
  appearance: { preference: "system", resolved: "dark" },
  onAppearanceChange: vi.fn(),
  preferences: { ...DEFAULT_APP_PREFERENCES },
  onPreferencesChange: vi.fn(),
  engineHealth: [],
  onRefreshProviders: vi.fn(),
  customizationApi: {
    getDefaultDirectory: vi.fn(async () => "/work"),
    listCustomization: vi.fn(async (root: string) => ({ root, entries: [], truncated: false })),
  },
  onClose: vi.fn(),
  ...overrides,
});

function chromePicker(overrides: Partial<SettingsPageProps> = {}): {
  picker: RecordedSelect;
  onPreferencesChange: SettingsPageProps["onPreferencesChange"];
} {
  const onPreferencesChange = vi.fn();
  renderToStaticMarkup(createElement(SettingsPage,
    props({ onPreferencesChange, ...overrides })));
  const picker = recorded.find(entry => entry.label === "Interface style");
  if (picker === undefined) throw new Error("Settings never rendered an Interface style picker");
  return { picker, onPreferencesChange };
}

beforeEach(() => { recorded.length = 0; });

describe("pressing the Interface style control", () => {
  it("offers both modes with the copy the menu shows", () => {
    const { picker } = chromePicker();
    expect(picker.options.map(option => option.value)).toEqual(["refined", "cli"]);
    expect(picker.options.map(option => option.label)).toEqual(["Refined", "Bare CLI"]);
    expect(picker.options[1].description)
      .toBe("Flat terminal console: monospace, square corners, no shadows or blur. Switches to a console palette.");
    expect(picker.value).toBe("refined");
  });

  it("writes the whole normalized preference object through onPreferencesChange", () => {
    const { picker, onPreferencesChange } = chromePicker();
    picker.onChange("cli");
    expect(onPreferencesChange).toHaveBeenCalledTimes(1);
    expect(onPreferencesChange).toHaveBeenCalledWith({
      ...DEFAULT_APP_PREFERENCES, uiChrome: "cli",
    });
  });

  it("presses back to refined without disturbing its neighbours", () => {
    const { picker, onPreferencesChange } = chromePicker({
      preferences: { ...DEFAULT_APP_PREFERENCES, uiChrome: "cli", uiFont: "serif", lastWorkspaceTheme: "hermes" },
    });
    expect(picker.value).toBe("cli");
    picker.onChange("refined");
    expect(onPreferencesChange).toHaveBeenCalledWith({
      ...DEFAULT_APP_PREFERENCES, uiChrome: "refined", uiFont: "serif", lastWorkspaceTheme: "hermes",
    });
  });

  it("ignores a value it does not recognise instead of persisting it", () => {
    const { picker, onPreferencesChange } = chromePicker();
    picker.onChange("terminal");
    expect(onPreferencesChange).not.toHaveBeenCalled();
  });

  it("leaves the theme picker on its own IPC path", () => {
    const onAppearanceChange = vi.fn();
    const onPreferencesChange = vi.fn();
    renderToStaticMarkup(createElement(SettingsPage,
      props({ onAppearanceChange, onPreferencesChange })));
    const chrome = recorded.find(entry => entry.label === "Interface style");
    chrome?.onChange("cli");
    // Form is a renderer preference; colour stays IPC-backed. Crossing the two
    // is what would drag the main process into a change it has no stake in.
    expect(onAppearanceChange).not.toHaveBeenCalled();
    expect(onPreferencesChange).toHaveBeenCalledTimes(1);
  });
});

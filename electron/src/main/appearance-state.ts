import {
  isCustomThemeSummary, isPanePresentation, isProjectPresentation, isResolvedThemeId,
  isSurfaceTranslucency, isSystemThemes, isThemeId, isWorkspacePresentation,
  type AppearanceState,
} from "../shared/appearance.js";

/** The largest theme file Rust reads, as UTF-16 code units can only be fewer. */
const THEME_CSS_MAX_LENGTH = 1024 * 1024;

type OptionalAppearanceField = Exclude<keyof AppearanceState, "preference" | "resolved">;

/** Each optional surface field, its guard and the name its refusal uses, in check order. */
const OPTIONAL_FIELDS: ReadonlyArray<readonly [OptionalAppearanceField, (value: unknown) => boolean, string]> = [
  ["translucency", isSurfaceTranslucency, "translucency"],
  ["panePresentation", isPanePresentation, "pane presentation"],
  ["workspacePresentation", isWorkspacePresentation, "workspace presentation"],
  ["projectPresentation", isProjectPresentation, "project presentation"],
  ["customThemes", value => Array.isArray(value) && value.length <= 64
    && value.every(isCustomThemeSummary), "theme file"],
  ["systemThemes", isSystemThemes, "Follow macOS theme"],
  ["themeCss", value => typeof value === "string" && value.length <= THEME_CSS_MAX_LENGTH, "theme CSS"],
];

/** Validates the host's appearance result before main applies it to the window. */
export function appearanceState(value: unknown): AppearanceState {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TypeError("The Rust host returned an invalid appearance state.");
  }
  const state = value as Record<string, unknown>;
  if (!isThemeId(state.preference) || !isResolvedThemeId(state.resolved)) {
    throw new TypeError("The Rust host returned an invalid appearance state.");
  }
  const result: Record<string, unknown> = { preference: state.preference, resolved: state.resolved };
  for (const [field, valid, label] of OPTIONAL_FIELDS) {
    const fieldValue = state[field];
    if (fieldValue === undefined) continue;
    if (!valid(fieldValue)) {
      throw new TypeError(`The Rust host returned invalid ${label} settings.`);
    }
    result[field] = fieldValue;
  }
  return result as unknown as AppearanceState;
}

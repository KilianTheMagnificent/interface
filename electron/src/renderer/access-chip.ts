import type { AppSelectOption } from "./components/AppSelect";
import { accessModeOption, accessModeOptions } from "./access-modes";
import { sessionStatusMode } from "./session-status";

/**
 * THE ROWS AN ACCESS PICKER OFFERS for a live session, shared by the pane
 * header and the Threads composer's access chip. The provider's list is
 * authoritative; this only decides what the closed trigger says and how an
 * inherited policy the user cannot re-pick is shown.
 *
 *  - "status": the terse CLI status wording (`›› auto mode on`) the pane
 *    header has always shown — the same words the CLI session line printed, so
 *    moving that control did not move them.
 *  - "label": the row's own short label from access-modes.ts ("Auto mode",
 *    "Accept edits"), which is what a chip beside a shield glyph says.
 *
 * An inherited policy (`unknown`) is marked "Current" and disabled. When it is
 * not among the provider's rows at all it is prepended as one, because a
 * picker whose trigger names a value its menu cannot show is a picker that
 * lies about what is running.
 */
export function accessSelectOptions(
  modes: readonly string[], current: string, unknown: boolean,
  trigger: "status" | "label" = "status", engine?: string,
): AppSelectOption[] {
  const withTrigger = (option: AppSelectOption): AppSelectOption => {
    if (trigger === "label") return option;
    const status = sessionStatusMode(option.value, engine);
    if (status.label === "") return option;
    return { ...option, triggerLabel: `${status.mark} ${status.label}` };
  };
  const rows = accessModeOptions(modes, engine).map(withTrigger);
  if (!unknown) return rows;
  if (rows.some(row => row.value === current)) {
    return rows.map(row => row.value === current
      ? {
        ...row,
        disabled: true,
        meta: "Current",
        description: "Current policy inherited from the provider configuration.",
      }
      : row);
  }
  const label = current === "" ? "Unknown policy"
    : trigger === "label" ? accessModeOption(current, engine).label : current;
  return [withTrigger({
    value: current,
    label,
    description: "Current policy inherited from the provider configuration.",
    meta: "Current",
    disabled: true,
  }), ...rows];
}

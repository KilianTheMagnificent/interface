import { useId, useState } from "react";
import type { ReactNode } from "react";

/**
 * The parts every Settings page is built from (owner, 2026-11-15): a small
 * sentence-case heading over one bordered card, rows of a name and one short
 * help line beside their control, one switch, and a link that opens what a
 * row keeps out of sight. Their styles live in `SETTINGS_PAGE_CSS`.
 */

export interface SettingsRowProps {
  labelId: string;
  label: ReactNode;
  /** One short help line. */
  description: ReactNode;
  /** More under the help line, such as what an inline link opens. */
  detail?: ReactNode;
  children?: ReactNode;
}

/** One setting: its name and one short help line on the left, its control on
 *  the right. No icon tile. */
export function SettingsRow({ labelId, label, description, detail, children }: SettingsRowProps) {
  return <div className="settings-row">
    <div className="settings-row-copy">
      <span id={labelId} className="settings-row-label">{label}</span>
      <span id={`${labelId}-description`} className="settings-row-description">{description}</span>
      {detail}
    </div>
    {children !== undefined && <div className="settings-row-action">{children}</div>}
  </div>;
}

/** A small sentence-case heading over one bordered card of rows. */
export function SettingsGroup({ label, tone, busy, children }: {
  label: ReactNode;
  /** Amber, for a group of things you need to set up. */
  tone?: "attention";
  busy?: boolean;
  children: ReactNode;
}) {
  return <div className="settings-group">
    <h3 className={`settings-group-label${tone === undefined ? "" : ` is-${tone}`}`}>{label}</h3>
    <div className="settings-card" aria-busy={busy || undefined}>{children}</div>
  </div>;
}

/** The one switch: named by its row (or `label`), its state said by its look
 *  and to assistive tech, with no "On"/"Off" word beside it. */
export function SettingsSwitch({ labelId, label, checked, disabled, busy, onChange }: {
  /** The row whose name and help line name and describe it. */
  labelId?: string;
  /** Its name, when it has no row of its own. */
  label?: string;
  checked: boolean;
  disabled?: boolean;
  busy?: boolean;
  onChange: (checked: boolean) => void;
}) {
  return <label className="settings-switch">
    <input type="checkbox" role="switch" checked={checked} disabled={disabled}
      aria-busy={busy || undefined} aria-label={label}
      aria-labelledby={labelId}
      aria-describedby={labelId === undefined ? undefined : `${labelId}-description`}
      onChange={event => { onChange(event.currentTarget.checked); }} />
  </label>;
}

/**
 * A text link inside a help line that opens what the row keeps out of sight
 * (setup commands, a folder path): `link` draws the link, `panel` what it
 * opens, each where the row wants it.
 */
export function useSettingsDisclosure(initiallyOpen = false): {
  open: boolean;
  toggle: () => void;
  link: (label: ReactNode, openLabel?: ReactNode) => ReactNode;
  panel: (children: ReactNode, className?: string) => ReactNode;
} {
  const [open, setOpen] = useState(initiallyOpen);
  const id = useId();
  const toggle = (): void => { setOpen(value => !value); };
  return {
    open,
    toggle,
    link: (label, openLabel = label) => <button type="button" className="settings-link"
      aria-expanded={open} aria-controls={id} onClick={toggle}>{open ? openLabel : label}</button>,
    panel: (children, className = "settings-row-more") => <div id={id} className={className}
      hidden={!open}>{children}</div>,
  };
}

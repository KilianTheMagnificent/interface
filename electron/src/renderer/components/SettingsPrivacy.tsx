import { SettingsGroup, SettingsRow } from "./SettingsControls";
import {
  PRIVACY_FOOT, PRIVACY_GROUP_LABEL, PRIVACY_PAGE_INTRO, PRIVACY_ROWS, type PrivacyRowSection,
} from "../privacy";

/**
 * Settings › Privacy (owner, 2026-11-21): everything that leaves this Mac on
 * its own, one row each, built like Notifications. A row ends with a link to
 * the page that controls it, drawn only when that page is in the menu.
 */
export function PrivacySection({ id, hidden, pages, onGo }: {
  id: string;
  hidden: boolean;
  /** The pages the menu shows now, with their menu names. */
  pages: readonly { id: string; label: string }[];
  onGo: (section: PrivacyRowSection) => void;
}) {
  return <section id={id} className="settings-section" hidden={hidden} aria-labelledby={`${id}-title`}>
    <div className="settings-section-heading">
      <div>
        <h2 id={`${id}-title`} className="settings-section-title">Privacy</h2>
        <p className="settings-section-copy">{PRIVACY_PAGE_INTRO}</p>
      </div>
    </div>
    <SettingsGroup label={PRIVACY_GROUP_LABEL}>
      {PRIVACY_ROWS.map(row => {
        const page = row.section === undefined ? undefined : pages.find(entry => entry.id === row.section);
        return <SettingsRow key={row.key} labelId={`${id}-${row.key}`} label={row.label}
          description={row.description}>
          {page !== undefined && row.section !== undefined && (
            <button type="button" className="settings-link" aria-label={`Go to ${page.label}`}
              onClick={() => { onGo(row.section!); }}>{page.label}</button>
          )}
        </SettingsRow>;
      })}
    </SettingsGroup>
    <p className="settings-foot">{PRIVACY_FOOT}</p>
  </section>;
}

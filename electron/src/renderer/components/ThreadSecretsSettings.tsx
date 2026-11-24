import "./ThreadSchedules.css";
import type { HeldSecretRow } from "../thread-secrets";

/**
 * Settings → Threads → Secrets (protocol 67, T3 section 12): the secrets
 * Interface holds for agents, by name and thread, each with Forget. Names
 * only: a value is never read back. Drawn as rows of the "Kept for your
 * threads" card (owner, 2026-11-15): a count row, then one row per secret.
 */
export function ThreadSecretsSettings({ rows, locked, error, onForget }: {
  rows: readonly HeldSecretRow[];
  locked: boolean;
  error: string | null;
  onForget: (row: HeldSecretRow) => void;
}) {
  return <>
    <div className="settings-row" data-thread-secrets-settings="">
      <div className="settings-row-copy">
        <span className="settings-row-label">Secrets</span>
        <span className="settings-row-description">
          {rows.length === 0
            ? "None held. When an agent asks for one, a card appears above its composer."
            : "Held for the agent that asked, until it is removed or you forget it."}
        </span>
      </div>
      <span className="settings-row-count">{rows.length}</span>
    </div>
    {rows.map(row => (
      <div key={row.rowKey} className="settings-row is-item" data-secret-row={row.name}>
        <div className="settings-row-copy">
          <span className="settings-row-label" title={row.name}>{row.name} · {row.threadTitle}</span>
          <span className="settings-row-description">{row.expires}</span>
        </div>
        <div className="settings-row-action">
          <button type="button" className="settings-button" disabled={locked}
            onClick={() => { onForget(row); }}>Forget</button>
        </div>
      </div>
    ))}
    {error !== null && <p className="settings-error" role="alert">Secrets: {error}</p>}
  </>;
}

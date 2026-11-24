import { useId, useState } from "react";
import type { ProviderAccountsCommand } from "../../shared/provider-accounts";
import {
  ACCOUNT_TERMS_NOTE, accountFolderWords, accountNameDraft, accountRemoveWords,
  type AccountGroupView, type AccountRowView,
} from "../provider-accounts-view";
import { homeRelativePath } from "../display-paths";
import { ConfirmDialog } from "./ConfirmDialog";

/**
 * Settings → Models & providers' accounts (protocol 60): one provider's
 * accounts behind its row's Accounts menu (owner, 2026-11-15), with Add
 * account, Rename, Set as default, Sign in (only for an account that is not
 * signed in) and Remove. Rust owns the accounts; this only draws them and
 * asks.
 */
export interface ProviderAccountsPanelProps {
  /** The panel's element id, which the row's Accounts button controls. */
  id: string;
  open: boolean;
  group: AccountGroupView;
  pending: boolean;
  onChange: (command: ProviderAccountsCommand) => void;
  /** Opens the account's own sign-in in a Code terminal. */
  onSignIn?: (engine: string, account: string) => void;
  /** Masks an identity for screen sharing, as the provider rows do. */
  mask: (identity: string) => string;
}

type Editing = { account: string | null; text: string } | null;
type Removing = { row: AccountRowView; step: "remove" | "folder" } | null;

/** Whether one row's Accounts menu is open, and the id its panel carries. */
export function useAccountsMenu(): { open: boolean; id: string; toggle: () => void } {
  const [open, setOpen] = useState(false);
  const id = useId();
  return { open, id, toggle: () => { setOpen(value => !value); } };
}

/** A provider row's bordered Accounts menu button. */
export function ProviderAccountsButton({ label, open, controls, onToggle }: {
  label: string; open: boolean; controls: string; onToggle: () => void;
}) {
  return <button type="button" className="settings-button" aria-expanded={open}
    aria-controls={controls} aria-label={`${label} accounts`} onClick={onToggle}>
    Accounts
    <span className={`codicon codicon-chevron-${open ? "up" : "down"}`} aria-hidden="true" />
  </button>;
}

/** What the Accounts menu opens under its row: every account and its actions. */
export function ProviderAccountsPanel({ id, open, group, pending, onChange, onSignIn, mask }:
    ProviderAccountsPanelProps) {
  // One name field at a time: a new account's (`account` null) or a rename's.
  const [editing, setEditing] = useState<Editing>(null);
  const [removing, setRemoving] = useState<Removing>(null);

  const submitName = (): void => {
    if (editing === null) return;
    const label = accountNameDraft(editing.text);
    if (label === null) return;
    onChange(editing.account === null
      ? { command: "add", provider: group.provider, label }
      : { command: "rename", provider: group.provider, account: editing.account, label });
    setEditing(null);
  };
  const nameField = (placeholder: string) => editing !== null && (
    <form className="settings-account-name" onSubmit={event => { event.preventDefault(); submitName(); }}>
      <input className="settings-account-input" autoFocus value={editing.text}
        aria-label={placeholder} placeholder={placeholder} maxLength={40} disabled={pending}
        onChange={event => { setEditing({ ...editing, text: event.currentTarget.value }); }}
        onKeyDown={event => { if (event.key === "Escape") { event.stopPropagation(); setEditing(null); } }} />
      <button type="submit" className="settings-button is-primary"
        disabled={pending || accountNameDraft(editing.text) === null}>
        {editing.account === null ? "Add" : "Rename"}
      </button>
      <button type="button" className="settings-button" onClick={() => { setEditing(null); }}>Cancel</button>
    </form>
  );

  const removeWords = removing === null ? null : removing.step === "remove"
    ? accountRemoveWords(group.label, removing.row)
    : accountFolderWords(removing.row.directory ?? "");
  const remove = (deleteDirectory: boolean): void => {
    if (removing === null) return;
    onChange({ command: "remove", provider: group.provider, account: removing.row.id,
      ...(deleteDirectory ? { deleteDirectory: true } : {}) });
    setRemoving(null);
  };

  return (
    <div id={id} className="settings-accounts" hidden={!open} data-account-provider={group.provider}>
      <ul className="settings-account-list" aria-label={`${group.label} accounts`}>
        {group.rows.map(row => (
          <li key={row.id} className="settings-account" data-account={row.id}>
            <span className="settings-row-copy">
              <span className="settings-account-title">
                {row.label}
                {row.isDefault && <span className="settings-account-badge">Default</span>}
              </span>
              <span className="settings-row-description">
                {row.identity === null ? row.statusText : `${row.statusText} ${mask(row.identity)}`}
                {row.directory !== null && <span title={row.directory}> · {homeRelativePath(row.directory)}</span>}
              </span>
              {editing !== null && editing.account === row.id && nameField("Account name")}
            </span>
            <span className="settings-account-actions">
              {row.canSignIn && onSignIn !== undefined && (
                <button type="button" className="settings-button" disabled={pending}
                  onClick={() => { onSignIn(group.engine, row.id); }}>Sign in</button>
              )}
              {row.canSetDefault && (
                <button type="button" className="settings-button" disabled={pending}
                  onClick={() => { onChange({ command: "setDefault", provider: group.provider, account: row.id }); }}>
                  Set as default
                </button>
              )}
              {row.canRename && (
                <button type="button" className="settings-button" disabled={pending}
                  onClick={() => { setEditing({ account: row.id, text: row.label }); }}>
                  Rename
                </button>
              )}
              {row.canRemove && (
                <button type="button" className="settings-button" disabled={pending}
                  onClick={() => { setRemoving({ row, step: "remove" }); }}>Remove…</button>
              )}
            </span>
          </li>
        ))}
      </ul>
      {editing !== null && editing.account === null
        ? nameField(`New ${group.label} account name`)
        : (
          <button type="button" className="settings-link settings-account-add" disabled={pending}
            onClick={() => { setEditing({ account: null, text: "" }); }}>
            Add {group.label} account
          </button>
        )}
      <p className="settings-foot">{ACCOUNT_TERMS_NOTE}</p>
      {removing !== null && removeWords !== null && (
        <ConfirmDialog open title={removeWords.title} description={removeWords.description}
          confirmLabel={removeWords.confirmLabel}
          cancelLabel={removeWords.cancelLabel}
          onConfirm={() => {
            if (removing.step === "remove" && removing.row.directory !== null) {
              setRemoving({ ...removing, step: "folder" });
            } else remove(removing.step === "folder");
          }}
          // The folder step's Cancel is "Keep folder": the account still goes,
          // as its first confirmation said; only the folder stays.
          onCancel={() => { if (removing.step === "folder") remove(false); else setRemoving(null); }} />
      )}
    </div>
  );
}

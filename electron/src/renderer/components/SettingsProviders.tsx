import { useState } from "react";
import type { ReactNode } from "react";
import type { EngineId } from "../../shared/engine";
import type { ProviderAccountsCommand } from "../../shared/provider-accounts";
import type { TerminalLaunchProfile } from "../../shared/ipc";
import type { ProviderSettingsSummary, TerminalToolSummary } from "../settings";
import { accessModeOptions } from "../access-modes";
import { accountGroupsView, type AccountGroupView } from "../provider-accounts-view";
import type { ProviderAccountsState } from "../use-provider-accounts";
import { withoutIpcErrorShape } from "../ipc-message";
import { AcpProfilesEditor, type AcpProfileView } from "./AcpProfilesEditor";
import { InstallHint } from "./InstallHint";
import { ProviderLogo } from "./ProviderLogo";
import { ProviderAccountsButton, ProviderAccountsPanel, useAccountsMenu } from "./ProviderAccountsSection";
import { SettingsGroup, SettingsRow, SettingsSwitch, useSettingsDisclosure } from "./SettingsControls";

/** At most this many model names in a provider's details. */
const SHOWN_MODELS = 4;

/** "Signed in as an•••@example.com · 2 models": what a ready provider runs on. */
export function providerAccountLine(provider: ProviderSettingsSummary, mask: (identity: string) => string): string {
  const models = provider.modelNames.length === 0 ? provider.modelSummary
    : `${provider.modelNames.length} model${provider.modelNames.length === 1 ? "" : "s"}`;
  return provider.loginIdentity === null ? models
    : `Signed in as ${mask(provider.loginIdentity)} · ${models}`;
}

/** "8 of 9 found on this Mac. Muse Code is not installed." */
export function terminalToolsLine(tools: readonly TerminalToolSummary[]): string {
  const missing = tools.filter(tool => tool.health === "missing").map(tool => tool.label);
  const found = `${tools.length - missing.length} of ${tools.length} found on this Mac.`;
  if (missing.length === 0) return found;
  const names = missing.length === 1 ? missing[0]
    : `${missing.slice(0, -1).join(", ")} and ${missing.at(-1)}`;
  return `${found} ${names} ${missing.length === 1 ? "is" : "are"} not installed.`;
}

interface ProviderRowProps {
  provider: ProviderSettingsSummary;
  id: string;
  accounts: AccountGroupView | undefined;
  accountsPending: boolean;
  onAccountsChange?: (command: ProviderAccountsCommand) => void;
  onAccountSignIn?: (engine: string, account: string) => void;
  onSignIn?: (engine: EngineId) => void;
  mask: (identity: string) => string;
}

/**
 * One provider as a row (owner, 2026-11-15): its mark and name; under it the
 * whole error sentence when it needs setting up, else the account it runs on;
 * on the right the one fix the row offers (Sign in), or Ready and its
 * Accounts menu. Its version, models, access modes and effort levels sit
 * behind Details.
 */
function ProviderRow({ provider, id, accounts, accountsPending, onAccountsChange, onAccountSignIn, onSignIn,
  mask }: ProviderRowProps) {
  const menu = useAccountsMenu();
  const details = useSettingsDisclosure();
  const needsSetup = provider.health === "unavailable" || provider.health === "signedOut";
  const shownModels = provider.modelNames.slice(0, SHOWN_MODELS);
  const moreModels = provider.modelNames.length - shownModels.length;
  const accessModes = accessModeOptions(provider.permissionModes, provider.engine);
  return <>
    <div className="settings-row settings-provider-row" data-provider={provider.engine}>
      <span className="settings-provider-mark" aria-hidden="true">
        <ProviderLogo engine={provider.engine} label={provider.label} size={18} />
      </span>
      <div className="settings-row-copy">
        <span id={`${id}-title`} className="settings-row-label" role="heading" aria-level={4}>{provider.label}</span>
        <span id={`${id}-title-description`}
          className={`settings-row-description${needsSetup ? " is-error" : ""}`}>
          {needsSetup ? provider.error ?? `${provider.label} needs setting up.`
            : provider.health === "checking" ? "Checking…"
              : providerAccountLine(provider, mask)}
          {" "}{details.link("Details", "Hide details")}
        </span>
      </div>
      <div className="settings-row-action">
        {!needsSetup && <span className="settings-provider-health">
          <span className={`settings-provider-dot is-${provider.health}`} aria-hidden="true" />
          {provider.healthLabel}
        </span>}
        {needsSetup && provider.signIn && onSignIn !== undefined && (
          <button type="button" className="settings-button is-primary"
            onClick={() => { onSignIn(provider.engine); }}>Sign in to {provider.label}</button>
        )}
        {accounts !== undefined && onAccountsChange !== undefined && (
          <ProviderAccountsButton label={provider.label} open={menu.open} controls={menu.id} onToggle={menu.toggle} />
        )}
      </div>
    </div>
    {details.panel(<>
      {provider.version !== null && <p className="settings-provider-fact">Version {provider.version}</p>}
      {provider.health === "signedOut" && provider.signIn && onSignIn !== undefined
        && <p className="settings-provider-fact">Sign in opens {provider.label}&rsquo;s sign-in in a Code terminal.</p>}
      {!needsSetup && provider.loginIdentity === null && provider.health === "ready"
        && <p className="settings-provider-fact">Authentication managed by the provider CLI.</p>}
      {shownModels.length > 0 && <div className="settings-provider-chips" role="group"
        aria-label={`${provider.label} models`}>
        <span className="settings-provider-chip-label">Models</span>
        {shownModels.map(model => <span key={model} className="settings-provider-model" title={model}>{model}</span>)}
        {moreModels > 0 && <span className="settings-provider-model">+{moreModels} more</span>}
      </div>}
      {accessModes.length > 0 && <div className="settings-provider-chips" role="group"
        aria-label={`${provider.label} access modes`}>
        <span className="settings-provider-chip-label">Access</span>
        {accessModes.map(mode => <span key={mode.value} className="settings-provider-capability"
          title={mode.description}>{mode.label}</span>)}
      </div>}
      {provider.effortLevels.length > 0 && <div className="settings-provider-chips" role="group"
        aria-label={`${provider.label} effort levels`}>
        <span className="settings-provider-chip-label">Effort</span>
        {provider.effortLevels.map(level => <span key={level} className="settings-provider-capability">{level}</span>)}
      </div>}
      {provider.errorDetail !== null && <p className="settings-provider-fact">
        {provider.label} said: <code>{provider.errorDetail}</code></p>}
    </>, "settings-row-more settings-provider-details")}
    {accounts !== undefined && onAccountsChange !== undefined && (
      <ProviderAccountsPanel id={menu.id} open={menu.open} group={accounts} pending={accountsPending}
        onChange={onAccountsChange} onSignIn={onAccountSignIn} mask={mask} />
    )}
  </>;
}

/** An account group whose provider has no row (its probe has not reported). */
function AccountsOnlyRow({ group, pending, onChange, onSignIn, mask }: {
  group: AccountGroupView; pending: boolean; onChange: (command: ProviderAccountsCommand) => void;
  onSignIn?: (engine: string, account: string) => void; mask: (identity: string) => string;
}) {
  const menu = useAccountsMenu();
  return <>
    <SettingsRow labelId={`${menu.id}-row`} label={group.label}
      description={`${group.rows.length} account${group.rows.length === 1 ? "" : "s"}`}>
      <ProviderAccountsButton label={group.label} open={menu.open} controls={menu.id} onToggle={menu.toggle} />
    </SettingsRow>
    <ProviderAccountsPanel id={menu.id} open={menu.open} group={group} pending={pending}
      onChange={onChange} onSignIn={onSignIn} mask={mask} />
  </>;
}

export interface ProvidersSectionProps {
  id: string;
  hidden: boolean;
  providers: readonly ProviderSettingsSummary[];
  refreshing: boolean;
  refreshError: string | null;
  onRefresh: () => void;
  onSignIn?: (engine: EngineId) => void;
  providerAccounts?: ProviderAccountsState;
  onProviderAccountsChange?: (command: ProviderAccountsCommand) => void;
  onAccountSignIn?: (engine: string, account: string) => void;
  /** Custom providers' display names, keyed by engine id. */
  labels: Readonly<Record<string, string>>;
  mask: (identity: string) => string;
  tools: readonly TerminalToolSummary[] | null;
  onPaneToolsChange?: (profileId: TerminalLaunchProfile["id"], enabled: boolean) => void;
  paneToolsPending: TerminalLaunchProfile["id"] | null;
  paneToolsError: string | null;
  acpProfiles: readonly AcpProfileView[];
  acpProfilesPending: boolean;
  acpProfilesError: string | null;
  acpProfilesRestartRequired: boolean;
  acpProfilesSavedRevision: number;
  onSaveAcpProfiles?: (profiles: AcpProfileView[]) => void;
}

/**
 * Settings › Models & providers, as the owner's mockup draws it (2026-11-15):
 * what needs setting up first, each with its whole sentence and its fix; then
 * what is ready, each with its account and an Accounts menu; then More:
 * Terminal tools behind "Show all N" and Custom providers behind their editor.
 */
export function ProvidersSection({
  id, hidden, providers, refreshing, refreshError, onRefresh, onSignIn, providerAccounts,
  onProviderAccountsChange, onAccountSignIn, labels, mask, tools, onPaneToolsChange, paneToolsPending,
  paneToolsError, acpProfiles, acpProfilesPending, acpProfilesError, acpProfilesRestartRequired,
  acpProfilesSavedRevision, onSaveAcpProfiles,
}: ProvidersSectionProps) {
  const [toolsOpen, setToolsOpen] = useState(false);
  const [customOpen, setCustomOpen] = useState(false);
  const accountGroups = providerAccounts === undefined || onProviderAccountsChange === undefined ? []
    : accountGroupsView(providerAccounts.view, labels);
  const accountsFor = (engine: string): AccountGroupView | undefined =>
    accountGroups.find(group => group.engine === engine);
  const needsSetup = providers.filter(provider =>
    provider.health === "unavailable" || provider.health === "signedOut");
  const ready = providers.filter(provider => provider.health === "ready" || provider.health === "checking");
  const readyCount = providers.filter(provider => provider.health === "ready").length;
  const orphans = accountGroups.filter(group => !providers.some(provider => provider.engine === group.engine));
  const rowId = (engine: string): string => `${id}-${engine.replace(/[^a-z0-9_-]/gi, "-")}`;
  const row = (provider: ProviderSettingsSummary): ReactNode => <ProviderRow key={provider.engine}
    provider={provider} id={rowId(provider.engine)} accounts={accountsFor(provider.engine)}
    accountsPending={providerAccounts?.pending ?? false} onAccountsChange={onProviderAccountsChange}
    onAccountSignIn={onAccountSignIn} onSignIn={onSignIn} mask={mask} />;
  const customCopy = acpProfilesError !== null ? acpProfilesError
    : acpProfilesPending ? "Saving custom providers…"
      : acpProfilesRestartRequired ? "Restart required for custom providers."
        : acpProfiles.length === 0 ? "None added. For connecting a provider that is not listed above."
          : `${acpProfiles.length} added: ${acpProfiles.map(profile => profile.displayName).join(", ")}.`;

  return <section id={id} className="settings-section" hidden={hidden} aria-labelledby={`${id}-title`}>
    <div className="settings-section-heading">
      <div>
        <h2 id={`${id}-title`} className="settings-section-title">Models &amp; providers</h2>
        <p className="settings-section-copy">
          {providers.length === 0 ? "Provider status has not been reported yet."
            : `${readyCount} of ${providers.length} providers are ready to use.`}
        </p>
      </div>
      <button type="button" className="settings-button" disabled={refreshing} aria-busy={refreshing || undefined}
        onClick={onRefresh}>
        {refreshing ? "Refreshing…" : "Refresh"}
      </button>
    </div>

    <div className="settings-provider-groups" aria-live="polite" aria-busy={refreshing || undefined}>
      {providers.length === 0 && <div className="settings-card settings-empty">
        {refreshing ? "Checking local provider connections…" : "No providers reported."}
      </div>}
      {needsSetup.length > 0 && <SettingsGroup label="Needs setup" tone="attention">{needsSetup.map(row)}</SettingsGroup>}
      {ready.length > 0 && <SettingsGroup label="Ready">{ready.map(row)}</SettingsGroup>}
      {orphans.length > 0 && onProviderAccountsChange !== undefined && <SettingsGroup label="Other accounts">
        {orphans.map(group => <AccountsOnlyRow key={group.provider} group={group}
          pending={providerAccounts?.pending ?? false} onChange={onProviderAccountsChange}
          onSignIn={onAccountSignIn} mask={mask} />)}
      </SettingsGroup>}
    </div>
    {providerAccounts !== undefined && (providerAccounts.checking || providerAccounts.notice !== null) && (
      <p className="settings-foot settings-account-notice" role="status">
        {providerAccounts.checking ? "Checking each account's sign-in…" : providerAccounts.notice}
      </p>
    )}
    {providerAccounts !== undefined && providerAccounts.error !== null && <p className="settings-error" role="alert">
      {withoutIpcErrorShape(providerAccounts.error)}</p>}

    {(tools !== null || onSaveAcpProfiles !== undefined) && <SettingsGroup label="More">
      {tools !== null && <SettingsRow labelId={`${id}-tools`} label="Terminal tools"
        description={`${terminalToolsLine(tools)} What the New workspace launcher can run.`}>
        <button type="button" className="settings-button" aria-expanded={toolsOpen}
          aria-controls={`${id}-tool-list`} onClick={() => { setToolsOpen(open => !open); }}>
          {toolsOpen ? "Hide" : `Show all ${tools.length}`}
        </button>
      </SettingsRow>}
      {tools !== null && <ul id={`${id}-tool-list`} className="settings-tool-list" aria-label="Terminal tools"
        hidden={!toolsOpen}>
        {tools.map(tool => <li key={tool.id} className="settings-tool" data-terminal-tool={tool.id}>
          <span className="settings-provider-mark" aria-hidden="true">
            {tool.engine === null
              ? <span className="codicon codicon-terminal" />
              : <ProviderLogo engine={tool.engine} label={tool.label} size={18} />}
          </span>
          <span className="settings-tool-copy">
            <span className="settings-tool-name">{tool.label}</span>
            <span className="settings-provider-meta">
              <span className="settings-provider-health">
                <span className={`settings-provider-dot is-${tool.health}`} aria-hidden="true" />
                {tool.healthLabel}
              </span>
              {tool.detail !== null && <span>{tool.detail}</span>}
              {tool.auth !== null && tool.health !== "signedOut" && <span>{tool.auth}</span>}
            </span>
            {tool.missing !== null && <span className="settings-tool-missing">
              <InstallHint hint={tool.missing} />
            </span>}
            {tool.changed !== null && <span className="settings-tool-missing" data-tool-change="">
              <InstallHint hint={tool.changed} />
            </span>}
            {tool.paneTools !== null && <span className="settings-tool-pane-tools">
              <span className="settings-tool-pane-tools-head">
                <span id={`${id}-pane-tools-${tool.id}`}>{tool.paneTools.label}</span>
                <SettingsSwitch labelId={`${id}-pane-tools-${tool.id}`} checked={tool.paneTools.on}
                  disabled={tool.paneTools.locked || onPaneToolsChange === undefined || paneToolsPending !== null}
                  busy={paneToolsPending === tool.id}
                  onChange={on => { onPaneToolsChange?.(tool.id, on); }} />
              </span>
              <span id={`${id}-pane-tools-${tool.id}-description`} className="settings-tool-missing">
                {tool.paneTools.note}</span>
            </span>}
          </span>
          {tool.signIn && tool.engine !== null && onSignIn !== undefined && (
            <button type="button" className="settings-button"
              onClick={() => { if (tool.engine !== null) onSignIn(tool.engine); }}>Sign in</button>
          )}
        </li>)}
      </ul>}
      {onSaveAcpProfiles !== undefined && <SettingsRow labelId={`${id}-custom`} label="Custom providers"
        description={<span className={acpProfilesError !== null ? "is-error" : undefined}
          title={acpProfilesError ?? undefined}>{customCopy}</span>}>
        <button type="button" className="settings-button" aria-expanded={customOpen}
          aria-controls={`${id}-custom-editor`} onClick={() => { setCustomOpen(open => !open); }}>
          {customOpen ? "Done" : acpProfiles.length === 0 ? "Add provider" : "Edit"}
        </button>
      </SettingsRow>}
      {onSaveAcpProfiles !== undefined && <div id={`${id}-custom-editor`} className="settings-row-more"
        hidden={!customOpen}>
        <AcpProfilesEditor profiles={acpProfiles} pending={acpProfilesPending} error={acpProfilesError}
          restartRequired={acpProfilesRestartRequired} savedRevision={acpProfilesSavedRevision}
          onSave={onSaveAcpProfiles} />
      </div>}
    </SettingsGroup>}
    {paneToolsError !== null && <p className="settings-error" role="alert">{paneToolsError}</p>}

    <p className="settings-refresh-note" role="status" aria-live="polite">
      {refreshing ? "Refreshing provider health and model catalogs…"
        : refreshError === null ? "Provider status is up to date." : ""}
    </p>
    {refreshError !== null && <p className="settings-error" role="alert">
      Providers could not be refreshed: {refreshError}
    </p>}
  </section>;
}

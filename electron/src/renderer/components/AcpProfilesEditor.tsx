import { useEffect, useId, useMemo, useRef, useState, type ChangeEvent } from "react";
import { ProviderLogo } from "./ProviderLogo";

export interface AcpProfileView {
  slug: string;
  displayName: string;
  executablePath: string;
  args: string[];
}

interface DraftProfile {
  /** Render identity only; never serialized into the persisted profile. */
  uiId: string;
  slug: string;
  displayName: string;
  executablePath: string;
  argsJson: string;
}

export type AcpProfileField = "slug" | "displayName" | "executablePath" | "argsJson";
export type AcpProfileFieldErrors = Partial<Record<AcpProfileField, string>>;

export interface AcpProfileDraftInput {
  slug: string;
  displayName: string;
  executablePath: string;
  argsJson: string;
}

export interface AcpProfileDraftValidation {
  errors: AcpProfileFieldErrors;
  profile: AcpProfileView | null;
}

const ACP_SLUG = /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/;
const CREDENTIAL_SHAPE = /(?:api[-_]?key|token|secret|password|credential)/i;
const SHELL_EXECUTABLES = new Set([
  "sh", "bash", "zsh", "dash", "ksh", "csh", "tcsh", "fish", "nu", "env",
]);

const characterCount = (value: string): number => Array.from(value).length;

/** Mirrors backend syntax rules so errors can sit beside the field before Save. */
export function validateAcpProfileDraft(
  draft: AcpProfileDraftInput,
  duplicateSlug = false,
): AcpProfileDraftValidation {
  const errors: AcpProfileFieldErrors = {};
  if (!ACP_SLUG.test(draft.slug) || characterCount(draft.slug) > 64) {
    errors.slug = "Use 1–64 lowercase letters, numbers, or hyphens.";
  } else if (draft.slug === "grok" || draft.slug === "qwen") {
    errors.slug = `“${draft.slug}” is reserved for Interface’s built-in provider.`;
  } else if (duplicateSlug) {
    errors.slug = "Each provider needs a unique stable slug.";
  }

  const displayName = draft.displayName.trim();
  if (displayName === "" || characterCount(displayName) > 80
      || /[\0-\x1f\x7f]/.test(displayName)) {
    errors.displayName = "Enter 1–80 visible characters.";
  }

  const executableName = draft.executablePath.split("/").at(-1)?.toLocaleLowerCase() ?? "";
  if (!draft.executablePath.startsWith("/") || draft.executablePath.length > 4_096
      || /[\0\r\n]/.test(draft.executablePath) || draft.executablePath.endsWith("/")
      || SHELL_EXECUTABLES.has(executableName)) {
    errors.executablePath = "Enter an absolute path to a non-shell executable.";
  }

  let args: string[] | null = null;
  try {
    const parsed: unknown = JSON.parse(draft.argsJson);
    if (!Array.isArray(parsed) || parsed.length > 64
        || parsed.some(value => typeof value !== "string")) {
      errors.argsJson = "Use a JSON array containing at most 64 string arguments.";
    } else if (parsed.some(value => characterCount(value) > 1_024 || value.includes("\0"))) {
      errors.argsJson = "Each argument must be at most 1,024 characters and contain no NUL.";
    } else if (parsed.some(value => CREDENTIAL_SHAPE.test(value))) {
      errors.argsJson = "Arguments cannot contain credential-shaped flags or values.";
    } else {
      args = parsed;
    }
  } catch {
    errors.argsJson = "Enter valid JSON, for example [\"agent\", \"stdio\"].";
  }

  return {
    errors,
    profile: Object.keys(errors).length === 0 && args !== null
      ? { slug: draft.slug, displayName, executablePath: draft.executablePath, args }
      : null,
  };
}

export interface AcpProfilesEditorProps {
  profiles: readonly AcpProfileView[];
  pending?: boolean;
  error?: string | null;
  restartRequired?: boolean;
  /** Increments only after the backend has durably accepted and normalized a save. */
  savedRevision?: number;
  onSave: (profiles: AcpProfileView[]) => void;
}

const draftOf = (profile: AcpProfileView, uiId: string): DraftProfile => ({
  uiId,
  slug: profile.slug,
  displayName: profile.displayName,
  executablePath: profile.executablePath,
  argsJson: JSON.stringify(profile.args),
});

const EMPTY_DRAFT: DraftProfile = {
  uiId: "", slug: "", displayName: "", executablePath: "", argsJson: "[]",
};

/** The four fields of a provider row, in the order they draw and are checked. */
const FIELDS: readonly {
  readonly field: AcpProfileField;
  readonly label: string;
  readonly wide?: true;
  readonly multiline?: true;
  readonly placeholder: string;
  readonly autoComplete?: "off";
  readonly spellCheck?: false;
  readonly maxLength?: number;
}[] = [
  { field: "slug", label: "Stable slug", placeholder: "my-agent", autoComplete: "off", maxLength: 64 },
  { field: "displayName", label: "Display name", placeholder: "Custom ACP agent", autoComplete: "off",
    maxLength: 80 },
  { field: "executablePath", label: "Absolute executable path", wide: true,
    placeholder: "/opt/homebrew/bin/provider", autoComplete: "off", spellCheck: false, maxLength: 4_096 },
  { field: "argsJson", label: "Arguments (JSON array; exact argv)", wide: true, multiline: true,
    placeholder: '["agent", "stdio"]', spellCheck: false },
];

export const ACP_PROFILES_CSS = `
.acp-profiles { margin-top:12px; overflow:hidden; background:var(--surface); border:var(--border-w) solid var(--border); border-radius:var(--radius-container); }
.acp-profiles-head { display:flex; flex-wrap:wrap; align-items:center; gap:10px; padding:11px 12px; border-bottom:var(--border-w) solid var(--border); }
.acp-profiles-title { margin:0; color:var(--fg-strong); font-size:var(--text-ui); font-weight:var(--wght-strong); }
.acp-profiles-spacer { flex:1 1 auto; }
.acp-profiles-button { min-height:28px; padding:0 9px; color:var(--fg); background:transparent; border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control); font:inherit; font-size:var(--text-meta); cursor:pointer; }
.acp-profiles-button:hover:not(:disabled) { color:var(--fg-strong); background:var(--row-hover); }
.acp-profiles-button:disabled { color:var(--fg-faint); background:transparent; cursor:default; }
.acp-profiles-copy { margin:0; padding:9px 12px; color:var(--fg-dim); border-bottom:var(--border-w) solid var(--border); font-size:var(--text-meta); line-height:var(--lh-ui); }
.acp-profile { padding:11px 12px; }
.acp-profile + .acp-profile { border-top:var(--border-w) solid var(--border); }
.acp-profile-summary { display:flex; align-items:center; gap:7px; margin:0 0 9px; color:var(--fg-strong); font-size:var(--text-ui); font-weight:var(--wght-strong); }
.acp-profile-summary-mark { display:inline-flex; align-items:center; justify-content:center; width:22px; height:22px; }
.acp-profile-summary-mark .provider-logo { width:20px; height:20px; }
.acp-profile-grid { display:grid; grid-template-columns:minmax(120px,.7fr) minmax(150px,1fr); gap:8px; }
.acp-profile-field { display:grid; gap:4px; min-width:0; color:var(--fg-dim); font-size:var(--text-meta); }
.acp-profile-field.is-wide { grid-column:1 / -1; }
.acp-profile-input { width:100%; min-width:0; min-height:30px; box-sizing:border-box; padding:5px 7px; color:var(--input-fg); background:var(--input-bg); border:var(--border-w) solid var(--input-border); border-radius:var(--radius-control); font-family:var(--font-mono); font-size:var(--text-meta); }
.acp-profile-input::placeholder { color:var(--input-placeholder); }
.acp-profile-input[aria-invalid="true"] { border-color:var(--danger-fg); }
.acp-profile-input:disabled { color:var(--fg-faint); background:var(--surface-raised); cursor:default; }
textarea.acp-profile-input { min-height:54px; resize:vertical; }
.acp-profile-field-error { margin:0; color:var(--danger-fg); font-family:var(--font-ui); font-size:var(--text-meta); line-height:var(--lh-ui); }
.acp-profile-actions { display:flex; justify-content:flex-end; margin-top:8px; }
.acp-profiles-empty { padding:18px 12px; color:var(--fg-dim); text-align:center; font-size:var(--text-meta); }
.acp-profiles-footer { display:flex; flex-wrap:wrap; align-items:center; gap:8px; padding:10px 12px; border-top:var(--border-w) solid var(--border); background:var(--editor-bg); }
/* This line spends most of its life saying "Provider roster is saved." — a
   settled state, and never a summons. Quiet tier for all four of its messages;
   .acp-profiles-error below is where a real failure goes. */
.acp-profiles-status { flex:1 1 auto; color:var(--fg-muted); font-size:var(--text-meta); }
.acp-profiles-error { margin:0; padding:8px 12px; color:var(--danger-fg); background:var(--danger-bg); font-size:var(--text-meta); }
@media (max-width:480px) {
  .acp-profile-grid { grid-template-columns:1fr; }
  .acp-profile-field.is-wide { grid-column:auto; }
  .acp-profiles-spacer { display:none; }
  .acp-profiles-head .acp-profiles-button { width:100%; }
  .acp-profiles-footer { align-items:stretch; }
  .acp-profiles-status { flex-basis:100%; }
  .acp-profiles-footer .acp-profiles-button { width:100%; }
}
`;

export function AcpProfilesEditor({ profiles, pending = false, error = null,
    restartRequired = false, savedRevision = 0, onSave }: AcpProfilesEditorProps) {
  const id = useId();
  const nextDraftId = useRef(0);
  const appliedSavedRevision = useRef(savedRevision);
  /** The roster the drafts were last minted from. */
  const seededFrom = useRef(profiles);
  const makeDraft = (profile: AcpProfileView): DraftProfile =>
    draftOf(profile, `${id}-profile-${nextDraftId.current++}`);
  const [drafts, setDrafts] = useState<DraftProfile[]>(() => profiles.map(makeDraft));
  const [dirty, setDirty] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const [touched, setTouched] = useState<ReadonlySet<string>>(() => new Set());
  const [submitted, setSubmitted] = useState(false);
  const addButtonRef = useRef<HTMLButtonElement>(null);
  const statusRef = useRef<HTMLSpanElement>(null);
  const fieldRefs = useRef(new Map<string, HTMLInputElement | HTMLTextAreaElement>());
  const pendingFocus = useRef<string | "add" | null>(null);

  const validations = useMemo(() => {
    const counts = new Map<string, number>();
    for (const draft of drafts) counts.set(draft.slug, (counts.get(draft.slug) ?? 0) + 1);
    return drafts.map(draft => validateAcpProfileDraft(draft,
      draft.slug !== "" && (counts.get(draft.slug) ?? 0) > 1));
  }, [drafts]);

  const fieldKey = (uiId: string, field: AcpProfileField): string => `${uiId}:${field}`;
  const markTouched = (uiId: string, field: AcpProfileField): void => {
    const key = fieldKey(uiId, field);
    setTouched(current => current.has(key) ? current : new Set([...current, key]));
  };

  useEffect(() => {
    const target = pendingFocus.current;
    if (target === null) return;
    pendingFocus.current = null;
    if (target === "add") addButtonRef.current?.focus();
    else fieldRefs.current.get(target)?.focus();
  }, [drafts]);

  // Reseed only for a roster these drafts were not minted from. Editing a field
  // back to its saved value clears `dirty` without a new roster, and re-minting
  // every row id there would remount the inputs and drop the keyboard focus.
  useEffect(() => {
    if (dirty || profiles === seededFrom.current) return;
    seededFrom.current = profiles;
    setDrafts(profiles.map(makeDraft));
  }, [dirty, profiles]);

  useEffect(() => {
    if (appliedSavedRevision.current === savedRevision) return;
    appliedSavedRevision.current = savedRevision;
    // Inputs are disabled while a save is pending, so once App publishes a
    // successful revision there cannot be a newer local edit to preserve.
    // Adopting the returned roster also makes backend trimming/normalization
    // visible instead of leaving the editor permanently marked unsaved.
    seededFrom.current = profiles;
    setDrafts(profiles.map(makeDraft));
    setDirty(false);
    setTouched(new Set());
    setSubmitted(false);
    setLocalError(null);
    queueMicrotask(() => { statusRef.current?.focus(); });
  }, [savedRevision]);

  const update = (index: number, patch: Partial<DraftProfile>): void => {
    setDrafts(current => current.map((draft, at) => at === index
      ? { ...draft, ...patch } : draft));
    setDirty(true);
    setLocalError(null);
  };

  const save = (): void => {
    setSubmitted(true);
    const firstInvalid = validations.findIndex(validation => validation.profile === null);
    if (firstInvalid >= 0) {
      setLocalError("Fix the highlighted provider fields before saving.");
      const firstField = FIELDS.map(spec => spec.field)
        .find(field => validations[firstInvalid].errors[field] !== undefined);
      if (firstField !== undefined) {
        const focus = fieldKey(drafts[firstInvalid].uiId, firstField);
        queueMicrotask(() => { fieldRefs.current.get(focus)?.focus(); });
      }
      return;
    }
    setLocalError(null);
    onSave(validations.flatMap(validation => validation.profile === null
      ? [] : [validation.profile]));
    // Keep dirty until the parent publishes the normalized saved roster. That
    // prevents a slow save response from replacing what the user just typed.
  };

  useEffect(() => {
    if (pending) return;
    const normalized = profiles.map(profile => ({
      slug: profile.slug, displayName: profile.displayName,
      executablePath: profile.executablePath, argsJson: JSON.stringify(profile.args),
    }));
    const current = drafts.map(draft => ({
      slug: draft.slug, displayName: draft.displayName,
      executablePath: draft.executablePath, argsJson: draft.argsJson,
    }));
    if (JSON.stringify(normalized) === JSON.stringify(current)) setDirty(false);
  }, [drafts, pending, profiles]);

  const fieldError = (index: number, field: AcpProfileField): string | undefined => {
    const draft = drafts[index];
    if (!submitted && !touched.has(fieldKey(draft.uiId, field))) return undefined;
    return validations[index]?.errors[field];
  };

  const renderFieldError = (index: number, field: AcpProfileField) => {
    const message = fieldError(index, field);
    if (message === undefined) return null;
    return <p id={`${drafts[index].uiId}-${field}-error`}
      className="acp-profile-field-error">{message}</p>;
  };

  const renderField = (draft: DraftProfile, index: number, spec: typeof FIELDS[number]) => {
    const key = fieldKey(draft.uiId, spec.field);
    const invalid = fieldError(index, spec.field) !== undefined;
    const control = {
      ref: (node: HTMLInputElement | HTMLTextAreaElement | null) => {
        if (node === null) fieldRefs.current.delete(key); else fieldRefs.current.set(key, node);
      },
      className: "acp-profile-input", value: draft[spec.field], disabled: pending,
      placeholder: spec.placeholder, autoComplete: spec.autoComplete, spellCheck: spec.spellCheck,
      maxLength: spec.maxLength, "aria-invalid": invalid || undefined,
      "aria-describedby": invalid ? `${draft.uiId}-${spec.field}-error` : undefined,
      onBlur: () => { markTouched(draft.uiId, spec.field); },
      onChange: (event: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => {
        update(index, { [spec.field]: event.currentTarget.value });
      },
    };
    return <label key={spec.field}
      className={spec.wide ? "acp-profile-field is-wide" : "acp-profile-field"}>{spec.label}
      {spec.multiline ? <textarea {...control} /> : <input {...control} />}
      {renderFieldError(index, spec.field)}
    </label>;
  };

  return <section className="acp-profiles" data-voice-guard="settings" aria-labelledby={`${id}-title`}
    aria-busy={pending || undefined}>
    <style>{ACP_PROFILES_CSS}</style>
    <header className="acp-profiles-head">
      <h3 id={`${id}-title`} className="acp-profiles-title">Custom ACP providers</h3>
      <span className="acp-profiles-spacer" />
      <button ref={addButtonRef} type="button" className="acp-profiles-button"
        disabled={pending || drafts.length >= 64}
        title={drafts.length >= 64 ? "A roster can contain at most 64 providers." : undefined}
        onClick={() => {
          const uiId = `${id}-profile-${nextDraftId.current++}`;
          pendingFocus.current = fieldKey(uiId, "slug");
          setDrafts(current => [...current, { ...EMPTY_DRAFT, uiId }]);
          setDirty(true);
          setSubmitted(false);
          setLocalError(null);
        }}>Add provider</button>
    </header>
    <p className="acp-profiles-copy">Add an ACP stdio executable using an absolute path and
      exact argument array. Interface never invokes a shell or accepts environment variables here.
      Authentication stays inside the provider&apos;s own login flow; do not put tokens or secrets in arguments.</p>
    {drafts.length === 0
      ? <div className="acp-profiles-empty">No custom ACP providers configured.</div>
      : drafts.map((draft, index) => <div className="acp-profile" key={draft.uiId}>
          <h4 className="acp-profile-summary">
            <span className="acp-profile-summary-mark" aria-hidden="true">
              <ProviderLogo engine={`acp:${draft.slug}`} label={draft.displayName || draft.slug || "Custom"}
                size={20} />
            </span>
            {draft.displayName.trim() || draft.slug || "New provider"}
          </h4>
          <div className="acp-profile-grid">
            {FIELDS.map(spec => renderField(draft, index, spec))}
          </div>
          <div className="acp-profile-actions">
            <button type="button" className="acp-profiles-button" disabled={pending}
              onClick={() => {
                setDrafts(current => {
                  const remaining = current.filter((_, at) => at !== index);
                  const neighbor = remaining[Math.min(index, remaining.length - 1)];
                  pendingFocus.current = neighbor === undefined
                    ? "add" : fieldKey(neighbor.uiId, "slug");
                  return remaining;
                });
                setDirty(true);
                setSubmitted(false);
                setLocalError(null);
              }}>Remove</button>
          </div>
        </div>)}
    {(localError ?? error) !== null && <p className="acp-profiles-error" role="alert">
      {localError ?? error}
    </p>}
    <footer className="acp-profiles-footer">
      <span ref={statusRef} className="acp-profiles-status" role="status" tabIndex={-1}
        aria-live="polite" aria-atomic="true" aria-busy={pending || undefined}>
        {pending ? "Saving provider roster…"
          : dirty ? "Unsaved provider roster changes."
            : restartRequired ? "Saved. Restart Interface to activate the new provider roster."
              : "Provider roster is saved."}
      </span>
      <button type="button" className="acp-profiles-button" disabled={!dirty || pending}
        onClick={save}>{pending ? "Saving…" : "Save roster"}</button>
    </footer>
  </section>;
}

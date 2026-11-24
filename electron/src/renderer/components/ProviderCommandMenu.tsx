import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ProviderCommand } from "../../shared/engine";

export const PROVIDER_COMMAND_MENU_CSS = `
.provider-command-menu { position:absolute; z-index:7; left:-1px; right:-1px;
  bottom:calc(100% + 6px); max-height:min(320px, 45vh); overflow-y:auto;
  padding:5px; background:var(--dropdown-bg);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-overlay);
  box-shadow:var(--overlay-shadow); container:provider-command-menu / inline-size;
  transform-origin:50% 100%;
  animation:provider-command-menu-in var(--motion-base) var(--ease-out) both; }
.provider-command-menu.is-inline { position:relative; left:auto; right:auto; bottom:auto;
  flex:none; width:auto; max-height:min(248px, 36vh); margin:0 7px 4px;
  box-sizing:border-box; }
.provider-command-menu-head { display:flex; align-items:center; justify-content:space-between;
  gap:10px; min-height:24px; padding:1px 7px 4px; color:var(--fg-dim);
  font-size:var(--text-meta); font-weight:var(--wght-strong); }
.provider-command-menu-hint { font-weight:var(--wght-regular); }
.provider-command-option { display:grid; grid-template-columns:minmax(0, 40%) minmax(0, 1fr);
  align-items:center; gap:10px; width:100%; min-height:36px; padding:5px 8px;
  color:var(--fg); font:inherit; text-align:left; background:transparent; border:0;
  border-radius:var(--radius-control); cursor:pointer;
  transition:color var(--motion-fast) linear,
    background-color var(--motion-fast) linear; }
.provider-command-option:hover, .provider-command-option.is-active {
  color:var(--fg-strong); background:var(--row-selected); }
.provider-command-name { display:block; min-width:0; overflow:hidden; text-overflow:ellipsis;
  color:var(--accent-text); font-family:var(--font-mono); font-size:var(--text-ui); white-space:nowrap; }
.provider-command-argument { margin-left:5px; color:var(--fg-selected-secondary); }
.provider-command-description { min-width:0; color:var(--fg-selected-secondary);
  font-size:var(--text-meta); line-height:var(--lh-ui); overflow:hidden; overflow-wrap:anywhere;
  display:-webkit-box; -webkit-box-orient:vertical; -webkit-line-clamp:2; }
.provider-command-sr-only { position:absolute; width:1px; height:1px; padding:0;
  margin:-1px; overflow:hidden; clip:rect(0, 0, 0, 0); white-space:nowrap; border:0; }
.provider-command-empty { margin:0; padding:10px 8px; color:var(--fg-muted);
  font-size:var(--text-meta); line-height:var(--lh-ui); }
@keyframes provider-command-menu-in {
  from { opacity:0; transform:translateY(5px) scale(.99); }
  to { opacity:1; transform:translateY(0) scale(1); }
}
@media (prefers-reduced-motion: reduce) {
  .provider-command-menu { animation:none; }
  .provider-command-option { transition:none; }
}
@container provider-command-menu (max-width:420px) {
  .provider-command-option { grid-template-columns:minmax(0, 1fr); gap:2px; }
}
`;

export interface ProviderCommandMenuProps {
  id: string;
  providerLabel: string;
  commands: readonly ProviderCommand[];
  activeIndex: number;
  onChoose: (command: ProviderCommand) => void;
  /** Enter-equivalent. Commands without an argument hint run on click. */
  onSubmit?: (command: ProviderCommand) => void;
  onActiveIndexChange: (index: number) => void;
  /** New Chat sits inside a clipped card, so its menu participates in layout. */
  placement?: "above" | "inline";
  /** Distinguishes loading/unavailable from a ready catalogue with no matches. */
  emptyLabel?: string;
}

export function providerCommandOptionId(menuId: string, index: number): string {
  return `${menuId}-option-${index}`;
}

export function providerCommandMenuAvailableHeight(anchorTop: number,
    clippingTop: number): number {
  return Math.max(0, Math.min(320, Math.floor(anchorTop - clippingTop - 12)));
}

/** Keep provider prose searchable and accessible while giving the menu a scan-friendly surface. */
export function providerCommandDescriptionPreview(description: string, max = 120): string {
  const normalized = description.replace(/\s+/g, " ").trim();
  if (normalized === "" || normalized.toLowerCase() === "provider command") return "";
  const boundedMax = Math.max(24, Math.floor(max));
  const firstSentence = /^(.+?[.!?])(?:\s|$)/.exec(normalized)?.[1];
  const candidate = firstSentence !== undefined && firstSentence.length >= 20
    ? firstSentence : normalized;
  if (candidate.length <= boundedMax) return candidate;
  return `${candidate.slice(0, boundedMax - 1).trimEnd().replace(/[,:;.!?]+$/u, "")}…`;
}

export function ProviderCommandMenu(props: ProviderCommandMenuProps) {
  const root = useRef<HTMLDivElement>(null);
  const options = useRef(new Map<number, HTMLButtonElement>());
  const [availableHeight, setAvailableHeight] = useState<number | null>(null);
  useLayoutEffect(() => {
    if (props.placement === "inline") {
      setAvailableHeight(null);
      return;
    }
    const menu = root.current;
    const anchor = menu?.parentElement;
    const clipping = menu?.closest(".workspace-tile-body") ?? menu?.closest(".conv");
    if (menu === null || anchor == null || clipping == null) return;
    const measure = (): void => {
      const next = providerCommandMenuAvailableHeight(
        anchor.getBoundingClientRect().top, clipping.getBoundingClientRect().top);
      setAvailableHeight(current => current === next ? current : next);
    };
    measure();
    const observer = typeof ResizeObserver === "undefined"
      ? null : new ResizeObserver(measure);
    observer?.observe(anchor);
    observer?.observe(clipping);
    window.addEventListener("resize", measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [props.commands.length, props.placement]);
  useEffect(() => {
    options.current.get(props.activeIndex)?.scrollIntoView({ block: "nearest" });
  }, [props.activeIndex, props.commands]);
  return (
    <div ref={root} id={props.id}
      className={`provider-command-menu is-floating${props.placement === "inline" ? " is-inline" : ""}`}
      role="listbox"
      style={availableHeight === null ? undefined : { maxHeight: availableHeight }}
      aria-label={`${props.providerLabel} commands`}>
      <style>{PROVIDER_COMMAND_MENU_CSS}</style>
      <div className="provider-command-menu-head" role="presentation" aria-hidden="true">
        <span>{props.providerLabel} commands</span>
        {props.commands.length > 0 && (
          <span className="provider-command-menu-hint">↑↓ Select · Enter run · Tab insert</span>
        )}
      </div>
      {props.commands.length === 0 ? (
        <p className="provider-command-empty" role="status" aria-live="polite">
          {props.emptyLabel ?? "No matching commands"}
        </p>
      ) : props.commands.map((command, index) => {
        const preview = providerCommandDescriptionPreview(command.description);
        return (
        <button key={command.name} id={providerCommandOptionId(props.id, index)} type="button"
          tabIndex={-1}
          ref={node => {
            if (node === null) options.current.delete(index);
            else options.current.set(index, node);
          }}
          role="option" aria-selected={index === props.activeIndex}
          className={`provider-command-option${index === props.activeIndex ? " is-active" : ""}`}
          onMouseEnter={() => { props.onActiveIndexChange(index); }}
          onMouseDown={event => { event.preventDefault(); }}
          onClick={() => {
            if (props.onSubmit !== undefined && command.argumentHint === undefined) {
              props.onSubmit(command);
            } else {
              props.onChoose(command);
            }
          }}>
          <span className="provider-command-name"
            title={`/${command.name}${command.argumentHint ? ` ${command.argumentHint}` : ""}`}>
            /{command.name}
            {command.argumentHint && (
              <span className="provider-command-argument">{command.argumentHint}</span>
            )}
          </span>
          {preview !== "" && <span className="provider-command-description"
            title={command.description}>
            <span aria-hidden="true">{preview}</span>
            <span className="provider-command-sr-only">{command.description}</span>
          </span>}
        </button>
        );
      })}
    </div>
  );
}

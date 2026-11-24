import { APP_SELECT_CSS } from "../components/AppSelect";
import { useState } from "react";
import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { CustomizationSection, SETTINGS_PAGE_CSS } from "../components/SettingsPage";
import type { CustomizationProvider, ProviderCustomization } from "../../shared/ipc";
import { assert, frames, until } from "./fixture-support";

export async function customizationFixture(root: Root): Promise<unknown> {
  flushSync(() => root.render(null));
  document.documentElement.dataset.theme = "dark";
  document.documentElement.dataset.chrome = "refined";
  const sources: string[] = [], checks: string[] = [];
  function Harness() {
    const [provider, setProvider] = useState<CustomizationProvider>("claude");
    const [health, setHealth] = useState<ProviderCustomization["health"]>();
    const path = `/provider/${provider}/settings.json`;
    return <div style={{ padding: 24, height: "100vh", overflow: "auto" }}>
      <style>{SETTINGS_PAGE_CSS + APP_SELECT_CSS}</style>
      <CustomizationSection id="customization-fixture" root="/project" userRoot={`/provider/${provider}`}
        provider={provider} onProviderChange={setProvider} onRefresh={() => { setHealth(undefined); }}
        entries={[{ name: `${provider}-docs`, kind: "mcp", detail: "stdio", scope: "user", sourcePath: path }]}
        health={health} onSource={entry => { sources.push(entry.sourcePath); }}
        onCheck={entry => { checks.push(entry.name); setHealth({ path, name: entry.name, state: "ready", message: "Temporary connection answered; existing session loading is unknown." }); }} />
    </div>;
  }
  flushSync(() => root.render(<Harness />));
  await frames(2);
  const select = document.querySelector<HTMLButtonElement>('button[aria-label^="Customization provider"]');
  assert(select, "Provider picker missing");
  select.focus();
  flushSync(() => select.click());
  await until(() => document.querySelector('[role="option"]') !== null, "Provider options missing");
  const codex = [...document.querySelectorAll<HTMLElement>('[role="option"]')].find(item => item.textContent?.includes("Codex"));
  assert(codex, "Codex option missing"); codex.click();
  await until(() => document.body.textContent?.includes("codex-docs") === true, "Provider switch did not render");
  document.querySelector<HTMLButtonElement>('[aria-label="Reveal source for codex-docs"]')?.click();
  document.querySelector<HTMLButtonElement>('[aria-label="Check MCP server codex-docs"]')?.click();
  await until(() => document.querySelector('[role="status"]')?.textContent?.includes("ready") === true, "Probe status did not render");
  assert(sources[0] === "/provider/codex/settings.json", "Source link used wrong provider");
  assert(checks[0] === "codex-docs", "Check used wrong server");
  assert(document.body.textContent?.includes("whether a running session has loaded them is unknown"), "Discovery overclaims loading");
  await frames(2);
  const overflow = document.documentElement.scrollWidth > window.innerWidth;
  assert(!overflow, "Customization overflowed the viewport");
  return { provider: "codex", sources, checks, loaded: "unknown", overflow };
}

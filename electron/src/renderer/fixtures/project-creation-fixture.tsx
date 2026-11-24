import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { CreateProjectDialog } from "../components/CreateProjectDialog";
import { Sidebar } from "../components/Sidebar";
import { assert, frames as settle, noop } from "./fixture-support";

/** UI contract; the hands-free full-app check separately exercises the Rust filesystem effect. */
export async function projectCreationFixture(root: Root): Promise<void> {
  let open = false;
  let workspaces = 0;
  const names: string[] = [];
  let finish: (() => void) | undefined;
  let created = false;
  let folder = { version: 1 as const, path: "/Users/fixture/Developer/Projects",
    display: "~/Developer/Projects", source: "default" as "default" | "chosen" };
  let changes = 0;
  const returnFocusRef = { current: null as HTMLElement | null };
  const render = (): void => flushSync(() => root.render(<div className="app-grid sidebar-overlay">
    <Sidebar groups={[]} rateLimits={{}} hidden={{ external: 0, trivial: 0 }} hideable={0}
      codeWorkspaces sectionLabel="Workspaces" newLabel="New workspace"
      backgroundInert={open} onSelect={noop} onNewInProject={noop} onArchive={noop} onToggleFilters={noop}
      onNew={() => { workspaces++; }} onNewProject={() => {
        returnFocusRef.current = document.activeElement as HTMLElement;
        open = true; render();
      }} />
    {open && <CreateProjectDialog returnFocusRef={returnFocusRef} folder={folder}
      onChangeFolder={async () => {
        changes++;
        folder = { version: 1, path: "/Users/fixture/code", display: "~/code", source: "chosen" };
        render();
      }}
      onCancel={() => { open = false; render(); }} onCreate={async name => {
        names.push(name);
        if (names.length === 1) throw new Error("That project name is already in use.");
        await new Promise<void>(resolve => { finish = resolve; });
        created = true;
        open = false; render();
      }} />}
  </div>));
  render(); await settle();
  const get = <T extends HTMLElement>(selector: string): T => {
    const node = document.querySelector<T>(selector);
    assert(node, `Project creation control missing: ${selector}`);
    return node;
  };
  const plus = () => get<HTMLButtonElement>('.sidebar-nav-button[aria-label="New workspace"]');
  const menuItem = (text: string): HTMLButtonElement => {
    const node = [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
      .find(item => item.textContent?.trim() === text);
    assert(node, `Project creation menu item missing: ${text}`); return node;
  };
  const openDialog = async () => {
    plus().focus(); plus().click(); await settle();
    menuItem("New project…").click(); await settle();
    assert(document.activeElement === get(".create-project-input"), "Project name did not receive focus");
  };
  const typeName = (text: string) => {
    const input = get<HTMLInputElement>(".create-project-input");
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  };
  plus().focus(); plus().click(); await settle();
  assert(plus().getAttribute("aria-expanded") === "true", "Plus did not open creation menu");
  menuItem("New workspace").click(); await settle();
  assert(workspaces === 1 && !open, "Existing workspace action changed behavior");
  await openDialog();
  assert(get<HTMLButtonElement>('[type="submit"]').disabled, "Empty name enabled creation");
  get(".create-project-input").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await settle();
  assert(!open && names.length === 0 && document.activeElement === plus(), "Cancel created a project or lost opener focus");
  await openDialog();
  assert(get(".create-project-path").textContent === "~/Developer/Projects/project-name",
    "The path line does not name the Projects folder");
  get<HTMLButtonElement>(".create-project-change").click(); await settle();
  assert(changes === 1 && get(".create-project-path").textContent === "~/code/project-name",
    "Change… did not move the project to the chosen folder");
  typeName("Existing"); await settle();
  get<HTMLButtonElement>('[type="submit"]').click(); await settle();
  assert(get('[role="alert"]').textContent?.includes("already in use"), "Creation failure was hidden");
  assert(get<HTMLInputElement>(".create-project-input").value === "Existing" && !created, "Failure lost name or opened workspace");
  typeName("My new project"); await settle();
  get<HTMLButtonElement>('[type="submit"]').click();
  get(".create-project-dialog").dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  await settle();
  assert(names.join("|") === "Existing|My new project", "Double submission created two requests");
  assert(get<HTMLInputElement>(".create-project-input").disabled, "Name editable during creation");
  get(".create-project-dialog").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await settle();
  assert(open && !created, "Pending creation was dismissed or completed early");
  const box = get(".create-project-dialog").getBoundingClientRect();
  assert(box.left >= 0 && box.right <= innerWidth && box.bottom <= innerHeight, "Project dialog overflows the viewport");
  assert(finish, "Creation never started"); finish(); await settle();
  assert(created && !document.querySelector(".create-project-dialog"), "Successful creation did not close dialog");
  flushSync(() => root.render(null));
}

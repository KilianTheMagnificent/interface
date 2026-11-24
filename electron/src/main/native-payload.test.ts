import { describe, expect, it, vi } from "vitest";
import { createNativePayloadHook } from "./native-payload.js";
import { CHANNELS } from "../shared/ipc.js";

describe("native Electron payload context", () => {
  it("adds the current system appearance without moving preference storage to Electron", async () => {
    const hook = createNativePayloadHook({
      systemDark: () => true,
      defaultDirectory: async () => "/work",
      pickDirectory: async () => null,
      projectsFolder: async () => "/work/projects",
    });
    await expect(hook(CHANNELS.getAppearance, {})).resolves.toEqual({ systemDark: true, surfaceVersion: 6 });
    await expect(hook(CHANNELS.setAppearance, { preference: "light" }))
      .resolves.toEqual({ preference: "light", systemDark: true, surfaceVersion: 6 });
  });

  it("shows the picker first, then sends only its selection to Rust authority", async () => {
    const pickDirectory = vi.fn(async () => "/work/chosen");
    const hook = createNativePayloadHook({
      systemDark: () => false,
      defaultDirectory: async () => "/work/default",
      pickDirectory,
      projectsFolder: async () => "/work/projects",
    });
    await expect(hook(CHANNELS.pickDirectory, {}))
      .resolves.toEqual({ selectedPath: "/work/chosen" });
    expect(pickDirectory).toHaveBeenCalledWith("/work/default");
  });

  it("starts the picker at a preferred path when the renderer supplies one", async () => {
    const pickDirectory = vi.fn(async () => "/work/chosen");
    const hook = createNativePayloadHook({
      systemDark: () => false,
      defaultDirectory: async () => "/work/default",
      pickDirectory,
      projectsFolder: async () => "/work/projects",
    });
    await expect(hook(CHANNELS.pickDirectory, { preferredPath: "/work/last" }))
      .resolves.toEqual({ selectedPath: "/work/chosen" });
    expect(pickDirectory).toHaveBeenCalledWith("/work/last");
    expect(pickDirectory).not.toHaveBeenCalledWith("/work/default");
  });

  it("ignores a blank preferred path and keeps the host default", async () => {
    const pickDirectory = vi.fn(async () => null);
    const hook = createNativePayloadHook({
      systemDark: () => false,
      defaultDirectory: async () => "/work/default",
      pickDirectory,
      projectsFolder: async () => "/work/projects",
    });
    await expect(hook(CHANNELS.pickDirectory, { preferredPath: "   " }))
      .resolves.toEqual({ selectedPath: null });
    expect(pickDirectory).toHaveBeenCalledWith("/work/default");
  });

  it("preserves cancellation and leaves ordinary Rust requests untouched", async () => {
    const hook = createNativePayloadHook({
      systemDark: () => false,
      defaultDirectory: async () => "/work",
      pickDirectory: async () => null,
      projectsFolder: async () => "/work/projects",
    });
    await expect(hook(CHANNELS.pickDirectory, {})).resolves.toEqual({ selectedPath: null });
    const payload = { sessionKey: "codex#one", text: "hello" };
    await expect(hook(CHANNELS.send, payload)).resolves.toBe(payload);
  });

  it("answers Change… in New project with the native picker's choice only", async () => {
    const pickDirectory = vi.fn(async () => "/work/code");
    const hook = createNativePayloadHook({
      systemDark: () => false,
      defaultDirectory: async () => "/work/default",
      pickDirectory,
      projectsFolder: async () => "/work/projects",
    });
    await expect(hook(CHANNELS.createProject, { projectsFolder: "choose", selectedPath: "/etc" }))
      .resolves.toEqual({ projectsFolder: "choose", selectedPath: "/work/code" });
    expect(pickDirectory).toHaveBeenCalledWith("/work/projects");
    // A read and a create pass through untouched, with no picker.
    await expect(hook(CHANNELS.createProject, { projectsFolder: "read" }))
      .resolves.toEqual({ projectsFolder: "read" });
    await expect(hook(CHANNELS.createProject, { name: "web" })).resolves.toEqual({ name: "web" });
    expect(pickDirectory).toHaveBeenCalledTimes(1);
  });
});

import { CHANNELS } from "../shared/ipc.js";
import { plainObject } from "./json.js";

/** Private Electron-parent operation; intentionally outside the public preload API. */
export const NATIVE_DIRECTORY_PICKER_START_PATH = "iface:nativeDirectoryPickerStartPath";

export type NativePayloadContext = {
  systemDark(): boolean;
  defaultDirectory(): Promise<string>;
  pickDirectory(defaultPath: string): Promise<string | null>;
  /** Where New project creates now: the picker's start for Change…. */
  projectsFolder(): Promise<string>;
};

function objectPayload(operation: string, payload: unknown): Record<string, unknown> {
  const named = plainObject(payload);
  if (named === undefined) {
    throw new TypeError(`${operation} requires a named-object payload.`);
  }
  return named;
}

/** Injects Electron-only observations/effects before Rust validates the request. */
export function createNativePayloadHook(context: NativePayloadContext) {
  return async (operation: string, payload: unknown): Promise<unknown> => {
    if (operation === CHANNELS.getAppearance || operation === CHANNELS.setAppearance) {
      return { ...objectPayload(operation, payload), systemDark: context.systemDark(), surfaceVersion: 6 };
    }
    if (operation === CHANNELS.pickDirectory) {
      const named = objectPayload(operation, payload);
      const preferred = typeof named.preferredPath === "string" ? named.preferredPath.trim() : "";
      const defaultPath = preferred === "" ? await context.defaultDirectory() : preferred;
      const selectedPath = await context.pickDirectory(defaultPath);
      return { selectedPath };
    }
    if (operation === CHANNELS.createProject) {
      const named = plainObject(payload);
      // Change… in New project: only the native picker's answer reaches Rust.
      if (named?.projectsFolder === "choose") {
        const selectedPath = await context.pickDirectory(await context.projectsFolder());
        return { projectsFolder: "choose", selectedPath };
      }
    }
    return payload;
  };
}

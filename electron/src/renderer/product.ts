export type BuildChannel = "dev" | "release";

export function readBuildChannel(): BuildChannel {
  const host = typeof globalThis.window === "object" && globalThis.window !== null
    ? globalThis.window as Window & { ifaceBuild?: { channel?: unknown } }
    : undefined;
  return host?.ifaceBuild?.channel === "dev" ? "dev" : "release";
}

export function isDevBuild(): boolean {
  return readBuildChannel() === "dev";
}

export function productLabel(): string {
  return isDevBuild() ? "Interface Dev" : "Interface";
}

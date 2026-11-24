export type BuildChannel = "dev" | "release";

/** Preload reads this from `process.argv`. Keep the literal in preload.cts too. */
export const BUILD_CHANNEL_ARG_PREFIX = "--interface-build-channel=";

export function productTitle(channel: BuildChannel): string {
  return channel === "dev" ? "Interface Dev" : "Interface";
}

export function resolveBuildChannel(input: {
  readonly appName: string;
  readonly viteDevServerUrl?: string;
}): BuildChannel {
  if (input.viteDevServerUrl !== undefined && input.viteDevServerUrl.length > 0) {
    return "dev";
  }
  if (input.appName === "Interface Dev") return "dev";
  return "release";
}

export function buildChannelArgument(channel: BuildChannel): string {
  return `${BUILD_CHANNEL_ARG_PREFIX}${channel}`;
}

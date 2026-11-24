import { isAbsolute, join } from "node:path";

export type RustHostPathOptions = {
  isPackaged: boolean;
  mainDirectory: string;
  resourcesPath: string;
  override?: string;
  isFile(path: string): boolean;
};

/** Resolves only the supervised first-party host binary; PATH lookup is forbidden. */
export function resolveRustHostPath(options: RustHostPathOptions): string {
  const override = options.override?.trim();
  const candidate = override !== undefined && override.length > 0
    ? override
    : options.isPackaged
      ? join(options.resourcesPath, "bin", "interface-host")
      : join(options.mainDirectory, "..", "..", "..", "target", "debug", "interface-host");
  if (!isAbsolute(candidate) || !options.isFile(candidate)) {
    throw new Error("The bundled Interface Rust host is missing or is not a regular file.");
  }
  return candidate;
}

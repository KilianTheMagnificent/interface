import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

// The target directory is per checkout (`.cargo/config.toml` says why it must
// not be shared) unless `CARGO_TARGET_DIR` or a user-level cargo config moves
// it, so `<checkout>/target` is not always where cargo writes. Ask cargo where
// it actually builds.

const cache = new Map();

/**
 * The first executable file `name` on PATH, else in rustup's own bin
 * directory (`~/.cargo/bin`, which a `nohup` or launchd PATH lacks). Null when
 * neither has it.
 */
export function findExecutable(name) {
  const directories = [
    ...(process.env.PATH ?? "").split(path.delimiter).filter(entry => entry !== ""),
    path.join(os.homedir(), ".cargo", "bin"),
  ];
  for (const directory of directories) {
    const candidate = path.join(directory, name);
    try {
      // A directory can carry the execute bit too, and PATH entries that
      // shadow a tool with a same-named folder are exactly the confusing case.
      if (fs.statSync(candidate).isFile()) {
        fs.accessSync(candidate, fs.constants.X_OK);
        return candidate;
      }
    } catch { /* keep looking */ }
  }
  return null;
}

function askCargo(root, cargo) {
  const metadata = JSON.parse(execFileSync(cargo, [
    "metadata", "--format-version", "1", "--no-deps",
    "--manifest-path", path.join(root, "Cargo.toml"),
  ], { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }));
  const directory = metadata.target_directory;
  if (typeof directory !== "string" || directory.length === 0) {
    throw new Error("cargo metadata reported no target_directory");
  }
  return directory;
}

/**
 * The directory cargo builds `workspaceRoot` into. `CARGO_TARGET_DIR` wins when
 * the environment sets one, because cargo obeys it over every config file;
 * otherwise cargo itself answers. Only a machine with no cargo at all falls
 * back to `<workspaceRoot>/target`, which is what a plain checkout would use.
 * Pass `cargoPath` to reuse a cargo the caller already resolved.
 */
export function cargoTargetDirectory(workspaceRoot, cargoPath = null) {
  const root = path.resolve(workspaceRoot);
  const cached = cache.get(root);
  if (cached !== undefined) return cached;
  const fromEnv = (process.env.CARGO_TARGET_DIR ?? "").trim();
  const cargo = cargoPath ?? findExecutable("cargo");
  let resolved;
  if (fromEnv !== "") resolved = path.resolve(root, fromEnv);
  else if (cargo === null) resolved = path.join(root, "target");
  else resolved = askCargo(root, cargo);
  cache.set(root, resolved);
  return resolved;
}

/** A cargo-built binary, by profile and name, under the real target directory. */
export function cargoBinary(workspaceRoot, profile, name, cargoPath = null) {
  return path.join(cargoTargetDirectory(workspaceRoot, cargoPath), profile, name);
}

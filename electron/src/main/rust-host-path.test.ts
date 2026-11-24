import { describe, expect, it } from "vitest";
import { resolveRustHostPath } from "./rust-host-path.js";

describe("Rust host binary resolution", () => {
  it("uses the packaged first-party resource and never PATH", () => {
    expect(resolveRustHostPath({
      isPackaged: true,
      mainDirectory: "/Applications/Interface.app/Contents/Resources/app.asar/dist-main/main",
      resourcesPath: "/Applications/Interface.app/Contents/Resources",
      isFile: path => path === "/Applications/Interface.app/Contents/Resources/bin/interface-host",
    })).toBe("/Applications/Interface.app/Contents/Resources/bin/interface-host");
  });

  it("resolves the development workspace target from compiled main", () => {
    expect(resolveRustHostPath({
      isPackaged: false,
      mainDirectory: "/repo/electron/dist-main/main",
      resourcesPath: "/unused",
      isFile: path => path === "/repo/target/debug/interface-host",
    })).toBe("/repo/target/debug/interface-host");
  });

  it("permits only an existing absolute explicit override", () => {
    expect(resolveRustHostPath({
      isPackaged: false,
      mainDirectory: "/repo/electron/dist-main/main",
      resourcesPath: "/unused",
      override: "/private/build/interface-host",
      isFile: path => path === "/private/build/interface-host",
    })).toBe("/private/build/interface-host");
    expect(() => resolveRustHostPath({
      isPackaged: false,
      mainDirectory: "/repo/electron/dist-main/main",
      resourcesPath: "/unused",
      override: "interface-host",
      isFile: () => true,
    })).toThrow(/missing or is not a regular file/);
  });
});

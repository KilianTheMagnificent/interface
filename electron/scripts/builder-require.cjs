"use strict";

// This package declares electron-builder, not the packages it brings along.
// @electron/osx-sign, @electron/asar and app-builder-lib's own files resolve
// from here only while npm happens to hoist them to the top of node_modules; an
// electron-builder upgrade that stops hoisting one would break macOS signing,
// the Dev bundle and the release tests with "module not found", far from the
// cause. Resolve them the way electron-builder does, through app-builder-lib.

const { createRequire } = require("node:module");

/** A require() that resolves from electron-builder's own app-builder-lib. */
function appBuilderLibRequire() {
  const electronBuilder = createRequire(require.resolve("electron-builder/package.json"));
  return createRequire(electronBuilder.resolve("app-builder-lib/package.json"));
}

module.exports = { appBuilderLibRequire };

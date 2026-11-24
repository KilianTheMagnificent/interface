"use strict";

// electron-builder's custom macOS signer. Every build is signed with the
// hardened runtime and Apple's secure timestamp, and the app bundle carries an
// explicit designated requirement that pins the signing team rather than the
// certificate's name: installed copies accept an update only when it satisfies
// the running app's designated requirement, and a team survives a certificate
// renewal and a later move to a Developer ID certificate of the same team.

const { execFile } = require("node:child_process");
const { X509Certificate } = require("node:crypto");
const { mkdtemp, rename, rm } = require("node:fs/promises");
const path = require("node:path");
const { promisify } = require("node:util");
const { appBuilderLibRequire } = require("./builder-require.cjs");

const { signAsync } = appBuilderLibRequire()("@electron/osx-sign");

const execFileAsync = promisify(execFile);
const APPLE_DEVELOPMENT_IDENTITY = /^Apple Development:/;
const KNOWN_CERT_PREFIX = /^(?:Apple Development|Developer ID Application|Developer ID Installer|Apple Distribution|3rd Party Mac Developer Application|3rd Party Mac Developer Installer|Mac Developer):/;
const KEYCHAIN_IDENTITY_LINE = /^\s*\d+\)\s+([0-9A-Fa-f]{40})\s+"([^"]+)"$/;
const PRIVATE_TMP_PREFIX = `/private/tmp${path.sep}`;
const CERTIFICATE_HASH = /^[0-9A-F]{40}$/;
const PEM_CERTIFICATE = /-----BEGIN CERTIFICATE-----[\s\S]+?-----END CERTIFICATE-----/gu;
const TEAM_IDENTIFIER = /^[A-Z0-9]{10}$/;
const BUNDLE_IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9.-]*$/;
const RESOURCE_BIN = path.join("Contents", "Resources", "bin");

/**
 * Entitlements of the first-party executables in Contents/Resources/bin, which
 * electron-builder would otherwise give the Electron helpers' JIT exceptions.
 * None of them runs JavaScript. A binary missing here stops signing, so a new
 * one never ships with entitlements nobody chose.
 */
const RESOURCE_BINARY_ENTITLEMENTS = Object.freeze({
  "interface-host": Object.freeze([]),
  iface: Object.freeze([]),
  // The voice assistant records from the microphone in this process.
  "interface-voice": Object.freeze(["com.apple.security.device.audio-input"]),
});

function nonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : undefined;
}

function isUnderPrivateTmp(filePath) {
  return typeof filePath === "string" && filePath.startsWith(PRIVATE_TMP_PREFIX);
}

function parseKeychainIdentities(output) {
  const seen = new Set();
  const identities = [];
  for (const line of String(output).split(/\r?\n/u)) {
    if (line.includes("CSSMERR_") || line.includes("(Missing required extension)")) continue;
    const match = KEYCHAIN_IDENTITY_LINE.exec(line);
    if (!match) continue;
    const hash = match[1].toUpperCase();
    const name = match[2];
    const key = `${hash}\n${name}`;
    if (seen.has(key)) continue;
    seen.add(key);
    identities.push({ hash, name });
  }
  return identities;
}

async function queryKeychainIdentities(keychain) {
  const args = ["find-identity", "-v", "-p", "codesigning"];
  if (typeof keychain === "string" && keychain.length > 0) args.push(keychain);
  const { stdout } = await execFileAsync("/usr/bin/security", args);
  return parseKeychainIdentities(stdout);
}

function matchIdentityEntry(identities, query) {
  const needle = nonEmptyString(query);
  if (needle === undefined) return undefined;
  const upper = needle.toUpperCase();
  return identities.find(entry => entry.hash === upper || entry.name === needle)
    ?? identities.find(entry => entry.name.includes(needle));
}

async function resolveKeychainIdentity(options, packager, deps = {}) {
  const env = deps.env ?? process.env;
  const listIdentities = deps.listIdentities ?? queryKeychainIdentities;
  const config = packager?.platformSpecificBuildOptions;
  const configured = nonEmptyString(config?.identity) ?? nonEmptyString(env?.CSC_NAME);
  const selected = nonEmptyString(options?.identity);
  if (configured !== undefined && KNOWN_CERT_PREFIX.test(configured) && selected !== undefined) {
    return { identity: selected, identityName: configured };
  }
  if (options?.platform !== "darwin") {
    return { identity: selected, identityName: configured ?? selected };
  }
  const identities = await listIdentities(options?.keychain);
  if (configured !== undefined) {
    const matched = matchIdentityEntry(identities, selected) ?? matchIdentityEntry(identities, configured);
    return {
      identity: selected ?? matched?.hash ?? configured,
      identityName: matched?.name ?? configured,
    };
  }
  // A build Apple does not notarize keeps the Apple Development identity every
  // copy so far was signed with, even once a Developer ID one is installed.
  if (config?.notarize === false) {
    const appleDev = identities.find(entry => APPLE_DEVELOPMENT_IDENTITY.test(entry.name));
    if (appleDev !== undefined) {
      return { identity: appleDev.hash, identityName: appleDev.name };
    }
  }
  const matched = matchIdentityEntry(identities, selected)
    ?? identities.find(entry => entry.name.startsWith("Developer ID Application:"))
    ?? identities.find(entry => APPLE_DEVELOPMENT_IDENTITY.test(entry.name));
  return {
    identity: matched?.hash ?? selected,
    identityName: matched?.name ?? selected,
  };
}

/**
 * Every macOS build is signed with the hardened runtime and a secure
 * timestamp; there is no untimestamped or unhardened variant. Apple
 * Development is the normal identity for a build Apple does not notarize.
 */
function resolveSigningPolicy(options, packager, resolvedIdentityName) {
  const config = packager?.platformSpecificBuildOptions;
  const configuredIdentity = nonEmptyString(resolvedIdentityName)
    ?? nonEmptyString(config?.identity)
    ?? nonEmptyString(process.env.CSC_NAME)
    ?? nonEmptyString(options?.identity);
  if (options?.platform !== "darwin") return { kind: "distribution" };
  if (config?.hardenedRuntime === false) {
    throw new Error(
      "Interface signs every macOS build with the hardened runtime; "
      + "electron-builder.yml must not set hardenedRuntime: false.",
    );
  }
  const appleDevelopment = typeof configuredIdentity === "string"
    && APPLE_DEVELOPMENT_IDENTITY.test(configuredIdentity);
  if (!appleDevelopment) return { kind: "distribution" };
  if (config?.notarize !== false) {
    throw new Error(
      "Apple notarizes only Developer ID builds; an Apple Development build "
      + "must set notarize: false.",
    );
  }
  return { kind: "apple-development" };
}

function certificateSubjectValues(subject, field) {
  const prefix = `${field}=`;
  return String(subject).split("\n")
    .filter(line => line.startsWith(prefix))
    .map(line => line.slice(prefix.length));
}

async function queryKeychainCertificates(name, keychain) {
  const args = ["find-certificate", "-a", "-p"];
  if (name !== undefined) args.push("-c", name);
  if (typeof keychain === "string" && keychain.length > 0) args.push(keychain);
  const { stdout } = await execFileAsync("/usr/bin/security", args, { maxBuffer: 16 * 1024 * 1024 });
  return stdout;
}

/**
 * The team (certificate subject OU) of the identity that signs this build,
 * read from that exact certificate: by its SHA-1 when the identity is a hash,
 * which is how electron-builder and the keychain name it, otherwise by its
 * full common name. Anything but exactly one certificate with exactly one
 * team-shaped OU stops signing.
 */
async function signingTeamIdentifier(resolved, keychain, deps = {}) {
  const findCertificates = deps.findCertificates ?? queryKeychainCertificates;
  const identity = nonEmptyString(resolved?.identity)?.toUpperCase();
  const name = nonEmptyString(resolved?.identityName);
  const byHash = identity !== undefined && CERTIFICATE_HASH.test(identity);
  if (!byHash && name === undefined) {
    throw new Error("No signing identity was resolved, so its team cannot be pinned.");
  }
  const pems = String(await findCertificates(byHash ? undefined : name, keychain))
    .match(PEM_CERTIFICATE) ?? [];
  const matches = new Map();
  for (const pem of pems) {
    const certificate = new X509Certificate(pem);
    const fingerprint = certificate.fingerprint.replaceAll(":", "").toUpperCase();
    const selected = byHash
      ? fingerprint === identity
      : certificateSubjectValues(certificate.subject, "CN").includes(name);
    if (selected) matches.set(fingerprint, certificate);
  }
  if (matches.size !== 1) {
    throw new Error(`Expected exactly one keychain certificate for the signing identity; found ${String(matches.size)}.`);
  }
  const [certificate] = matches.values();
  const teams = certificateSubjectValues(certificate.subject, "OU");
  if (teams.length !== 1 || !TEAM_IDENTIFIER.test(teams[0])) {
    throw new Error("The signing certificate does not name exactly one Apple team (subject OU).");
  }
  return teams[0];
}

/** The designated requirement every Interface app bundle is signed with. */
function teamPinnedRequirement(bundleIdentifier, teamIdentifier) {
  if (typeof bundleIdentifier !== "string" || !BUNDLE_IDENTIFIER.test(bundleIdentifier)) {
    throw new Error("The app's bundle identifier cannot be written into a signing requirement.");
  }
  if (typeof teamIdentifier !== "string" || !TEAM_IDENTIFIER.test(teamIdentifier)) {
    throw new Error("The signing team cannot be written into a signing requirement.");
  }
  return `designated => identifier "${bundleIdentifier}" and anchor apple generic `
    + `and certificate leaf[subject.OU] = "${teamIdentifier}"`;
}

async function readBundleIdentifier(appPath) {
  const { stdout } = await execFileAsync("/usr/bin/plutil", [
    "-extract", "CFBundleIdentifier", "raw", "-o", "-", "--",
    path.join(appPath, "Contents", "Info.plist"),
  ]);
  return stdout.trim();
}

/** The name of a first-party executable directly in Contents/Resources/bin. */
function resourceBinaryName(appPath, filePath) {
  const prefix = `${path.join(appPath, RESOURCE_BIN)}${path.sep}`;
  if (!filePath.startsWith(prefix)) return undefined;
  return filePath.slice(prefix.length);
}

function mapSigningOptions(options, packager, originalAppPath, cleanPath, resolved) {
  const identityName = typeof resolved === "string" ? resolved : resolved?.identityName;
  const resolvedIdentity = typeof resolved === "object" && resolved !== null ? resolved.identity : undefined;
  const requirement = typeof resolved === "object" && resolved !== null
    ? resolved.designatedRequirement : undefined;
  resolveSigningPolicy(options, packager, identityName);
  const darwin = options?.platform === "darwin";
  if (darwin && typeof requirement !== "string") {
    throw new Error("A macOS build is signed with its team-pinned designated requirement, and none was resolved.");
  }
  const toOriginalPath = (filePath) => {
    if (filePath === cleanPath) return originalAppPath;
    const prefix = `${cleanPath}${path.sep}`;
    return filePath.startsWith(prefix)
      ? `${originalAppPath}${filePath.slice(cleanPath.length)}`
      : filePath;
  };
  const originalOptionsForFile = options.optionsForFile;
  return {
    ...options,
    ...(resolvedIdentity !== undefined ? { identity: resolvedIdentity } : {}),
    app: cleanPath,
    ignore: typeof options.ignore === "function"
      ? filePath => options.ignore(toOriginalPath(filePath))
      : options.ignore,
    optionsForFile: (filePath) => {
      const perFile = typeof originalOptionsForFile === "function"
        ? originalOptionsForFile(toOriginalPath(filePath))
        : {};
      if (!darwin) return perFile;
      if (String(perFile?.timestamp ?? "").toLowerCase() === "none") {
        throw new Error("Interface never signs without Apple's secure timestamp.");
      }
      const signed = { ...perFile, hardenedRuntime: true };
      // Only the app bundle is pinned to the team: the updater and Squirrel.Mac
      // check an update against the running app's requirement. Nested code
      // keeps codesign's automatic requirement; the deep, strict verification
      // of the bundle still checks every piece of it.
      if (filePath === cleanPath) return { ...signed, requirements: `=${requirement}` };
      const binary = resourceBinaryName(cleanPath, filePath);
      if (binary === undefined) return signed;
      const entitlements = Object.hasOwn(RESOURCE_BINARY_ENTITLEMENTS, binary)
        ? RESOURCE_BINARY_ENTITLEMENTS[binary] : undefined;
      if (entitlements === undefined) {
        throw new Error(`${path.join(RESOURCE_BIN, binary)} has no entitlements chosen in sign-mac.cjs.`);
      }
      return { ...signed, entitlements: [...entitlements] };
    },
  };
}

/** electron-builder custom macOS signer. The input is generated output only. */
async function signMac(options, packager, deps = {}) {
  const appPath = options.app;
  if (typeof appPath !== "string" || !appPath.endsWith(".app")) {
    throw new TypeError("The macOS signing target must be an app bundle.");
  }
  const identity = await resolveKeychainIdentity(options, packager);
  let resolved = identity;
  if (options?.platform === "darwin") {
    const team = await signingTeamIdentifier(identity, options.keychain, deps);
    const bundleIdentifier = await (deps.readBundleIdentifier ?? readBundleIdentifier)(appPath);
    resolved = { ...identity, designatedRequirement: teamPinnedRequirement(bundleIdentifier, team) };
    process.stdout.write(
      `  • signing with the hardened runtime, secure timestamps and a requirement pinned to team ${team}\n`,
    );
  }
  if (isUnderPrivateTmp(appPath)) {
    await execFileAsync("/usr/bin/xattr", ["-cr", appPath]);
    const signingOptions = mapSigningOptions(options, packager, appPath, appPath, resolved);
    await signAsync(signingOptions);
    return;
  }
  const appStem = appPath.slice(0, -".app".length);
  const signingDirectory = await mkdtemp("/private/tmp/interface-rust-sign-");
  const cleanPath = path.join(signingDirectory, path.basename(appPath));
  const originalPath = `${appStem}.with-provenance.app`;
  await rm(originalPath, { recursive: true, force: true });
  try {
    await execFileAsync("/usr/bin/xattr", ["-cr", appPath]);
    await execFileAsync("/usr/bin/ditto", ["--norsrc", appPath, cleanPath]);
    process.stdout.write("  • copied generated bundle outside File Provider before code signing\n");

    const signingOptions = mapSigningOptions(options, packager, appPath, cleanPath, resolved);
    await signAsync(signingOptions);

    await rename(appPath, originalPath);
    try {
      await execFileAsync("/usr/bin/ditto", ["--norsrc", cleanPath, appPath]);
      await execFileAsync("/usr/bin/codesign", ["--verify", "--deep", "--strict", appPath]);
    } catch (error) {
      await rm(appPath, { recursive: true, force: true });
      await rename(originalPath, appPath).catch(() => undefined);
      throw error;
    }
    await rm(originalPath, { recursive: true, force: true });
  } finally {
    await rm(signingDirectory, { recursive: true, force: true });
  }
}

module.exports = signMac;
module.exports.RESOURCE_BINARY_ENTITLEMENTS = RESOURCE_BINARY_ENTITLEMENTS;
module.exports.isUnderPrivateTmp = isUnderPrivateTmp;
module.exports.mapSigningOptions = mapSigningOptions;
module.exports.parseKeychainIdentities = parseKeychainIdentities;
module.exports.resolveKeychainIdentity = resolveKeychainIdentity;
module.exports.resolveSigningPolicy = resolveSigningPolicy;
module.exports.signingTeamIdentifier = signingTeamIdentifier;
module.exports.teamPinnedRequirement = teamPinnedRequirement;

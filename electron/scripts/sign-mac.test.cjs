"use strict";

const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { X509Certificate } = require("node:crypto");
const { readFileSync } = require("node:fs");
const { mkdir, mkdtemp, readFile, rm, writeFile } = require("node:fs/promises");
const { tmpdir } = require("node:os");
const path = require("node:path");
const { afterEach, test } = require("node:test");
const { appBuilderLibRequire } = require("./builder-require.cjs");
const { signAsync } = appBuilderLibRequire()("@electron/osx-sign");
const osxSignUtil = appBuilderLibRequire()("@electron/osx-sign/dist/cjs/util");
const signMac = require("./sign-mac.cjs");

const roots = [];

afterEach(async () => {
  await Promise.all(roots.splice(0).map(root => rm(root, { recursive: true, force: true })));
});

// A throwaway self-signed certificate (its key was discarded) shaped like an
// Apple Development one: the team is the subject OU, not the id in the name.
const TEST_CERTIFICATE = `-----BEGIN CERTIFICATE-----
MIICcTCCAhegAwIBAgIUCeQ162tkI+i+JJYAL1038uvH8F4wCgYIKoZIzj0EAwIw
gYwxGjAYBgoJkiaJk/IsZAEBDApURVNUVUlEMDAxMTUwMwYDVQQDDCxBcHBsZSBE
ZXZlbG9wbWVudDogVGVzdCBCdWlsZGVyIChBQkNERTEyMzQ1KTETMBEGA1UECwwK
MUEyQjNDNEQ1RTEVMBMGA1UECgwMVGVzdCBCdWlsZGVyMQswCQYDVQQGEwJVUzAg
Fw0yNjExMjMyMzQ1NTJaGA8yMTI2MTAzMDIzNDU1MlowgYwxGjAYBgoJkiaJk/Is
ZAEBDApURVNUVUlEMDAxMTUwMwYDVQQDDCxBcHBsZSBEZXZlbG9wbWVudDogVGVz
dCBCdWlsZGVyIChBQkNERTEyMzQ1KTETMBEGA1UECwwKMUEyQjNDNEQ1RTEVMBMG
A1UECgwMVGVzdCBCdWlsZGVyMQswCQYDVQQGEwJVUzBZMBMGByqGSM49AgEGCCqG
SM49AwEHA0IABKd/IuREauRKzp1X0DwEUUdqm+yWGWoADCKQm0K+dYbBXiSA7+JT
cXjCahYYOlz+rSFas/FzGbnWBjkvGU2xoHijUzBRMB0GA1UdDgQWBBS+9TK1v/NL
zJteJ5Rea39uqeWOpjAfBgNVHSMEGDAWgBS+9TK1v/NLzJteJ5Rea39uqeWOpjAP
BgNVHRMBAf8EBTADAQH/MAoGCCqGSM49BAMCA0gAMEUCIDOTkNySSGk1Nu5WNDqN
AX7scPWq7th927F03E8xaRSiAiEA4PF1W4wI8GRWGiApdbQ4ZpM2TLHBJunPXvu4
DvkGLbU=
-----END CERTIFICATE-----
`;
const TEST_CERTIFICATE_NAME = "Apple Development: Test Builder (ABCDE12345)";
const TEST_CERTIFICATE_HASH = new X509Certificate(TEST_CERTIFICATE).fingerprint.replaceAll(":", "");
const TEST_TEAM = "1A2B3C4D5E";
const TEST_REQUIREMENT = 'designated => identifier "com.example.interface" and anchor apple generic '
  + `and certificate leaf[subject.OU] = "${TEST_TEAM}"`;

function packager(identity, overrides = {}) {
  return {
    platformSpecificBuildOptions: {
      identity,
      hardenedRuntime: true,
      notarize: false,
      ...overrides,
    },
  };
}

function baseOptions(app, optionsForFile) {
  return {
    app,
    identity: "TEST-IDENTITY",
    identityValidation: false,
    platform: "darwin",
    type: "distribution",
    strictVerify: true,
    preAutoEntitlements: false,
    preEmbedProvisioningProfile: false,
    optionsForFile,
  };
}

function resolvedIdentity(identityName = "Apple Development: Local Developer (TEAMID)") {
  return { identityName, designatedRequirement: TEST_REQUIREMENT };
}

test("Apple Development signs hardened, timestamped and un-notarized, and anything weaker fails closed", () => {
  const options = { platform: "darwin" };
  assert.deepEqual(signMac.resolveSigningPolicy(options, packager(
    "Apple Development: Local Developer (TEAMID)",
  )), { kind: "apple-development" });
  assert.deepEqual(signMac.resolveSigningPolicy(options, packager(
    "Developer ID Application: Distribution Developer (TEAMID)",
    { notarize: true },
  )), { kind: "distribution" });
  // The old untimestamped, unhardened local build is gone for every identity.
  for (const identity of [
    "Apple Development: Local Developer (TEAMID)",
    "Developer ID Application: Distribution Developer (TEAMID)",
  ]) {
    assert.throws(() => signMac.resolveSigningPolicy(options, packager(
      identity, { hardenedRuntime: false },
    )), /hardened runtime/);
  }
  assert.throws(() => signMac.resolveSigningPolicy(options, packager(
    "Apple Development: Local Developer (TEAMID)",
    { notarize: true },
  )), /Apple notarizes only Developer ID builds/);
});

test("path remapping keeps secure timestamps and pins the team on the app bundle only", () => {
  const original = "/work/release/mac-arm64/Interface.app";
  const clean = "/private/tmp/interface-sign-test/Interface.app";
  const seen = [];
  const options = baseOptions(original, filePath => {
    seen.push(filePath);
    return { entitlements: "/tmp/interface.entitlements", hardenedRuntime: true };
  });
  options.ignore = filePath => {
    seen.push(filePath);
    return filePath.endsWith("ignored.pak");
  };

  const mapped = signMac.mapSigningOptions(options, packager(
    "Apple Development: Local Developer (TEAMID)",
  ), original, clean, resolvedIdentity());
  assert.equal(mapped.app, clean);
  assert.equal(mapped.ignore(`${clean}/Contents/Resources/ignored.pak`), true);
  assert.deepEqual(mapped.optionsForFile(`${clean}/Contents/Resources/icudtl.dat`), {
    entitlements: "/tmp/interface.entitlements",
    hardenedRuntime: true,
  });
  assert.deepEqual(mapped.optionsForFile(clean), {
    entitlements: "/tmp/interface.entitlements",
    hardenedRuntime: true,
    requirements: `=${TEST_REQUIREMENT}`,
  });
  assert.deepEqual(seen, [
    `${original}/Contents/Resources/ignored.pak`,
    `${original}/Contents/Resources/icudtl.dat`,
    original,
  ]);
  assert.equal(signMac.isUnderPrivateTmp(clean), true);
  assert.equal(signMac.isUnderPrivateTmp(original), false);
});

test("a build is never signed without a secure timestamp or its team-pinned requirement", () => {
  const app = "/private/tmp/interface-sign-test/Interface.app";
  for (const timestamp of ["none", "NONE"]) {
    const mapped = signMac.mapSigningOptions(baseOptions(app, () => ({ timestamp })), packager(
      "Apple Development: Local Developer (TEAMID)",
    ), app, app, resolvedIdentity());
    assert.throws(() => mapped.optionsForFile(app), /secure timestamp/);
  }
  assert.throws(() => signMac.mapSigningOptions(baseOptions(app, () => ({})), packager(
    "Apple Development: Local Developer (TEAMID)",
  ), app, app, { identityName: "Apple Development: Local Developer (TEAMID)" }),
  /team-pinned designated requirement/);
});

test("Resources/bin executables get only the entitlements chosen for them", () => {
  const app = "/private/tmp/interface-sign-test/Interface.app";
  const mapped = signMac.mapSigningOptions(
    baseOptions(app, () => ({ entitlements: "/tmp/inherit.plist", hardenedRuntime: true })),
    packager("Apple Development: Local Developer (TEAMID)"), app, app, resolvedIdentity(),
  );
  const bin = name => `${app}/Contents/Resources/bin/${name}`;
  assert.deepEqual(mapped.optionsForFile(bin("interface-host")).entitlements, []);
  assert.deepEqual(mapped.optionsForFile(bin("iface")).entitlements, []);
  assert.deepEqual(mapped.optionsForFile(bin("interface-voice")).entitlements,
    ["com.apple.security.device.audio-input"]);
  assert.equal(mapped.optionsForFile(bin("interface-voice")).hardenedRuntime, true);
  assert.equal(mapped.optionsForFile(bin("interface-voice")).requirements, undefined);
  assert.throws(() => mapped.optionsForFile(bin("new-helper")), /no entitlements chosen/);
  assert.throws(() => mapped.optionsForFile(bin("nested/helper")), /no entitlements chosen/);
  // Electron's own nested code keeps electron-builder's inherit entitlements.
  assert.equal(mapped.optionsForFile(
    `${app}/Contents/Frameworks/Interface Helper (Renderer).app`,
  ).entitlements, "/tmp/inherit.plist");
});

test("the signing team is read from the identity's own certificate", async () => {
  const asked = [];
  const findCertificates = async (name, keychain) => {
    asked.push({ name, keychain });
    return `${TEST_CERTIFICATE}${TEST_CERTIFICATE}`;
  };
  assert.equal(await signMac.signingTeamIdentifier(
    { identity: TEST_CERTIFICATE_HASH.toLowerCase(), identityName: TEST_CERTIFICATE_NAME },
    "/tmp/build.keychain", { findCertificates },
  ), TEST_TEAM);
  assert.equal(await signMac.signingTeamIdentifier(
    { identityName: TEST_CERTIFICATE_NAME }, undefined, { findCertificates },
  ), TEST_TEAM);
  assert.deepEqual(asked, [
    { name: undefined, keychain: "/tmp/build.keychain" },
    { name: TEST_CERTIFICATE_NAME, keychain: undefined },
  ]);
  await assert.rejects(signMac.signingTeamIdentifier(
    { identity: "A".repeat(40) }, undefined, { findCertificates },
  ), /found 0/);
  await assert.rejects(signMac.signingTeamIdentifier(
    { identityName: "Apple Development: Test Builder" }, undefined, { findCertificates },
  ), /found 0/);
  await assert.rejects(signMac.signingTeamIdentifier({}, undefined, { findCertificates }),
    /No signing identity/);
});

test("the designated requirement pins exactly the identifier and the team", () => {
  assert.equal(signMac.teamPinnedRequirement("com.example.interface", TEST_TEAM), TEST_REQUIREMENT);
  for (const [identifier, team] of [
    ['com.example" or anchor apple', TEST_TEAM],
    ["com.example.interface", 'ABC" or true'],
    ["com.example.interface", "abcde12345"],
    ["com.example.interface", "ABCDE1234"],
    ["", TEST_TEAM],
    ["com.example.interface", undefined],
  ]) {
    assert.throws(() => signMac.teamPinnedRequirement(identifier, team), /signing requirement/);
  }
});

async function fakeElectronApp() {
  const root = await mkdtemp(path.join(tmpdir(), "interface-rust-sign-policy-test-"));
  roots.push(root);
  const app = path.join(root, "Interface.app");
  const resource = path.join(
    app,
    "Contents", "Frameworks", "Electron Framework.framework", "Resources", "icudtl.dat",
  );
  const voice = path.join(app, "Contents", "Resources", "bin", "interface-voice");
  for (const file of [resource, voice]) {
    await mkdir(path.dirname(file), { recursive: true });
    await writeFile(file, Buffer.alloc(64));
  }
  return { app, resource, voice };
}

test("electron-osx-sign emits the exact hardened, timestamped and team-pinned arguments", async () => {
  const originalExecFileAsync = osxSignUtil.execFileAsync;
  const calls = [];
  osxSignUtil.execFileAsync = async (command, args) => {
    calls.push({ command, args: [...args] });
    return "";
  };
  try {
    const fake = await fakeElectronApp();
    const perFile = () => ({
      entitlements: "/tmp/interface.entitlements",
      hardenedRuntime: true,
      additionalArguments: [],
    });
    await signAsync(signMac.mapSigningOptions(
      baseOptions(fake.app, perFile),
      packager("Apple Development: Local Developer (TEAMID)"),
      fake.app,
      fake.app,
      resolvedIdentity(),
    ));
    const signing = file => calls.find(candidate => candidate.args.at(-1) === file);
    assert.deepEqual(signing(fake.resource), {
      command: "codesign",
      args: [
        "--sign", "TEST-IDENTITY", "--force", "--timestamp", "--options", "runtime",
        "--entitlements", "/tmp/interface.entitlements", fake.resource,
      ],
    });
    assert.deepEqual(signing(fake.app), {
      command: "codesign",
      args: [
        "--sign", "TEST-IDENTITY", "--force", `-r=${TEST_REQUIREMENT}`, "--timestamp",
        "--options", "runtime", "--entitlements", "/tmp/interface.entitlements", fake.app,
      ],
    });
    const voice = signing(fake.voice).args;
    assert.deepEqual(voice.slice(0, 6),
      ["--sign", "TEST-IDENTITY", "--force", "--timestamp", "--options", "runtime"]);
    assert.equal(voice.at(-3), "--entitlements");
    assert.match(await readFile(voice.at(-2), "utf8"),
      /<key>com\.apple\.security\.device\.audio-input<\/key>\s*<true\/>/);
    assert.ok(calls.every(call => !call.args.some(arg => /^--timestamp=/.test(arg))));
  } finally {
    osxSignUtil.execFileAsync = originalExecFileAsync;
  }
});

function entitlementKeys(file) {
  const json = execFileSync("/usr/bin/plutil", ["-convert", "json", "-o", "-", "--",
    path.resolve(__dirname, "..", "build", file)], { encoding: "utf8" });
  const entitlements = JSON.parse(json);
  assert.ok(Object.values(entitlements).every(value => value === true), file);
  return Object.keys(entitlements).sort();
}

test("entitlements grant what the hardened runtime needs and never dyld or debugger access", () => {
  const app = entitlementKeys("entitlements.mac.plist");
  assert.deepEqual(app, [
    "com.apple.security.automation.apple-events",
    "com.apple.security.cs.allow-jit",
    "com.apple.security.cs.allow-unsigned-executable-memory",
    "com.apple.security.device.audio-input",
    "com.apple.security.device.camera",
    "com.apple.security.personal-information.addressbook",
    "com.apple.security.personal-information.calendars",
    "com.apple.security.personal-information.location",
    "com.apple.security.personal-information.photos-library",
  ]);
  const nested = entitlementKeys("entitlements.mac.inherit.plist");
  assert.deepEqual(nested, [
    "com.apple.security.cs.allow-jit",
    "com.apple.security.cs.allow-unsigned-executable-memory",
  ]);
  const binaries = Object.values(signMac.RESOURCE_BINARY_ENTITLEMENTS).flat();
  for (const key of [...app, ...nested, ...binaries]) {
    assert.doesNotMatch(key, /allow-dyld-environment-variables|get-task-allow|disable-library-validation/);
  }
  const builderSource = readFileSync(path.resolve(__dirname, "..", "electron-builder.yml"), "utf8");
  assert.match(builderSource, /^ {2}hardenedRuntime: true$/m);
  assert.match(builderSource, /^ {2}notarize: false$/m);
  assert.match(builderSource, /^ {2}entitlements: build\/entitlements\.mac\.plist$/m);
  assert.match(builderSource, /^ {2}entitlementsInherit: build\/entitlements\.mac\.inherit\.plist$/m);
  // Each personal-information entitlement has the text macOS shows when asking.
  for (const key of [
    "NSMicrophoneUsageDescription", "NSCameraUsageDescription", "NSAppleEventsUsageDescription",
    "NSContactsUsageDescription", "NSCalendarsUsageDescription", "NSLocationUsageDescription",
    "NSPhotoLibraryUsageDescription",
  ]) {
    assert.match(builderSource, new RegExp(`^ {4}${key}: \\S`, "m"), key);
  }
});

test("builder config holds no hardcoded signing identity or personal email", () => {
  const builderSource = readFileSync(path.resolve(__dirname, "..", "electron-builder.yml"), "utf8");
  assert.doesNotMatch(builderSource, /^ {2}identity:/m);
  assert.doesNotMatch(builderSource, /@[a-z0-9.-]+\.[a-z]{2,}/i);
});

test("resolves local Apple Development and CSC_NAME identities from keychain output", async () => {
  const keychainOutput = [
    '  1) AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA "Developer ID Application: Example Corp (TEAM123456)"',
    '  2) BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB "Apple Development: Local Builder (TEAM123456)"',
    '  3) CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC "Apple Development: Expired (TEAM123456)" (CSSMERR_TP_CERT_EXPIRED)',
    "     2 valid identities found",
  ].join("\n");
  const parsed = signMac.parseKeychainIdentities(keychainOutput);
  assert.deepEqual(parsed, [
    { hash: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", name: "Developer ID Application: Example Corp (TEAM123456)" },
    { hash: "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", name: "Apple Development: Local Builder (TEAM123456)" },
  ]);

  const listIdentities = async () => parsed;

  // Unset CSC_NAME and unset builder identity on an un-notarized build resolves Apple Development.
  const autoLocal = await signMac.resolveKeychainIdentity(
    { platform: "darwin", identity: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" },
    packager(undefined),
    { env: {}, listIdentities },
  );
  assert.deepEqual(autoLocal, {
    identity: "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
    identityName: "Apple Development: Local Builder (TEAM123456)",
  });
  const mappedLocal = signMac.mapSigningOptions(
    baseOptions("/out/Interface.app", () => ({ hardenedRuntime: true })),
    packager(undefined),
    "/out/Interface.app",
    "/private/tmp/Interface.app",
    { ...autoLocal, designatedRequirement: TEST_REQUIREMENT },
  );
  assert.equal(mappedLocal.identity, "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
  assert.deepEqual(mappedLocal.optionsForFile("/private/tmp/Interface.app"), {
    hardenedRuntime: true,
    requirements: `=${TEST_REQUIREMENT}`,
  });

  // Explicit CSC_NAME selecting Developer ID keeps secure distribution timestamps.
  const explicitDist = await signMac.resolveKeychainIdentity(
    { platform: "darwin", identity: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" },
    packager(undefined, { notarize: true }),
    { env: { CSC_NAME: "Example Corp (TEAM123456)" }, listIdentities },
  );
  assert.deepEqual(explicitDist, {
    identity: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    identityName: "Developer ID Application: Example Corp (TEAM123456)",
  });
  assert.deepEqual(
    signMac.resolveSigningPolicy({ platform: "darwin" }, packager(undefined, { notarize: true }), explicitDist.identityName),
    { kind: "distribution" },
  );

  // Explicit CSC_NAME resolving to Apple Development still fails closed if notarization is asked for.
  const notarizedDev = await signMac.resolveKeychainIdentity(
    { platform: "darwin", identity: "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB" },
    packager(undefined, { notarize: true }),
    { env: { CSC_NAME: "Local Builder (TEAM123456)" }, listIdentities },
  );
  assert.throws(
    () => signMac.resolveSigningPolicy({ platform: "darwin" }, packager(undefined, { notarize: true }), notarizedDev.identityName),
    /Apple notarizes only Developer ID builds/,
  );
});

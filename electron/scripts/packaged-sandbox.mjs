import path from "node:path";
import { homedir } from "node:os";

const quote = value => JSON.stringify(value);

/** All children inherit this Seatbelt policy. The application never receives
 * the owner's environment or a provider executable from the owner's PATH.
 * macOS refuses setuid /bin/ps in Seatbelt. Only that immutable read-only
 * system utility is exempt, so terminal identity checks still use real data. */
export function packagedSandboxPolicy(profile, ownerHome = homedir()) {
  if (!path.isAbsolute(profile) || !path.isAbsolute(ownerHome) || profile.startsWith(ownerHome + path.sep)) {
    throw new Error("The production test profile must be outside the owner home.");
  }
  return `(version 1)
(allow default)
(allow process-exec (with no-sandbox) (literal "/bin/ps"))
(deny file-write*)
(allow file-write* (subpath ${quote(profile)}) (subpath "/dev"))
(deny file-read* (subpath ${quote(ownerHome)}))
(deny network*)
(allow network-bind (local ip "localhost:*"))
(allow network-inbound (local ip "localhost:*"))
(allow network-outbound (remote ip "localhost:*"))
(allow network* (local unix-socket) (remote unix-socket))
`;
}

export function packagedSandboxEnvironment(profile) {
  const home = path.join(profile, "process-home");
  return {
    HOME: home, CFFIXED_USER_HOME: home,
    PATH: [path.join(profile, "bin"), "/usr/bin", "/bin", "/usr/sbin", "/sbin"].join(":"),
    SHELL: "/bin/zsh", LANG: "en_US.UTF-8", TERM: "xterm-256color",
    TMPDIR: path.join(profile, "tmp"),
    ZDOTDIR: home, HISTFILE: "/dev/null",
    XDG_CONFIG_HOME: path.join(home, ".config"),
    XDG_CACHE_HOME: path.join(home, ".cache"),
    XDG_DATA_HOME: path.join(home, ".local", "share"),
    INTERFACE_PACKAGED_SMOKE_PROFILE: profile,
    INTERFACE_HOST_SMOKE_DATA_ROOT: path.join(profile, "data"),
    INTERFACE_HOST_SMOKE_HOME: path.join(profile, "home"),
    INTERFACE_VOICE_HELPER: "/usr/bin/false",
  };
}

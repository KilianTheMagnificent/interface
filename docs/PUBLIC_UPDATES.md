# Public updates

How a copy of Interface on someone else's Mac updates itself, and how the
owner publishes a release to it. Built 21 Nov 2026. **The channel is live:**
the first public release, 0.3.182, went out on 24 Nov 2026 as an Apple
silicon (arm64) archive and a signed manifest on
[`interface-releases`](https://github.com/KilianTheMagnificent/interface-releases/releases/latest).
Builds check
`https://github.com/KilianTheMagnificent/interface-releases/releases/latest/download/interface-update.json`.
A build that finds nothing there (a "not found") reads that as up to date and
goes on updating from the feed `cargo xtask promote` stages on its own Mac.
Nothing newer than 0.3.182 exists yet, so no installed copy has taken a
public update (see "What was checked, and what was not").

## What happens on a user's Mac

1. About 45 seconds after launch, then every four hours, and whenever the user
   picks Check for updates, the Rust host fetches one small file over HTTPS:
   `interface-update.json`, the **signed manifest**.
2. The host checks the manifest's Ed25519 signature against the public keys
   compiled into the app. A manifest no trusted key signed is refused before
   any of its contents are read.
   A feed that answers "not found" has nothing published yet; that reads as
   up to date, not as a fault.
3. If the manifest names a version newer than the running one, the host
   downloads the archive it names from beside the manifest, and checks its
   size and SHA-512 against the signed values.
4. Only then does the archive go into `update-feed` in the app's data folder,
   as the same two files `cargo xtask stage-update` writes. From there the
   installer that has shipped every release so far takes over: electron-updater
   reads the loopback feed, Squirrel.Mac and the host both check that the new
   app is signed by the same developer as the running one and is a newer
   version, and Settings offers Restart to update.

The app never installs by itself; the user restarts when they choose.

## What this protects against

| If this happens | Result |
| --- | --- |
| The server or account hosting the files is taken over | It can stop updates reaching people. It cannot make anyone install anything: it has neither the manifest key nor the Apple signing identity. |
| Someone alters the archive in transit or on the host | Size or SHA-512 mismatch; refused, never handed to the installer. |
| Someone serves an old, genuine release to push people backwards | Refused: the version must be newer than the running app, and never older than a release the app has already seen signed. |
| Something on the Mac sets `INTERFACE_UPDATE_URL` | Ignored in a build with a channel. The feed address and keys are part of the binary. |
| The manifest key is stolen but not the Apple identity | The thief can sign a manifest, but the archive must still be an Interface build signed by the owner's Apple identity and newer than the installed one. |

Not covered: a thief with both the manifest key and the Apple signing
identity, which both live on the owner's Mac. Keep that Mac's disk encrypted
and its login private.

## Rules that must never be broken once a build is public

A copy installed today has to be able to read what is published years from
now. Breaking any of these strands every copy that is already out there, and
the only fix is asking each person to download again.

- **Keep the feed address working.** It is compiled into every build. If the
  hosting has to move, list the new address as a second feed first, ship that
  build, and keep the old address serving for as long as old copies exist.
- **Never lose every trusted key.** Make two (`primary` and `backup`), keep the
  backup's private file off this Mac, and list both public keys before the
  first public build.
- **Never change the manifest file's name or raise its `schema`.** New fields
  may be added; old apps ignore what they do not know. Anything incompatible
  is published as a second file beside this one.
- **Never change the bundle identifier or the Apple signing identity** between
  two public releases. The installer accepts an update only from the developer
  who signed the running app. Settle both before the first public build.
  Builds signed by `electron/scripts/sign-mac.cjs` since 23 Nov 2026 name that
  developer by Apple team, so a renewed certificate or a Developer ID
  certificate of the same team still updates them; builds before it name the
  certificate itself.
- **Never publish a lower version than the last one published.**

## Go-live

The owner's decisions: the bundle identifier stays `com.andreromero.interface`
and releases live on GitHub (21 Nov 2026); the source is published too, and
there is no paid Apple Developer Program, so no Developer ID certificate and
no notarization (23 Nov 2026).

What that means for a stranger's first install: macOS refuses to open an
un-notarized app that came through a browser until the person allows it once
(System Settings > Privacy & Security > Open Anyway). `install.sh` avoids the
refusal: a file fetched with `curl` carries no quarantine flag, so the app
opens normally, and the script checks the release's checksum and that the app
is signed by the owner's Apple team before it installs anything. Updates after
that never meet Gatekeeper; the app's own checks above apply.

Builds are signed with the owner's free Apple Development certificate, with
the hardened runtime and Apple's secure timestamp, and name their developer by
Apple team (`electron/scripts/sign-mac.cjs`). The certificate is renewed
yearly (this one runs to 30 Jul 2027); a renewed certificate of the same
team, or a Developer ID certificate of it later, still updates installed
copies. `package-mac`, `promote` and `publish-update` refuse a build that is
not signed that way.

Done (as of 25 Nov 2026, read from the live repositories):

- Releases repository `KilianTheMagnificent/interface-releases` is public and
  holds release `v0.3.182` (`Interface-0.3.182-arm64-mac.zip` and
  `interface-update.json`), published 24 Nov 2026.
- Two signing keys, `primary` and `backup`, in
  `~/.config/interface-release/`, both listed in
  `crates/interface-updater/release-channel.json` with the feed address. The
  live manifest is signed by the first of the two.
- The public copy of the source, `KilianTheMagnificent/interface`, is public,
  with pull requests limited to collaborators and private vulnerability
  reporting on. It is a separate repository with its own history, written by
  `python3 scripts/export_public.py "<public checkout>"` (the listed paths as
  committed; it stops on a personal address, the owner's home folder path, a
  dead link in the front files or an unknown key-shaped string). The private
  development repository stays private.

Still to do, each on the owner's word:

1. **Owner, any time:** copy
   `~/.config/interface-release/update-key-backup.pk8` somewhere that is not
   this Mac (a password manager, a USB stick in a drawer). It is only needed
   if `primary` is lost, and then it is the only way to keep updating
   installed copies. Nothing public shows whether this has been done.
2. Run the README's install line on a clean Mac, publish the next release,
   and watch it arrive through Restart to update. A virtual Mac serves:
   `tart clone ghcr.io/cirruslabs/macos-golden-gate-vanilla:<version> newmac`
   then `tart run newmac`. That image ships with Gatekeeper switched off, so
   run `sudo spctl --global-enable` in it first, or the result proves nothing.

## Each release

After `cargo xtask promote` has staged the release on this Mac:

    cargo xtask publish-update --allow-unnotarized --github <owner>/<repo>

It signs the staged release, runs the installed app's own update check
against the signed folder (signature, download, size, SHA-512), uploads both
files as a draft release, makes it the latest, then reads the live feed back
and downloads the archive to check it. Installed copies pick it up within
four hours.

Without `--github` it stops after writing the two files to
`release/public-feed/<version>/`, for uploading to any other static host.

To look at what is live at any time:

    cargo xtask publish-update --check
    cargo xtask publish-update --check --download

Publishing is its own owner-requested step, like promotion: an agent runs it
only when asked to publish.

## Changing keys

To retire a key, first ship a release, signed with a key old copies trust,
whose build lists the new key. Once nearly everyone has that release, later
builds may drop the old key. Copies that never took the bridging release can
no longer update.

## Where the code is

| What | Where |
| --- | --- |
| Feed address and trusted keys | `crates/interface-updater/release-channel.json`, read by `src/channel.rs` |
| Manifest format, signing and verifying | `crates/interface-updater/src/signed_manifest.rs` |
| Fetch, download, check, stage | `crates/interface-updater/src/remote.rs` |
| When checks run and what Settings shows | `crates/interface-updater/src/service.rs` (`check_channel`, `spawn_channel_schedule`) |
| Host wiring, environment feed ignored | `apps/interface-host/src/updater.rs` (`updater_service`) |
| `update-key`, `publish-update` | `xtask/src/update_channel.rs` |
| Reading the running app's designated requirement | `crates/interface-updater/src/signature.rs`, `src/macos.rs` (`read_mac_app_requirement`) |
| Signing: hardened runtime, secure timestamp, team-pinned requirement, entitlements | `electron/scripts/sign-mac.cjs`, `electron/build/entitlements.mac*.plist` |
| A package or published release must keep updates working | `xtask/src/package.rs` (`verify_public_signature`), `electron/scripts/smoke-packaged.mjs` |

The Electron side is unchanged: it still reads only the loopback feed.

## What was checked, and what was not

Checked on 21 Nov 2026: the unit tests of the three crates; the real 0.3.181
archive (145 MB) signed with a throwaway key, served over loopback HTTP and
taken by the app's update code byte for byte; the same run refused with an
untrusted key and with one byte of the archive flipped; the HTTPS client
against a real GitHub `releases/latest/download` redirect.

Also checked on 21 Nov 2026, after the channel was set up: `cargo xtask
publish-update --check` against the real address ("Nothing is published
there yet"), and the staged 0.3.181 signed and rehearsed with each of the two
real keys, without uploading.

Checked on 23 Nov 2026: a build packaged with the hardened runtime, secure
timestamps and the team-pinned requirement comes up with updates on and
satisfies the requirement of the installed 0.3.180, so that copy can update
to it; `install.sh` installed that build from a local folder standing in for
the feed, and refused unsigned and differently signed builds, a wrong
checksum and a release naming another download
(`python3 -m unittest scripts/test_install_script.py`); a stand-in app signed
the same way and fetched with `curl` opened with no Gatekeeper refusal on
macOS 27.3.

Checked on 25 Nov 2026 against the live release 0.3.182, from a throwaway
folder and a throwaway data folder (nothing on the checking Mac was touched):
the manifest at the `releases/latest/download` address is the one attached to
the release, and its Ed25519 signature verifies against the first of the two
keys in `release-channel.json` (the manifest names key id `b980dad7bffe8905`);
the archive's size (146,421,740 bytes) and SHA-512 match the signed values;
`cargo xtask publish-update --check --download` reports "signed by a trusted
key" and that the archive "downloads and matches its signed size and SHA-512";
the README's one-line install, run with `INTERFACE_INSTALL_DIR` and
`INTERFACE_NO_OPEN=1` against the real feed, installed `Interface.app` (arm64,
version 0.3.182, bundle id `com.andreromero.interface`, minimum macOS 12.0);
that copy passes `codesign --verify --deep --strict`, carries the hardened
runtime and a secure timestamp, is signed by team `36B25Z964W`, and is
rejected by `spctl` (as expected for an app that is not notarized). The
install tests and export tests (`python3 -m unittest discover -s scripts -p
'test_*.py'`) pass.

Not checked: a packaged app updating itself from the public address end to
end (0.3.182 is the only release, so no installed copy has anything newer to
take); a first launch on a different, clean Mac; behaviour behind a corporate
proxy that inspects TLS (the client trusts only the public web roots, so such
a network cannot update).

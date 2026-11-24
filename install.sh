#!/bin/bash
# Interface installer. In Terminal:
#
#   curl -fsSL https://raw.githubusercontent.com/KilianTheMagnificent/interface/main/install.sh | bash
#
# Downloads the latest release, checks it against the checksum its release
# names and that it is signed by the maintainer's Apple team, puts
# Interface.app into Applications and opens it. Interface is not notarized by
# Apple; a build fetched this way opens without the "Apple could not verify"
# refusal that a browser download gets, which is why the signature is checked
# here instead. The installed app then updates itself (docs/PUBLIC_UPDATES.md).
#
# For testing: INTERFACE_INSTALL_DIR (where the app goes), INTERFACE_NO_OPEN=1
# (do not open it), INTERFACE_RELEASE_FEED (another folder address holding
# interface-update.json and the archive). None of them loosens the signature
# check.
set -euo pipefail

RELEASES="KilianTheMagnificent/interface-releases"
BUNDLE_ID="com.andreromero.interface"
TEAM_ID="36B25Z964W"
APP_NAME="Interface.app"
# The rule the app's own updater holds a build to (electron/scripts/sign-mac.cjs).
REQUIREMENT="identifier \"$BUNDLE_ID\" and anchor apple generic and certificate leaf[subject.OU] = \"$TEAM_ID\""

say() { printf '%s\n' "$*"; }
stop() { printf 'Interface was not installed: %s\n' "$*" >&2; exit 1; }

# 1 when dotted version $1 is lower than $2, else 0.
version_lower() {
  [ "$1" = "$2" ] && { echo 0; return; }
  local lowest
  lowest=$(printf '%s\n%s\n' "$1" "$2" | sort -t. -k1,1n -k2,2n -k3,3n | head -1)
  [ "$lowest" = "$1" ] && echo 1 || echo 0
}

plist_value() { /usr/bin/plutil -extract "$2" raw -o - "$1" 2>/dev/null || true; }

main() {
  [ "$(uname -s)" = "Darwin" ] || stop "Interface runs on macOS only."
  [ "$(uname -m)" = "arm64" ] || stop "Interface needs an Apple Silicon Mac (M1 or newer). Intel Macs are not supported."
  local mac_version
  mac_version=$(/usr/bin/sw_vers -productVersion)

  local work
  work=$(mktemp -d "${TMPDIR:-/tmp}/interface-install.XXXXXX")
  # shellcheck disable=SC2064
  trap "rm -rf '$work'" EXIT

  say "Looking up the latest Interface release…"
  local feed="${INTERFACE_RELEASE_FEED:-https://github.com/$RELEASES/releases/latest/download}"
  local protocols="=https"
  [ -z "${INTERFACE_RELEASE_FEED:-}" ] || protocols="=https,http"
  curl -fsSL --proto "$protocols" --max-filesize 65536 -o "$work/envelope.json" "$feed/interface-update.json" \
    || stop "no release could be read from GitHub. Check the internet connection and try again."

  # The release description is a signed envelope; the app checks that
  # signature itself on every update. Here the code signature below is the
  # check: only a build signed by the maintainer's Apple team is installed.
  plist_value "$work/envelope.json" payload | /usr/bin/base64 -D > "$work/release.json" 2>/dev/null \
    || stop "the release description could not be read."
  local version name sha512 product
  product=$(plist_value "$work/release.json" product)
  version=$(plist_value "$work/release.json" version)
  name=$(plist_value "$work/release.json" artifact.name)
  sha512=$(plist_value "$work/release.json" artifact.sha512)
  [ "$product" = "$BUNDLE_ID" ] || stop "the release description is not Interface's."
  printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || stop "the release names no version."
  [ "$name" = "Interface-$version-arm64-mac.zip" ] || stop "the release names an unexpected download."
  [ -n "$sha512" ] || stop "the release names no checksum."

  local folder="${INTERFACE_INSTALL_DIR:-/Applications}"
  if [ ! -w "$folder" ]; then
    folder="$HOME/Applications"
    mkdir -p "$folder"
  fi
  local destination="$folder/$APP_NAME"
  if [ -d "$destination" ]; then
    local installed
    installed=$(plist_value "$destination/Contents/Info.plist" CFBundleShortVersionString)
    if /usr/bin/pgrep -f "$destination/Contents/MacOS/" >/dev/null 2>&1; then
      say "Interface ${installed:-} is already installed and running. It keeps itself up to date: Settings > Updates > Check for updates."
      return 0
    fi
    if printf '%s' "$installed" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
        && [ "$(version_lower "$installed" "$version")" = 0 ]; then
      say "Interface $installed is already installed in $folder."
      [ -n "${INTERFACE_NO_OPEN:-}" ] || /usr/bin/open "$destination"
      return 0
    fi
  fi

  say "Downloading Interface ${version}…"
  local progress="-sS"
  [ ! -t 2 ] || progress="--progress-bar"
  curl -fL --proto "$protocols" "$progress" -o "$work/app.zip" "$feed/$name" \
    || stop "the download failed. Try again."
  local actual
  actual=$(/usr/bin/shasum -a 512 "$work/app.zip" | cut -d' ' -f1 | /usr/bin/xxd -r -p | /usr/bin/base64)
  [ "$actual" = "$sha512" ] || stop "the download does not match the checksum its release names."

  /usr/bin/ditto -x -k "$work/app.zip" "$work/unpacked" || stop "the download could not be unpacked."
  local app="$work/unpacked/$APP_NAME"
  [ -d "$app" ] || stop "the download does not contain $APP_NAME."

  say "Checking the signature…"
  /usr/bin/codesign --verify --deep --strict "$app" 2>/dev/null \
    || stop "the app's signature is broken."
  /usr/bin/codesign --verify -R="$REQUIREMENT" "$app" 2>/dev/null \
    || stop "the app is not signed by Interface's maintainer."

  local needs built
  needs=$(plist_value "$app/Contents/Info.plist" LSMinimumSystemVersion)
  built=$(plist_value "$app/Contents/Info.plist" CFBundleShortVersionString)
  [ "$built" = "$version" ] || stop "the download is not the version the release names."
  if [ -n "$needs" ] && [ "$(version_lower "$mac_version" "$needs")" = 1 ]; then
    stop "this release needs macOS $needs or newer; this Mac has $mac_version."
  fi

  say "Installing into ${folder}…"
  local staged="$folder/.Interface-installing.$$"
  rm -rf "$staged"
  /usr/bin/ditto "$app" "$staged" || { rm -rf "$staged"; stop "could not write to $folder."; }
  /usr/bin/xattr -dr com.apple.quarantine "$staged" 2>/dev/null || true
  /usr/bin/codesign --verify -R="$REQUIREMENT" "$staged" 2>/dev/null \
    || { rm -rf "$staged"; stop "the installed copy did not pass the signature check."; }
  rm -rf "$destination"
  mv "$staged" "$destination"

  say ""
  say "Interface $version is installed in $folder."
  say "It runs the agent command-line tools already on this Mac (Claude Code, Codex and others),"
  say "signed in as you. Install and sign in to at least one of them first."
  [ -n "${INTERFACE_NO_OPEN:-}" ] || /usr/bin/open "$destination"
}

main "$@"

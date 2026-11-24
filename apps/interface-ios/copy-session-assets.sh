#!/bin/sh
set -eu
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
cd "$SRCROOT/../../electron"
npx vite build --config vite.phone.config.ts
destination="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/RemoteSession"
mkdir -p "$destination"
for asset in remote.html remote.js icon.svg manifest.webmanifest; do
  cp "dist-phone/$asset" "$destination/$asset"
done
# Vite includes the IIFE's styles in JavaScript. Keep the fixed asset map total.
: > "$destination/remote.css"

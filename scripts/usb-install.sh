#!/bin/bash
# scripts/usb-install.sh -- put a built GameApp.app on a phone that is plugged in by cable, signed by Xcode with the Mac's own development account. A TEST AID, never the deliverable.
#
# The deliverable (dist/GameApp-rust-*.ipa) is ad hoc signed and re-signed by SideStore / FlareStore on the phone. A phone on the cable cannot take that: it wants a development signature and a
# provisioning profile. This script does what Xcode does for an app, without a project of ours: it wraps a COPY of the built bundle in an .xcarchive and lets `xcodebuild -exportArchive
# -allowProvisioningUpdates` sign it (the entitlements are the ones already in the executable's signature: increased-memory-limit). Xcode makes, finds and embeds the profile itself: this
# script never reads, copies or prints a provisioning profile or a certificate's private key.
#
#   scripts/usb-install.sh <GameApp.app> [--device <udid>] [--bundle-id <id>] [--team <TEAMID>] [--no-launch]
#
# The copy gets the bundle id dev.ibuilder.app.usb by default: its own data container beside the one SideStore made, and an App ID the Apple account already holds (a free account is
# limited to ten new App IDs a week). The .app given is not changed. Requirements: Xcode signed in with an Apple ID (the team's development certificate is in the login keychain), the phone
# trusted and in developer mode, `xcrun devicectl list devices` showing it "connected".
set -uo pipefail
APP="${1:-}"; shift || true
[ -d "$APP" ] || { echo "usage: $0 <GameApp.app> [--device <udid>] [--bundle-id <id>] [--team <TEAMID>] [--no-launch]" >&2; exit 2; }
DEVICE=""; BUNDLE_ID="dev.ibuilder.app.usb"; TEAM=""; LAUNCH=1
while [ $# -gt 0 ]; do
  case "$1" in
    --device) DEVICE="$2"; shift 2 ;;
    --bundle-id) BUNDLE_ID="$2"; shift 2 ;;
    --team) TEAM="$2"; shift 2 ;;
    --no-launch) LAUNCH=0; shift ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done
die() { echo "usb-install: $*" >&2; exit 1; }
if [ -z "$DEVICE" ]; then
  DEVICE="$(xcrun devicectl list devices 2>/dev/null | awk '/physical/ && /connected/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) {print $i; exit}}')"
  [ -n "$DEVICE" ] || die "no physical device is connected (xcrun devicectl list devices); plug the phone in, unlock it, trust this Mac"
fi
if [ -z "$TEAM" ]; then
  # the development certificate's organisational unit is the team id (public data of the certificate, not its key)
  TEAM="$(security find-certificate -c "Apple Development" -p 2>/dev/null | openssl x509 -noout -subject 2>/dev/null | sed -n 's/.*OU=\([A-Z0-9]\{10\}\).*/\1/p' | head -1)"
  [ -n "$TEAM" ] || die "no 'Apple Development' certificate in the keychain: sign in to Xcode (Settings > Accounts) first, or pass --team"
fi
W="$(mktemp -d "${TMPDIR:-/tmp}/usb-install.XXXXXX")" || exit 2
if [ "${USB_KEEP:-0}" = 1 ]; then echo "usb-install: working folder kept: $W" >&2; else trap 'rm -rf "${W:?}"' EXIT; fi
NAME="$(basename "$APP" .app)"
ARCH="$W/$NAME.xcarchive"
mkdir -p "$ARCH/Products/Applications"
ditto --norsrc --noextattr --noacl "$APP" "$ARCH/Products/Applications/$NAME.app" || die "could not copy the bundle"
COPY="$ARCH/Products/Applications/$NAME.app"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $BUNDLE_ID" "$COPY/Info.plist" || die "could not set the bundle id"
# The background task identifier the app asks for is "<runtime bundle id>.build.<n>" (GameAppBackground.cs) and Info.plist must permit it: this copy runs under another bundle id than the one the build
# knew, so its own wildcard is added to the permitted list (BGTaskSchedulerPermittedIdentifiers).
/usr/libexec/PlistBuddy -c "Add :BGTaskSchedulerPermittedIdentifiers: string $BUNDLE_ID.build.*" "$COPY/Info.plist" >/dev/null 2>&1 || true
# The executable's own (ad hoc) signature carries the entitlements and the OLD identifier; Xcode keeps both when it signs again, and an identifier that is not the bundle id is refused at install
# (0xe8008016). So the copy's executable is signed ad hoc once more, with the new identifier and the same entitlements; Xcode then signs everything with the team's certificate.
EXE="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$COPY/Info.plist" 2>/dev/null || echo "$NAME")"
codesign -d --entitlements :- "$COPY/$EXE" > "$W/entitlements.plist" 2>/dev/null
[ -s "$W/entitlements.plist" ] && grep -q '<plist' "$W/entitlements.plist" || echo '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict/></plist>' > "$W/entitlements.plist"
codesign --force --sign - --timestamp=none --identifier "$BUNDLE_ID" --entitlements "$W/entitlements.plist" "$COPY/$EXE" >/dev/null 2>&1 || die "could not re-sign the executable with the new identifier"
# the nested frameworks: the installer wants each one's signing identifier to be its own CFBundleIdentifier, and Xcode keeps the identifier the signature already has
for fw in "$COPY"/Frameworks/*.framework; do
  [ -d "$fw" ] || continue
  fwid="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$fw/Info.plist" 2>/dev/null)"
  [ -n "$fwid" ] || continue
  fwexe="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$fw/Info.plist" 2>/dev/null)"
  codesign --force --sign - --timestamp=none --identifier "$fwid" "$fw/${fwexe:-$(basename "$fw" .framework)}" >/dev/null 2>&1 || die "could not re-sign $fw"
done
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$COPY/Info.plist" 2>/dev/null || echo 1.0)"
BUILD="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$COPY/Info.plist" 2>/dev/null || echo 1)"
cat > "$ARCH/Info.plist" <<XML
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>ApplicationProperties</key><dict>
    <key>ApplicationPath</key><string>Applications/$NAME.app</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$BUILD</string>
    <key>SigningIdentity</key><string>Apple Development</string>
    <key>Team</key><string>$TEAM</string>
  </dict>
  <key>ArchiveVersion</key><integer>2</integer>
  <key>Name</key><string>$NAME</string>
  <key>SchemeName</key><string>$NAME</string>
</dict></plist>
XML
cat > "$W/export.plist" <<XML
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>method</key><string>debugging</string>
  <key>teamID</key><string>$TEAM</string>
  <key>signingStyle</key><string>automatic</string>
  <key>stripSwiftSymbols</key><false/>
  <key>compileBitcode</key><false/>
  <key>thinning</key><string>&lt;none&gt;</string>
</dict></plist>
XML
echo "usb-install: signing $NAME as $BUNDLE_ID (team $TEAM) with Xcode, for device $DEVICE" >&2
xcodebuild -exportArchive -archivePath "$ARCH" -exportPath "$W/out" -exportOptionsPlist "$W/export.plist" -allowProvisioningUpdates > "$W/export.log" 2>&1 \
  || { tail -25 "$W/export.log" >&2; die "xcodebuild -exportArchive failed"; }
IPA="$(find "$W/out" -name '*.ipa' | head -1)"
if [ -n "$IPA" ]; then
  mkdir -p "$W/x" && ditto -x -k "$IPA" "$W/x" || die "could not read the exported .ipa"
  SIGNED="$W/x/Payload/$NAME.app"
else
  SIGNED="$(find "$W/out" -maxdepth 2 -name '*.app' | head -1)"
fi
[ -d "$SIGNED" ] || { tail -25 "$W/export.log" >&2; die "Xcode exported no app"; }
codesign -d --entitlements :- "$SIGNED" 2>/dev/null | grep -o 'increased-memory-limit\|get-task-allow' | sort -u | tr '\n' ' ' | sed 's/^/usb-install: entitlements kept: /' >&2; echo >&2
xcrun devicectl device install app --device "$DEVICE" "$SIGNED" > "$W/install.log" 2>&1 || { grep -vE '^\s*$' "$W/install.log" | tail -25 >&2; die "devicectl install failed"; }
tail -4 "$W/install.log" >&2
if [ "$LAUNCH" = 1 ]; then
  xcrun devicectl device process launch --device "$DEVICE" --terminate-existing "$BUNDLE_ID" 2>&1 | tail -3 >&2 || echo "usb-install: the app is installed but did not launch (is the phone unlocked?)" >&2
fi
echo "usb-install: done ($BUNDLE_ID on $DEVICE)" >&2

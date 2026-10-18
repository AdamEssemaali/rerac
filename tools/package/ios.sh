#!/usr/bin/env bash
# Builds rerac-metal (crate rc-metal, the native Metal runtime) for iOS and wraps it in an app bundle.
#
#   tools/package/ios.sh [sim|device] [--no-build]
#
#   sim (default)  dist/ios/sim/ReRAC.app for the iOS Simulator (Apple silicon), signed ad hoc. Install and run:
#                    xcrun simctl install booted dist/ios/sim/ReRAC.app
#                    xcrun simctl launch booted "$IOS_BUNDLE_ID"
#   device         dist/ios/device/ReRAC.app and dist/ios/ReRAC.ipa for an iPhone / iPad, signed with
#                    IOS_SIGN_IDENTITY       a code-signing identity (`security find-identity -v -p codesigning`)
#                    IOS_PROVISIONING_PROFILE  a .mobileprovision for IOS_BUNDLE_ID (Xcode makes one: Settings >
#                                            Accounts, then a project with this bundle id; ~/Library/Developer/Xcode/
#                                            UserData/Provisioning Profiles/)
#                  Install: `xcrun devicectl device install app --device <id> dist/ios/device/ReRAC.app`.
#
#   IOS_BUNDLE_ID  the bundle identifier (default io.github.adamessemaali.rerac; a free Apple ID team needs its own).
#
# The game data is not in the bundle. The app reads it from `Documents/data` in its container: copy the launcher's
# data folder (games/rac1/data, or the development `extracted/`) there with the Finder (iPhone > Files > ReRAC) or the
# Files app; in the Simulator, `xcrun simctl get_app_container booted "$IOS_BUNDLE_ID" data` shows the container.
# RC_DATA_DIR (simulator: `SIMCTL_CHILD_RC_DATA_DIR=<dir> xcrun simctl launch ...`) points elsewhere.
#
# Input: a controller (GameController: Xbox, PlayStation, MFi), a hardware keyboard (the desktop keys), or the
# on-screen pad, which appears on the first touch (crate::engine::touch).
set -euo pipefail

kind=sim
build=1
for a in "$@"; do
  case "$a" in
    sim|device) kind="$a" ;;
    --no-build) build=0 ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "unknown argument: $a" >&2; exit 2 ;;
  esac
done

repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
version="$(awk '/^\[/ { in_pkg = ($0 == "[workspace.package]") } in_pkg && /^version = "/ { gsub(/^version = "|"$/, ""); print; exit }' Cargo.toml)"
bundle_id="${IOS_BUNDLE_ID:-io.github.adamessemaali.rerac}"
# MSL 3.2 (crate::shader, as wgpu-hal's) needs iOS 18.
min_os=18.0

case "$kind" in
  sim) target=aarch64-apple-ios-sim; platform=iPhoneSimulator; sdk=iphonesimulator ;;
  device) target=aarch64-apple-ios; platform=iPhoneOS; sdk=iphoneos ;;
esac
if [ "$kind" = device ]; then
  : "${IOS_SIGN_IDENTITY:?set IOS_SIGN_IDENTITY (see --help)}"
  : "${IOS_PROVISIONING_PROFILE:?set IOS_PROVISIONING_PROFILE (see --help)}"
fi

bin="${CARGO_TARGET_DIR:-$repo/target}/$target/release/rerac-metal"
if [ "$build" = 1 ]; then
  rustup target list --installed | grep -qx "$target" || rustup target add "$target"
  start=$(date +%s)
  IPHONEOS_DEPLOYMENT_TARGET=$min_os cargo build --release --locked -p rc-metal --target "$target"
  echo "build: $(( $(date +%s) - start )) s"
fi
[ -x "$bin" ] || { echo "missing $bin (run without --no-build)" >&2; exit 1; }

out="$repo/dist/ios/$kind"
app="$out/ReRAC.app"
rm -rf "$app"
mkdir -p "$app"
cp "$bin" "$app/rerac-metal"
sdk_version="$(xcrun --sdk "$sdk" --show-sdk-version)"

cat > "$app/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$bundle_id</string>
  <key>CFBundleName</key><string>ReRAC</string>
  <key>CFBundleDisplayName</key><string>ReRAC</string>
  <key>CFBundleExecutable</key><string>rerac-metal</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleSupportedPlatforms</key><array><string>$platform</string></array>
  <key>DTPlatformName</key><string>$sdk</string>
  <key>DTSDKName</key><string>$sdk$sdk_version</string>
  <key>MinimumOSVersion</key><string>$min_os</string>
  <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
  <key>UIRequiredDeviceCapabilities</key><array><string>arm64</string><string>metal</string></array>
  <key>UISupportedInterfaceOrientations</key>
  <array><string>UIInterfaceOrientationLandscapeRight</string><string>UIInterfaceOrientationLandscapeLeft</string></array>
  <key>UISupportedInterfaceOrientations~ipad</key>
  <array><string>UIInterfaceOrientationLandscapeRight</string><string>UIInterfaceOrientationLandscapeLeft</string></array>
  <key>UILaunchScreen</key><dict/>
  <key>UIRequiresFullScreen</key><true/>
  <key>UIStatusBarHidden</key><true/>
  <key>UIFileSharingEnabled</key><true/>
  <key>LSSupportsOpeningDocumentsInPlace</key><true/>
  <key>GCSupportsControllerUserInteraction</key><true/>
  <key>GCSupportedGameControllers</key>
  <array><dict><key>ProfileName</key><string>ExtendedGamepad</string></dict></array>
</dict>
</plist>
EOF
plutil -lint "$app/Info.plist" > /dev/null

case "$kind" in
  sim)
    codesign --force --sign - --timestamp=none "$app"
    echo "app $app"
    ;;
  device)
    cp "$IOS_PROVISIONING_PROFILE" "$app/embedded.mobileprovision"
    ent="$out/entitlements.plist"
    security cms -D -i "$IOS_PROVISIONING_PROFILE" > "$out/profile.plist"
    /usr/libexec/PlistBuddy -x -c 'Print :Entitlements' "$out/profile.plist" > "$ent"
    codesign --force --sign "$IOS_SIGN_IDENTITY" --entitlements "$ent" --timestamp=none "$app"
    rm -rf "$out/Payload" "$repo/dist/ios/ReRAC.ipa"
    mkdir -p "$out/Payload"
    cp -R "$app" "$out/Payload/"
    (cd "$out" && zip -qry "$repo/dist/ios/ReRAC.ipa" Payload)
    rm -rf "$out/Payload"
    echo "app $app"
    echo "ipa $repo/dist/ios/ReRAC.ipa"
    ;;
esac

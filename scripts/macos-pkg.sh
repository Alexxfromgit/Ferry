#!/usr/bin/env bash
# Wraps a built Ferry.app in a macOS Installer package: ./scripts/macos-pkg.sh <Ferry.app> <out-dir>
# Tauri has no .pkg target, so build.sh --bundle calls this after `tauri build`.
#
# pkgbuild makes a component package that installs the app into /Applications, then productbuild
# wraps it in a product archive (what Installer.app expects) limited to the app's architecture and
# minimum macOS version. The bundle is marked non-relocatable: otherwise Installer would "upgrade"
# any other copy of dev.ferry.desktop it finds, such as one in target/, instead of /Applications.
#
# Set APPLE_INSTALLER_SIGNING_IDENTITY to a "Developer ID Installer" identity to sign the package.
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "Usage: $0 <Ferry.app> <out-dir>" >&2
    exit 2
fi
app=${1%/}
out_dir=$2

plist="$app/Contents/Info.plist"
if [[ ! -f "$plist" ]]; then
    echo "No app bundle at $app" >&2
    exit 1
fi
# Apple silicon only runs code with a valid signature; Tauri signs the bundle ad hoc ("-") unless
# APPLE_SIGNING_IDENTITY names a Developer ID.
codesign --verify --deep --strict "$app"
read_plist() { /usr/libexec/PlistBuddy -c "Print :$1" "$plist"; }
identifier=$(read_plist CFBundleIdentifier)
version=$(read_plist CFBundleShortVersionString)
title=$(read_plist CFBundleDisplayName)
min_os=$(read_plist LSMinimumSystemVersion)
executable="$app/Contents/MacOS/$(read_plist CFBundleExecutable)"

# Installer's names for the CPU types, and Tauri's for the file name (as in Ferry_0.1.0_aarch64.dmg).
host_archs=$(lipo -archs "$executable" | sed -e 's/ /,/g')
case "$host_archs" in
    arm64) file_arch=aarch64 ;;
    x86_64) file_arch=x64 ;;
    *) file_arch=universal ;;
esac

work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

mkdir "$work/root"
# Copy without extended attributes or quarantine flags so the package doesn't install them.
ditto --norsrc --noextattr --noqtn --noacl "$app" "$work/root/$(basename -- "$app")"
pkgbuild --analyze --root "$work/root" "$work/components.plist" >/dev/null
plutil -replace 0.BundleIsRelocatable -bool NO "$work/components.plist"

pkgbuild --quiet \
    --root "$work/root" \
    --component-plist "$work/components.plist" \
    --identifier "$identifier" \
    --version "$version" \
    --install-location /Applications \
    "$work/component.pkg"

cat >"$work/distribution.xml" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
    <title>$title</title>
    <options customize="never" require-scripts="false" hostArchitectures="$host_archs"/>
    <domains enable_anywhere="false" enable_currentUserHome="false" enable_localSystem="true"/>
    <volume-check>
        <allowed-os-versions>
            <os-version min="$min_os"/>
        </allowed-os-versions>
    </volume-check>
    <choices-outline>
        <line choice="default">
            <line choice="$identifier"/>
        </line>
    </choices-outline>
    <choice id="default"/>
    <choice id="$identifier" visible="false">
        <pkg-ref id="$identifier"/>
    </choice>
    <pkg-ref id="$identifier" version="$version" onConclusion="none">component.pkg</pkg-ref>
</installer-gui-script>
EOF

sign_args=()
if [[ -n "${APPLE_INSTALLER_SIGNING_IDENTITY:-}" ]]; then
    sign_args=(--sign "$APPLE_INSTALLER_SIGNING_IDENTITY")
fi

# Clear packages from earlier versions so release staging finds exactly one.
rm -rf -- "$out_dir"
mkdir -p -- "$out_dir"
pkg="$out_dir/${title}_${version}_${file_arch}.pkg"
productbuild --quiet \
    --distribution "$work/distribution.xml" \
    --package-path "$work" \
    --identifier "$identifier" \
    --version "$version" \
    ${sign_args[@]+"${sign_args[@]}"} \
    "$pkg"
echo "Built $pkg"

#!/usr/bin/env bash
# Builds the macOS bundle for Libertum: Libertum.app and a compressed Libertum-<version>-macos-<arch>.dmg.
#
# Rendered with the tools every macOS runner has (sips, iconutil, hdiutil, codesign); no installer
# toolkit is needed. The bundle declares Libertum for .lbtd, .odt, .docx, .rtf and .md, carries the
# README and the third-party notices, and is signed ad hoc (not with a Developer ID: no certificate
# authority will issue one for this project). Gatekeeper still asks the first time.
#
# Usage (from the repository root, after `cargo build --release -p libertum-app`):
#
#   packaging/macos/build-app.sh
#   packaging/macos/build-app.sh --version 0.1.2 --binary target/aarch64-apple-darwin/release/libertum --out dist
#
# Good to run only on macOS.
set -euo pipefail

root_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)

usage() {
    sed -n '2,14p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

die() {
    printf 'build-app: %s\n' "$*" >&2
    exit 1
}

version=""
binary=""
out_dir=""
arch=""

while [ $# -gt 0 ]; do
    case "$1" in
        --version) version="${2:?--version needs a value}"; shift 2 ;;
        --binary) binary="${2:?--binary needs a value}"; shift 2 ;;
        --out) out_dir="${2:?--out needs a value}"; shift 2 ;;
        --arch) arch="${2:?--arch needs a value}"; shift 2 ;;
        -h|--help) usage 0 ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

[ "$(uname -s)" = "Darwin" ] || die "this script builds a macOS bundle and must run on macOS"

# The workspace version is the bundle version unless one is given.
if [ -z "$version" ]; then
    # Strip CR so a Windows checkout with CRLF line endings still matches.
    version=$(tr -d '\r' < "$root_dir/Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n 1)
fi
[ -n "$version" ] || die "could not read the version from Cargo.toml; pass --version"

if [ -z "$binary" ]; then
    binary="$root_dir/target/release/libertum"
fi
[ -f "$binary" ] || die "no binary at $binary; build it first (cargo build --release -p libertum-app)"
[ -z "$out_dir" ] && out_dir="$root_dir/dist"

if [ -z "$arch" ]; then
    case "$(uname -m)" in
        arm64|aarch64) arch="arm64" ;;
        x86_64) arch="x64" ;;
        *) arch=$(uname -m) ;;
    esac
fi

for tool in sips iconutil hdiutil; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is missing (it comes with macOS)"
done

logo="$root_dir/crates/libertum-app/assets/logo.png"
[ -f "$logo" ] || die "no logo at $logo"

mkdir -p "$out_dir"
work=$(mktemp -d "${TMPDIR:-/tmp}/libertum-app.XXXXXX")
trap 'rm -rf "$work"' EXIT

app="$out_dir/Libertum.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

# The program.
install -m 755 "$binary" "$app/Contents/MacOS/libertum"

# The icon: every size Finder and the Dock ask for, from the one 512 px logo.
iconset="$work/Libertum.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$logo" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$logo" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/libertum.icns"

install -m 644 "$root_dir/README.md" "$app/Contents/Resources/README.md"
install -m 644 "$root_dir/docs/third-party-licences.md" \
    "$app/Contents/Resources/third-party-licences.md"

# The bundle id matches the window's application id (`libertum`), so macOS treats the bundle and
# its windows as one app: a Dock icon, a menu bar and "Open With" all use this name.
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Libertum</string>
    <key>CFBundleDisplayName</key><string>Libertum</string>
    <key>CFBundleIdentifier</key><string>app.libertum.libertum</string>
    <key>CFBundleExecutable</key><string>libertum</string>
    <key>CFBundleIconFile</key><string>libertum.icns</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleShortVersionString</key><string>${version}</string>
    <key>CFBundleVersion</key><string>${version}</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key><string>Libertum document</string>
            <key>CFBundleTypeRole</key><string>Editor</string>
            <key>LSHandlerRank</key><string>Owner</string>
            <key>LSItemContentTypes</key><array><string>app.libertum.document</string></array>
            <key>CFBundleTypeExtensions</key><array><string>lbtd</string></array>
        </dict>
        <dict>
            <key>CFBundleTypeName</key><string>OpenDocument text</string>
            <key>CFBundleTypeRole</key><string>Editor</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>CFBundleTypeExtensions</key><array><string>odt</string></array>
        </dict>
        <dict>
            <key>CFBundleTypeName</key><string>Word document</string>
            <key>CFBundleTypeRole</key><string>Editor</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>CFBundleTypeExtensions</key><array><string>docx</string></array>
        </dict>
        <dict>
            <key>CFBundleTypeName</key><string>Rich Text Format document</string>
            <key>CFBundleTypeRole</key><string>Editor</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>CFBundleTypeExtensions</key><array><string>rtf</string></array>
        </dict>
        <dict>
            <key>CFBundleTypeName</key><string>Markdown document</string>
            <key>CFBundleTypeRole</key><string>Editor</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>CFBundleTypeExtensions</key><array><string>md</string><string>markdown</string></array>
        </dict>
    </array>
    <key>UTExportedTypeDeclarations</key>
    <array>
        <dict>
            <key>UTTypeIdentifier</key><string>app.libertum.document</string>
            <key>UTTypeDescription</key><string>Libertum document</string>
            <key>UTTypeConformsTo</key><array><string>public.data</string></array>
            <key>UTTypeTagSpecification</key>
            <dict>
                <key>public.filename-extension</key><array><string>lbtd</string></array>
            </dict>
        </dict>
    </array>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null

# Ad hoc signing: Apple Silicon requires a signature, and this is the one an unsigned project can
# give. It is not notarised, so Gatekeeper still asks the first time (right-click > Open).
if command -v codesign >/dev/null 2>&1; then
    codesign --force --sign - "$app/Contents/MacOS/libertum" >/dev/null 2>&1 || true
    codesign --force --sign - "$app" >/dev/null 2>&1 || true
fi

dmg="$out_dir/Libertum-${version}-macos-${arch}.dmg"
rm -f "$dmg"
hdiutil create -quiet -volname "Libertum" -srcfolder "$app" -ov -format UDZO "$dmg"

printf 'build-app: %s\nbuild-app: %s\n' "$app" "$dmg"

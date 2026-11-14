#!/usr/bin/env bash
# Builds a self-contained Linux AppImage for Libertum: the release binary, the libraries it
# loads at run time, the desktop entry and the icon, in one executable file.
#
# packaging/linux/build-packages.sh makes the .deb and the .rpm, which rely on the
# distribution's libraries; this makes the portable artefact that carries its own.
#
# The packaging tools are linuxdeploy and appimagetool (both MIT, from the AppImage project).
# They are downloaded on first use unless their paths are given. On a machine without FUSE
# they run through `--appimage-extract-and-run` (APPIMAGE_EXTRACT_AND_RUN=1).
#
# Usage (from the repository root, after `cargo build --release -p libertum-app`):
#
#   packaging/linux/build-appimage.sh
#   packaging/linux/build-appimage.sh --version 0.1.2 --binary target/x86_64-unknown-linux-gnu/release/libertum --out dist
#   packaging/linux/build-appimage.sh --linuxdeploy /tmp/linuxdeploy --appimagetool /tmp/appimagetool
#
# Tools and network: set APPIMAGE_TOOLS_DIR to reuse downloaded tools between runs.
set -euo pipefail

root_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
tools_dir="${APPIMAGE_TOOLS_DIR:-}"

usage() {
    sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

die() {
    printf 'build-appimage: %s\n' "$*" >&2
    exit 1
}

version=""
binary=""
out_dir=""
appimage_arch=""
linuxdeploy=""
appimagetool=""

while [ $# -gt 0 ]; do
    case "$1" in
        --version) version="${2:?--version needs a value}"; shift 2 ;;
        --binary) binary="${2:?--binary needs a value}"; shift 2 ;;
        --out) out_dir="${2:?--out needs a value}"; shift 2 ;;
        --arch) appimage_arch="${2:?--arch needs a value}"; shift 2 ;;
        --linuxdeploy) linuxdeploy="${2:?--linuxdeploy needs a value}"; shift 2 ;;
        --appimagetool) appimagetool="${2:?--appimagetool needs a value}"; shift 2 ;;
        -h|--help) usage 0 ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

[ "$(uname -s)" = "Linux" ] || die "this script builds a Linux AppImage and must run on Linux"

if [ -z "$version" ]; then
    # Strip CR so a Windows checkout with CRLF line endings still matches.
    version=$(tr -d '\r' < "$root_dir/Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n 1)
fi
[ -n "$version" ] || die "could not read the version from Cargo.toml; pass --version"

if [ -z "$binary" ]; then
    binary="target/release/libertum"
fi
case "$binary" in
    /*) ;;
    *) binary="$root_dir/$binary" ;;
esac
[ -f "$binary" ] || die "no binary at $binary (run: cargo build --release -p libertum-app)"
[ -x "$binary" ] || die "$binary is not executable"
[ -z "$out_dir" ] && out_dir="$root_dir/dist"

case "$(uname -m)" in
    x86_64|amd64) host_arch="x86_64" ;;
    aarch64|arm64) host_arch="aarch64" ;;
    *) host_arch=$(uname -m) ;;
esac
[ -z "$appimage_arch" ] && appimage_arch="$host_arch"

mkdir -p "$out_dir"
cleanup_dirs=()
if [ -z "$tools_dir" ]; then
    tools_dir=$(mktemp -d "${TMPDIR:-/tmp}/libertum-appimage.XXXXXX")
    cleanup_dirs+=("$tools_dir")
else
    mkdir -p "$tools_dir"
fi
cleanup() {
    if [ ${#cleanup_dirs[@]} -gt 0 ]; then
        rm -rf "${cleanup_dirs[@]}"
    fi
}
trap cleanup EXIT

# The tools are AppImage files themselves, so they need either FUSE or the extract-and-run
# runtime. This also works in a container and on the CI runner.
export APPIMAGE_EXTRACT_AND_RUN=1

fetch_tool() {
    local name="$1" url="$2" dest="$3"
    if [ -x "$dest" ]; then
        return 0
    fi
    command -v curl >/dev/null 2>&1 || die "curl is needed to download $name"
    printf 'build-appimage: downloading %s\n' "$name" >&2
    curl -fsSL --retry 3 --retry-delay 5 "$url" -o "$dest" ||
        die "could not download $name from $url"
    chmod +x "$dest"
}

[ -n "$linuxdeploy" ] || linuxdeploy="$tools_dir/linuxdeploy"
[ -n "$appimagetool" ] || appimagetool="$tools_dir/appimagetool"
fetch_tool linuxdeploy \
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-${host_arch}.AppImage" \
    "$linuxdeploy"
fetch_tool appimagetool \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${host_arch}.AppImage" \
    "$appimagetool"

# Libraries the binary dlopen()s rather than links (the .deb declares the same): the AppImage
# carries the ones that are safe to carry. A window-system library is left to the host, as
# every desktop has one and a bundled copy would fight the platform's own; the graphics
# loaders, font stack and input libraries travel with the app.
libraries=(
    libfontconfig.so.1
    libfreetype.so.6
    libxkbcommon.so.0
    libwayland-client.so.0
    libwayland-cursor.so.0
    libwayland-egl.so.1
    libvulkan.so.1
    libEGL.so.1
    libGLESv2.so.2
)

library_args=()
if command -v ldconfig >/dev/null 2>&1; then
    for name in "${libraries[@]}"; do
        path=$(ldconfig -p 2>/dev/null | sed -n "s/^[[:space:]]*${name} ([^)]*) => \(.*\)$/\1/p" | head -n 1)
        if [ -n "$path" ] && [ -f "$path" ]; then
            library_args+=(--library "$path")
        else
            printf 'build-appimage: %s is not installed here; the AppImage will use the host'"'"'s\n' "$name" >&2
        fi
    done
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/libertum-appdir.XXXXXX")
cleanup_dirs+=("$work")
appdir="$work/AppDir"

printf 'build-appimage: staging %s\n' "$appdir" >&2
"$linuxdeploy" \
    --appdir "$appdir" \
    --executable "$binary" \
    --desktop-file "$root_dir/packaging/linux/libertum.desktop" \
    --icon-file "$root_dir/crates/libertum-app/assets/logo.png" \
    --icon-filename libertum \
    "${library_args[@]}" || die "linuxdeploy failed"

# The notices belong with the program, as in the .deb and the .rpm.
install -Dm644 "$root_dir/README.md" "$appdir/usr/share/doc/libertum/README.md"
install -Dm644 "$root_dir/docs/third-party-licences.md" \
    "$appdir/usr/share/doc/libertum/third-party-licences.md"

appimage="$out_dir/Libertum-${version}-${appimage_arch}.AppImage"
rm -f "$appimage"
printf 'build-appimage: packing %s\n' "$appimage" >&2
ARCH="$appimage_arch" "$appimagetool" "$appdir" "$appimage" || die "appimagetool failed"
chmod +x "$appimage"

printf 'build-appimage: %s\n' "$appimage"

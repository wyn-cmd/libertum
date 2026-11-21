#!/usr/bin/env bash
# Builds the Linux packages for Libertum: a .deb (dpkg-deb) and an .rpm (rpmbuild).
#
# The packages carry the release binary, a desktop entry, the application icon, the
# shared-mime-info entry for .lbtd files, the README and the third-party licence
# notices. No AppImage yet (PLAN.md §9); this script is the whole of the Linux packaging.
#
# Usage (from the repository root, after `cargo build --release -p libertum-app`):
#
#   packaging/linux/build-packages.sh
#   packaging/linux/build-packages.sh --version 0.1.2 --binary target/x86_64-unknown-linux-gnu/release/libertum
#   packaging/linux/build-packages.sh --format deb          # one of deb, rpm, all
#
# Requirements: dpkg-deb for the .deb, rpmbuild (the `rpm` package on Debian/Ubuntu)
# for the .rpm, and readelf (binutils) to work out the oldest glibc the binary needs.
set -euo pipefail

root_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)

usage() {
    sed -n '2,15p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

die() {
    printf 'build-packages: %s\n' "$*" >&2
    exit 1
}

version=""
binary=""
out_dir=""
format="all"
host_arch="$(uname -m)"
arch=""

while [ $# -gt 0 ]; do
    case "$1" in
        --version) version="${2:?--version needs a value}"; shift 2 ;;
        --binary) binary="${2:?--binary needs a value}"; shift 2 ;;
        --out) out_dir="${2:?--out needs a value}"; shift 2 ;;
        --format) format="${2:?--format needs a value}"; shift 2 ;;
        --arch) arch="${2:?--arch needs a value}"; shift 2 ;;
        -h|--help) usage 0 ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

case "$format" in
    deb|rpm|all) ;;
    *) die "--format must be deb, rpm or all (got '$format')" ;;
esac

# The workspace version is the package version unless one is given.
if [ -z "$version" ]; then
    # Strip CR so a Windows checkout with CRLF line endings still matches.
    version=$(tr -d '\r' < "$root_dir/Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n 1)
fi
[ -n "$version" ] || die "could not read the version from Cargo.toml; pass --version"
case "$version" in
    [0-9]*) ;;
    *) die "'$version' does not look like a package version" ;;
esac

if [ -z "$binary" ]; then
    binary="target/release/libertum"
fi
case "$binary" in
    /*) ;;
    *) binary="$root_dir/$binary" ;;
esac
[ -f "$binary" ] || die "no binary at $binary (run: cargo build --release -p libertum-app)"
[ -x "$binary" ] || die "$binary is not executable"

if [ -z "$out_dir" ]; then
    out_dir="dist"
fi
case "$out_dir" in
    /*) ;;
    *) out_dir="$root_dir/$out_dir" ;;
esac
mkdir -p "$out_dir"

[ -n "$arch" ] || arch="$host_arch"
case "$arch" in
    x86_64|amd64) host_arch=x86_64; deb_arch=amd64; rpm_arch=x86_64 ;;
    aarch64|arm64) host_arch=aarch64; deb_arch=arm64; rpm_arch=aarch64 ;;
    i386|i686) host_arch=i686; deb_arch=i386; rpm_arch=i686 ;;
    *) deb_arch="$arch"; rpm_arch="$arch" ;;
esac

# The oldest glibc the binary calls into, from its versioned symbols. A .deb that says
# libc6 (>= 2.39) refuses to install on an older distribution instead of failing later.
glibc_min=""
if command -v readelf >/dev/null 2>&1; then
    glibc_min=$(readelf --version-info "$binary" 2>/dev/null \
        | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -Vu | tail -n 1 || true)
fi
if [ -z "$glibc_min" ]; then
    # A statically linked or unusual binary: the build host's glibc is an upper bound.
    glibc_min=$(ldd --version 2>/dev/null | head -n 1 | grep -o '[0-9][0-9.]*$' || true)
fi
glibc_min=${glibc_min:-2.17}

# Runtime libraries. The window system, keyboard, font discovery, Vulkan and OpenGL
# libraries are either dlopen()ed or linked indirectly, so listing them explicitly is the
# only way the package manager knows a desktop needs them: fontconfig for the installed
# fonts, X11 and Wayland for the window, libxkbcommon for the keyboard, Vulkan (wgpu's
# first choice) and EGL/GLES (its fallback) for the window compositor, OpenSSL for HTTPS
# MCP servers (native-tls; Windows and macOS use their own TLS).
deb_depends="libc6 (>= $glibc_min), libgcc-s1, libfontconfig1, libx11-6, libxcursor1, libxrandr2, libxi6, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-cursor0, libvulkan1, libegl1, libgles2, libssl3t64 | libssl3"

rpm_requires="libc.so.6()(64bit)
libm.so.6()(64bit)
libgcc_s.so.1()(64bit)
libfontconfig.so.1()(64bit)
libX11.so.6()(64bit)
libXcursor.so.1()(64bit)
libXrandr.so.2()(64bit)
libXi.so.6()(64bit)
libxkbcommon.so.0()(64bit)
libxkbcommon-x11.so.0()(64bit)
libwayland-client.so.0()(64bit)
libwayland-cursor.so.0()(64bit)
libvulkan.so.1()(64bit)
libEGL.so.1()(64bit)
libssl.so.3()(64bit)
libcrypto.so.3()(64bit)
libGLESv2.so.2()(64bit)"

stage=$(mktemp -d "${TMPDIR:-/tmp}/libertum-packages.XXXXXX")
trap 'rm -rf "$stage"' EXIT
stage_root="$stage/root"

# --- The files both packages install -------------------------------------------------------

install -Dm755 "$binary" "$stage_root/usr/bin/libertum"
install -Dm644 "$root_dir/packaging/linux/libertum.desktop" \
    "$stage_root/usr/share/applications/libertum.desktop"
install -Dm644 "$root_dir/packaging/linux/libertum.mime.xml" \
    "$stage_root/usr/share/mime/packages/libertum.xml"
install -Dm644 "$root_dir/crates/libertum-app/assets/logo.png" \
    "$stage_root/usr/share/icons/hicolor/512x512/apps/libertum.png"
install -Dm644 "$root_dir/crates/libertum-app/assets/logo.png" \
    "$stage_root/usr/share/pixmaps/libertum.png"
install -Dm644 "$root_dir/README.md" "$stage_root/usr/share/doc/libertum/README.md"
install -Dm644 "$root_dir/docs/third-party-licences.md" \
    "$stage_root/usr/share/doc/libertum/third-party-licences.md"
install -Dm644 "$root_dir/packaging/linux/copyright" \
    "$stage_root/usr/share/doc/libertum/copyright"

# The caches are rebuilt after install and removal; a missing tool is not an error.
refresh_caches='if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || :
fi
if command -v update-mime-database >/dev/null 2>&1; then
    update-mime-database /usr/share/mime || :
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || :
fi'

# --- .deb ----------------------------------------------------------------------------------

build_deb() {
    command -v dpkg-deb >/dev/null 2>&1 || die "dpkg-deb is missing; install dpkg-dev (or build only the .rpm)"

    local debroot="$stage/debroot"
    cp -a "$stage_root" "$debroot"
    mkdir -p "$debroot/DEBIAN"

    cat > "$debroot/DEBIAN/control" <<EOF
Package: libertum
Version: $version
Architecture: $deb_arch
Maintainer: Libertum <noreply@libertum.invalid>
Installed-Size: $(du -sk "$debroot" | cut -f1)
Depends: $deb_depends
Recommends: shared-mime-info, desktop-file-utils, curl, pkexec
Section: editors
Priority: optional
Homepage: https://github.com/wyn-cmd/libertum
Description: word processor for long documents
 Libertum is a desktop word processor for long documents such as books and
 novels. It edits ODT and DOCX natively, reads and writes RTF and Markdown,
 and keeps its own lossless .lbtd format.
EOF

    cat > "$debroot/DEBIAN/postinst" <<EOF
#!/bin/sh
set -e
$refresh_caches
exit 0
EOF
    cat > "$debroot/DEBIAN/postrm" <<EOF
#!/bin/sh
set -e
$refresh_caches
exit 0
EOF
    chmod 0755 "$debroot/DEBIAN/postinst" "$debroot/DEBIAN/postrm"

    local deb="$out_dir/libertum_${version}_${deb_arch}.deb"
    rm -f "$deb"
    dpkg-deb --root-owner-group -Zxz --build "$debroot" "$deb" >/dev/null
    printf 'built %s\n' "$deb"
}

# --- .rpm ----------------------------------------------------------------------------------

build_rpm() {
    command -v rpmbuild >/dev/null 2>&1 || die "rpmbuild is missing; install the rpm package (or build only the .deb)"

    local top="$stage/rpmbuild"
    mkdir -p "$top/BUILD" "$top/BUILDROOT" "$top/RPMS" "$top/SOURCES" "$top/SPECS" "$top/SRPMS"

    local requires=""
    local line
    while IFS= read -r line; do
        requires="${requires}Requires: ${line}
"
    done <<< "$rpm_requires"

    cat > "$top/SPECS/libertum.spec" <<EOF
Name: libertum
Version: $version
Release: 1
Summary: Word processor for long documents
License: Proprietary
Group: Applications/Editors
URL: https://github.com/wyn-cmd/libertum
AutoReqProv: no
# The update check uses the system curl and the package updater its pkexec; the app works
# without either.
Recommends: curl, pkexec
$requires
%global debug_package %{nil}
%global _build_id_links none

%description
Libertum is a desktop word processor for long documents such as books and
novels. It edits ODT and DOCX natively, reads and writes RTF and Markdown,
and keeps its own lossless .lbtd format.

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}
cp -a %{_stagedir}/. %{buildroot}/

%post
$refresh_caches

%postun
$refresh_caches

%files
%defattr(-, root, root, -)
%attr(0755, root, root) /usr/bin/libertum
/usr/share/applications/libertum.desktop
/usr/share/mime/packages/libertum.xml
/usr/share/icons/hicolor/512x512/apps/libertum.png
/usr/share/pixmaps/libertum.png
/usr/share/doc/libertum/README.md
/usr/share/doc/libertum/third-party-licences.md
/usr/share/doc/libertum/copyright
EOF

    if ! rpmbuild --define "_topdir $top" --define "_stagedir $stage_root" \
        --define "_binary_payload w6.xzdio" --target "$rpm_arch" \
        -bb "$top/SPECS/libertum.spec" >"$stage/rpmbuild.log" 2>&1; then
        cat "$stage/rpmbuild.log" >&2
        die "rpmbuild failed"
    fi
    local rpm
    rpm="$top/RPMS/$rpm_arch/libertum-${version}-1.${rpm_arch}.rpm"
    [ -f "$rpm" ] || die "rpmbuild did not produce $rpm"
    rm -f "$out_dir/libertum-${version}-1.${rpm_arch}.rpm"
    cp "$rpm" "$out_dir/"
    printf 'built %s\n' "$out_dir/libertum-${version}-1.${rpm_arch}.rpm"
}

case "$format" in
    deb) build_deb ;;
    rpm) build_rpm ;;
    all) build_deb; build_rpm ;;
esac

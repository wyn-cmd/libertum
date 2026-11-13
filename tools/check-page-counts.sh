#!/usr/bin/env bash
# Compares Libertum's own page count with LibreOffice's for each document given (PLAN Phase 5:
# within ±2%). CI's `libreoffice-page-counts` job runs this after building the `page_count`
# example, so a layout change that moves pages away from the reference engine is caught.
#
# Requires: `soffice` (the libreoffice-writer-nogui package), `pdfinfo` (poppler-utils), and a
# built example: `cargo build --release -p libertum-app --example page_count`.
#
# Usage:
#
#   tools/check-page-counts.sh tests/fixtures/odt/rich.odt tests/fixtures/docx/rich.docx
#   tools/check-page-counts.sh --tolerance 2 --page-count target/release/examples/page_count <files...>
#
# Each count is one row; the script exits non-zero when any document is outside the tolerance.
set -euo pipefail

root_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
tolerance=2
page_count=""
files=()

usage() {
    sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

die() {
    printf 'check-page-counts: %s\n' "$*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --tolerance) tolerance="${2:?--tolerance needs a value}"; shift 2 ;;
        --page-count) page_count="${2:?--page-count needs a value}"; shift 2 ;;
        -h|--help) usage 0 ;;
        --) shift; files+=("$@"); break ;;
        -*) die "unknown option: $1 (try --help)" ;;
        *) files+=("$1"); shift ;;
    esac
done

case "$tolerance" in
    [0-9]*) ;;
    *) die "--tolerance must be a whole percentage" ;;
esac

[ ${#files[@]} -gt 0 ] || die "give at least one document (see --help)"

if [ -z "$page_count" ]; then
    page_count="$root_dir/target/release/examples/page_count"
fi
[ -x "$page_count" ] || die "no page_count example at $page_count (run: cargo build --release -p libertum-app --example page_count)"

for tool in soffice pdfinfo; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is missing (install libreoffice-writer-nogui and poppler-utils)"
done

work=$(mktemp -d "${TMPDIR:-/tmp}/libertum-page-counts.XXXXXX")
cleanup() {
    rm -rf "$work"
}
trap cleanup EXIT

# One LibreOffice profile for the whole run: a fresh one per call is slow, and a shared user
# profile would touch the machine's own settings.
profile="$work/lo-profile"
mkdir -p "$profile"

# LibreOffice writes <stem>.pdf; its page count is the number under "Pages:".
libreoffice_pages() {
    local file="$1" stem pdf
    stem=$(basename -- "$file")
    stem="${stem%.*}"
    pdf="$work/$stem.pdf"
    rm -f "$pdf"
    if ! soffice --headless --norestore \
        "-env:UserInstallation=file://$profile" \
        --convert-to pdf --outdir "$work" "$file" >"$work/soffice.log" 2>&1; then
        return 1
    fi
    [ -f "$pdf" ] || return 1
    pdfinfo "$pdf" | sed -n 's/^Pages:[[:space:]]*//p' | head -n 1
}

printf '%-44s %8s %8s %8s %8s\n' "document" "ours" "LO" "diff" "allowed"
failures=0
for file in "${files[@]}"; do
    case "$file" in
        /*) ;;
        *) file="$root_dir/$file" ;;
    esac
    [ -f "$file" ] || die "no such document: $file"
    label=$(basename -- "$file")

    if ! ours=$("$page_count" "$file"); then
        printf '%-44s %8s\n' "$label" "failed"
        printf 'check-page-counts: %s could not be paginated by the page_count example\n' "$label" >&2
        failures=$((failures + 1))
        continue
    fi
    case "$ours" in
        [0-9]*) ;;
        *) die "the page_count example printed something that is not a number: '$ours'" ;;
    esac

    if ! theirs=$(libreoffice_pages "$file"); then
        printf '%-44s %8s %8s\n' "$label" "$ours" "failed"
        printf 'check-page-counts: LibreOffice could not convert %s; see the log:\n' "$label" >&2
        sed 's/^/  /' "$work/soffice.log" >&2 || true
        failures=$((failures + 1))
        continue
    fi
    case "$theirs" in
        [0-9]*) ;;
        *) die "pdfinfo printed something that is not a number for $label: '$theirs'" ;;
    esac

    # Whole pages, rounded up, and never zero: a short fixture may differ by one page without
    # meaning anything, while a long document must stay inside the percentage.
    allowed=$(( (ours * tolerance + 99) / 100 ))
    [ "$allowed" -ge 1 ] || allowed=1
    if [ "$ours" -ge "$theirs" ]; then
        difference=$((ours - theirs))
    else
        difference=$((theirs - ours))
    fi

    printf '%-44s %8s %8s %8s %8s\n' "$label" "$ours" "$theirs" "$difference" "$allowed"
    if [ "$difference" -gt "$allowed" ]; then
        printf 'check-page-counts: %s is outside %s%% (ours %s, LibreOffice %s)\n' \
            "$label" "$tolerance" "$ours" "$theirs" >&2
        failures=$((failures + 1))
    fi
done

if [ "$failures" -gt 0 ]; then
    printf 'check-page-counts: %s document(s) outside the tolerance\n' "$failures" >&2
    exit 1
fi
printf 'check-page-counts: all %s document(s) within %s%%\n' "${#files[@]}" "$tolerance"

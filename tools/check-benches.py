#!/usr/bin/env python3
"""Compares two criterion baselines and fails when a benchmark regresses.

CI runs the same benches twice in one job (the base commit with `--save-baseline base`, the
head with `--save-baseline head`) in one target directory, so both runs share a runner and
only that runner's own noise remains. This script reads the two `estimates.json` files of
every benchmark and reports the change in the mean.

A regression must exceed both the percentage threshold and an absolute floor before it fails
the job: on a shared runner a few dozen microseconds of noise in a 300 µs benchmark must not
fail a pull request.

Usage:
    tools/check-benches.py [target/criterion] [--base base] [--head head]
                           [--threshold 10] [--floor-us 50]
"""

import argparse
import json
import pathlib
import sys


def mean_ns(path: pathlib.Path) -> float:
    with path.open(encoding="utf-8") as handle:
        data = json.load(handle)
    return float(data["mean"]["point_estimate"])


def human(ns: float) -> str:
    if ns >= 1_000_000_000:
        return f"{ns / 1_000_000_000:.3f} s"
    if ns >= 1_000_000:
        return f"{ns / 1_000_000:.2f} ms"
    if ns >= 1_000:
        return f"{ns / 1_000:.2f} us"
    return f"{ns:.0f} ns"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", nargs="?", default="target/criterion")
    parser.add_argument("--base", default="base")
    parser.add_argument("--head", default="head")
    parser.add_argument("--threshold", type=float, default=10.0,
                        help="percentage a benchmark may grow by (default 10)")
    parser.add_argument("--floor-us", type=float, default=50.0,
                        help="absolute growth, microseconds, that is always allowed (default 50)")
    args = parser.parse_args()

    root = pathlib.Path(args.root)
    if not root.is_dir():
        print(f"check-benches: no criterion results at {root}", file=sys.stderr)
        return 1

    rows = []
    failures = []
    for base_file in sorted(root.rglob(f"{args.base}/estimates.json")):
        # target/criterion/<group>/<benchmark>/<baseline>/estimates.json
        benchmark = base_file.parent.parent
        head_file = benchmark / args.head / "estimates.json"
        if not head_file.is_file():
            continue
        name = benchmark.relative_to(root).as_posix()
        base_ns = mean_ns(base_file)
        head_ns = mean_ns(head_file)
        if base_ns <= 0:
            continue
        change = (head_ns - base_ns) / base_ns * 100.0
        allowed_ns = max(base_ns * args.threshold / 100.0, args.floor_us * 1000.0)
        rows.append((name, base_ns, head_ns, change))
        if head_ns - base_ns > allowed_ns:
            failures.append((name, base_ns, head_ns, change, allowed_ns))

    if not rows:
        print(
            f"check-benches: no benchmark has both a '{args.base}' and a '{args.head}' "
            f"run under {root}",
            file=sys.stderr,
        )
        return 1

    width = max(len(name) for name, _, _, _ in rows)
    print(f"{'benchmark':<{width}}  {'base':>10}  {'head':>10}  {'change':>9}")
    for name, base_ns, head_ns, change in rows:
        print(f"{name:<{width}}  {human(base_ns):>10}  {human(head_ns):>10}  {change:+8.1f}%")

    if failures:
        print(
            f"\ncheck-benches: {len(failures)} benchmark(s) regressed by more than "
            f"{args.threshold}% and {args.floor_us} us:",
            file=sys.stderr,
        )
        for name, base_ns, head_ns, change, allowed_ns in failures:
            print(
                f"  {name}: {human(base_ns)} -> {human(head_ns)} ({change:+.1f}%, "
                f"allowed {human(allowed_ns)})",
                file=sys.stderr,
            )
        return 1

    print(f"\ncheck-benches: all {len(rows)} benchmark(s) within {args.threshold}%")
    return 0


if __name__ == "__main__":
    sys.exit(main())

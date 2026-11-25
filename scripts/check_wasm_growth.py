#!/usr/bin/env python3
"""Warn, without failing, when a contract wasm grows past its baseline.

Run after `stellar contract build`. `size-baselines.json` holds the last
recorded size of each contract wasm. A contract more than `tolerance_percent`
above its baseline gets a GitHub warning annotation so the growth is visible in
review; the build still passes, because a legitimate feature can need the room
and arguing about it is a job for the reviewer, not for CI.

The hard ceiling is `size-budgets.json`, checked by `check_wasm_sizes.py`. This
script only answers "did it move a lot since we last looked?".

Refresh a baseline after a deliberate size change:

    python3 scripts/check_wasm_growth.py --update

which rewrites the recorded sizes from the build in `target/`.
"""

import json
import pathlib
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
BASELINES = REPO / "size-baselines.json"
WASM_DIR = REPO / "target" / "wasm32v1-none" / "release"
DEFAULT_TOLERANCE = 5


def load_baselines():
    data = json.loads(BASELINES.read_text(encoding="utf-8"))
    return data.get("tolerance_percent", DEFAULT_TOLERANCE), data.get("baselines", {})


def built_wasm():
    if not WASM_DIR.is_dir():
        return {}
    return {p.stem: p for p in sorted(WASM_DIR.glob("*.wasm"))}


def update(wasm):
    tolerance, baselines = load_baselines()
    for name, path in wasm.items():
        baselines[name] = path.stat().st_size
    BASELINES.write_text(
        json.dumps(
            {"tolerance_percent": tolerance, "baselines": dict(sorted(baselines.items()))},
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"recorded {len(wasm)} baselines in {BASELINES.relative_to(REPO)}")


def main(argv):
    wasm = built_wasm()
    if not wasm:
        print(f"error: {WASM_DIR} not found. Run 'stellar contract build' first.", file=sys.stderr)
        return 1

    if "--update" in argv[1:]:
        update(wasm)
        return 0

    tolerance, baselines = load_baselines()
    print(f"{'CONTRACT':<24}{'BYTES':>10}{'BASELINE':>10}{'GROWTH':>10}")
    for name, path in sorted(wasm.items()):
        size = path.stat().st_size
        baseline = baselines.get(name)
        if baseline is None:
            print(f"{name:<24}{size:>10}{'no baseline':>10}{'-':>10}")
            continue
        growth = 100.0 * (size - baseline) / baseline
        print(f"{name:<24}{size:>10}{baseline:>10}{growth:>9.1f}%")
        if growth > tolerance:
            allow = int(baseline * (1 + tolerance / 100.0))
            print(
                f"::warning file=size-baselines.json,title=Wasm growth::{name}.wasm is "
                f"{size} bytes, {growth:.1f}% over its {baseline}-byte baseline "
                f"(over {tolerance}% above {allow}). If the growth is intended, run "
                f"`python3 scripts/check_wasm_growth.py --update` in the same commit "
                f"and mention it in the description."
            )

    missing = [name for name in baselines if name not in wasm]
    if missing:
        print(
            f"note: no wasm built for {', '.join(sorted(missing))}. "
            "Run 'stellar contract build' first.",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

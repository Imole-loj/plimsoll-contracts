#!/usr/bin/env python3
"""Show where a built contract wasm spends its bytes.

Run after `stellar contract build`:

    python3 scripts/wasm_size_report.py target/wasm32v1-none/release/coverage_ledger.wasm

For each file it prints the size of every wasm section, then the largest
exported function bodies. The per-function view is the same question `twiggy
top` answers, but it needs no extra tooling: release builds strip the name
section, so imported/twiggy-style symbol names are not available, while the
export section still names every entry point. Everything else is reported as
one unnamed total.

`twiggy` is still the better tool once you have it installed
(`cargo install twiggy`) because it follows call edges; this script only
reports body sizes. docs/wasm-size.md explains both.
"""

import pathlib
import sys

SECTION_NAMES = {
    0: "custom",
    1: "type",
    2: "import",
    3: "function",
    4: "table",
    5: "memory",
    6: "global",
    7: "export",
    8: "start",
    9: "element",
    10: "code",
    11: "data",
    12: "data count",
}


def uleb(data, i):
    """Read an unsigned LEB128 integer, returning (value, next index)."""
    result = shift = 0
    while True:
        byte = data[i]
        i += 1
        result |= (byte & 0x7F) << shift
        shift += 7
        if not byte & 0x80:
            return result, i


def read_name(data, i):
    length, i = uleb(data, i)
    return data[i : i + length].decode("utf-8", "replace"), i + length


def sections(data):
    """Yield (section id, payload) for a module, skipping the header."""
    i = 8
    while i < len(data):
        section_id = data[i]
        i += 1
        size, i = uleb(data, i)
        yield section_id, data[i : i + size]
        i += size


def imported_functions(payload):
    """How many of the module's function indices are imports."""
    count, i = uleb(payload, 0)
    functions = 0
    for _ in range(count):
        _, i = read_name(payload, i)
        _, i = read_name(payload, i)
        kind = payload[i]
        i += 1
        if kind == 0:  # func: type index
            functions += 1
            _, i = uleb(payload, i)
        elif kind == 1:  # table: element type + limits
            i += 1
            _, i = uleb(payload, i)
            flags = payload[i]
            i += 1
            _, i = uleb(payload, i)
            if flags & 1:
                _, i = uleb(payload, i)
        elif kind == 2:  # memory: limits
            flags = payload[i]
            i += 1
            _, i = uleb(payload, i)
            if flags & 1:
                _, i = uleb(payload, i)
        elif kind == 3:  # global: value type + mutability
            i += 2
    return functions


def exported_functions(payload):
    """Map function index to export name."""
    count, i = uleb(payload, 0)
    names = {}
    for _ in range(count):
        name, i = read_name(payload, i)
        kind = payload[i]
        i += 1
        index, i = uleb(payload, i)
        if kind == 0:
            names[index] = name
    return names


def body_sizes(payload):
    """Body size of each defined function, in function-index order."""
    count, i = uleb(payload, 0)
    sizes = []
    for _ in range(count):
        size, i = uleb(payload, i)
        sizes.append(size)
        i += size
    return sizes


def report(path):
    data = path.read_bytes()
    print(f"{path}")
    print(f"  total {len(data)} bytes\n")

    imported = 0
    names = {}
    bodies = []
    totals = {}
    for section_id, payload in sections(data):
        label = SECTION_NAMES.get(section_id, f"section {section_id}")
        totals[label] = totals.get(label, 0) + len(payload)
        if section_id == 2:
            imported = imported_functions(payload)
        elif section_id == 7:
            names = exported_functions(payload)
        elif section_id == 10:
            bodies = body_sizes(payload)

    print(f"  {'SECTION':<16}{'BYTES':>10}{'SHARE':>8}")
    for label, size in sorted(totals.items(), key=lambda item: -item[1]):
        share = 100.0 * size / len(data)
        print(f"  {label:<16}{size:>10}{share:>7.1f}%")

    if not bodies:
        print("\n  no code section\n")
        return

    measured = []
    unnamed = 0
    for i, size in enumerate(bodies):
        index = imported + i
        if index in names:
            measured.append((size, names[index]))
        else:
            unnamed += 1

    print(f"\n  {'LARGEST EXPORTED FUNCTIONS':<32}{'BYTES':>10}")
    for size, name in sorted(measured, reverse=True)[:10]:
        print(f"  {name:<32}{size:>10}")
    print(f"  {'(' + str(unnamed) + ' unnamed internal functions)':<32}{'':>10}")
    print()


def main(argv):
    paths = [pathlib.Path(arg) for arg in argv[1:]]
    if not paths:
        built = pathlib.Path(__file__).resolve().parent.parent / "target/wasm32v1-none/release"
        paths = sorted(built.glob("*.wasm"))
    if not paths:
        print("error: no wasm files given and none are built.", file=sys.stderr)
        return 1
    for path in paths:
        report(path)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

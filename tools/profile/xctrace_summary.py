#!/usr/bin/env python3
"""Summarise an Instruments Time Profiler trace: self and inclusive time by symbol.

    python3 -I tools/profile/xctrace_summary.py TRACE.trace [--top N] [--filter SUBSTR] [--short] [--inline]

TRACE may also be an XML file already exported with
    xcrun xctrace export --input TRACE.trace \
        --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]'

Self time is charged to a sample's leaf frame; inclusive time to every distinct symbol on the
sample's backtrace, once per sample. Percentages are of the (filtered) sample weight.

Frames in a binary that is on disk are mapped by address to the out-of-line function that
contains them (from `nm`). This matters because xctrace, when it finds a dSYM, names each frame
after its innermost *inlined* callee and drops the function it was inlined into, and because it
names identical-code-folded functions `<deduplicated_symbol>`. `--inline` keeps xctrace's
names instead (finer grained, but the containing function is lost). Folded aliases are shown
as `name (+N folded)`. Rust symbols are demangled with `c++filt` (Apple's understands legacy
and v0 mangling) and legacy `::h<hash>` suffixes are stripped. Standard library only.
"""

import argparse
import bisect
import collections
import os
import re
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET

XPATH = '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]'
HASH = re.compile(r"::h[0-9a-f]{16}\b")
UNNAMED = re.compile(r"^(0x[0-9a-f]+|<deduplicated_symbol>)?$")


def load_xml(path):
    if os.path.isdir(path) or path.endswith(".trace"):
        out = subprocess.run(
            ["xcrun", "xctrace", "export", "--input", path, "--xpath", XPATH],
            check=True, capture_output=True,
        ).stdout
        return ET.fromstring(out)
    return ET.parse(path).getroot()


class Refs:
    """xctrace writes each value in full with `id=` once and as `ref=` afterwards."""

    def __init__(self, root):
        self.by_id = {e.get("id"): e for e in root.iter() if e.get("id") is not None}

    def __call__(self, e):
        r = e.get("ref")
        return self.by_id[r] if r is not None else e


def samples(root):
    """Yields (weight, [frame elements, leaf first]) per row."""
    deref = Refs(root)
    for row in root.iter("row"):
        weight, frames = 1, []
        for child in row:
            child = deref(child)
            if child.tag == "weight":
                weight = int(child.text or 1)
            elif child.tag in ("tagged-backtrace", "backtrace"):
                bt = child if child.tag == "backtrace" else child.find("backtrace")
                if bt is not None:
                    frames = [deref(f) for f in deref(bt) if f.tag == "frame" or f.get("ref")]
                    frames = [f for f in frames if f.tag == "frame"]
        yield weight, frames


def frame_binary(frame, deref):
    b = frame.find("binary")
    return deref(b) if b is not None else None


def demangle(names):
    names = sorted(set(names))
    tool = shutil.which("c++filt")
    if not tool or not names:
        return {n: n for n in names}
    out = subprocess.run([tool], input="\n".join(names) + "\n", capture_output=True, text=True).stdout.splitlines()
    return dict(zip(names, out)) if len(out) == len(names) else {n: n for n in names}


class Symtab:
    """Text symbols of one Mach-O image, from `nm -n`, to map a sample address to the
    out-of-line function that contains it."""

    def __init__(self, path, arch):
        self.addrs, self.names, self.base, self.uuid = [], [], 0x100000000, None
        try:
            a = ["-arch", arch] if arch else []
            out = subprocess.run(["nm", "-n", *a, path], capture_output=True, text=True, check=True).stdout
            seg = subprocess.run(["otool", *a, "-l", path], capture_output=True, text=True).stdout
        except (OSError, subprocess.CalledProcessError):
            return
        m = re.search(r"segname __TEXT\n\s+vmaddr (0x[0-9a-f]+)", seg)
        if m:
            self.base = int(m.group(1), 16)
        u = re.search(r"\buuid ([0-9A-Fa-f-]+)", seg)
        self.uuid = u.group(1).upper() if u else None
        for line in out.splitlines():
            parts = line.split(None, 2)
            if len(parts) == 3 and parts[1] in "tT" and not parts[2].startswith("ltmp"):
                a, n = int(parts[0], 16), parts[2][1:] if parts[2].startswith("_") else parts[2]
                if self.addrs and self.addrs[-1] == a:
                    self.names[-1].append(n)
                else:
                    self.addrs.append(a)
                    self.names.append([n])

    def lookup(self, addr, load):
        """Aliases (identical-code-folded functions share an address) of the function at `addr`."""
        i = bisect.bisect_right(self.addrs, addr - load + self.base) - 1
        return self.names[i] if i >= 0 else None


def short(name):
    """Drops generic argument lists: `a::f::<T, U>` -> `a::f`, `<a::S<T> as b::Tr>::m` kept readable."""
    prev = None
    while prev != name:
        prev = name
        name = re.sub(r"::<[^<>]*>|(?<=[\w)])<[^<>]*>", "", name)
    return name


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("trace", help=".trace bundle or exported time-profile XML")
    ap.add_argument("--top", type=int, default=40, help="rows per table (default 40)")
    ap.add_argument("--filter", metavar="SUBSTR", help="keep samples with a frame whose symbol contains SUBSTR")
    ap.add_argument("--short", action="store_true", help="drop generic arguments from symbols")
    ap.add_argument("--inline", action="store_true", help="use xctrace's (innermost inlined) frame names")
    args = ap.parse_args()

    root = load_xml(args.trace)
    deref = Refs(root)
    rows = list(samples(root))

    tables = {}

    def function_of(f, leaf):
        """Aliases of the out-of-line function containing frame `f`, or None."""
        b = frame_binary(f, deref)
        if b is None or not b.get("path") or not b.get("load-addr") or not f.get("addr"):
            return None
        path = b.get("path")
        # System libraries: xctrace's names (from the shared cache) beat their stripped on-disk
        # symbol tables, so only unnamed frames are looked up there.
        system = path.startswith(("/usr/lib/", "/System/"))
        if (args.inline or system) and not UNNAMED.match(f.get("name") or ""):
            return None
        if path not in tables:
            t = Symtab(path, b.get("arch")) if os.path.isfile(path) else None
            if t is not None and b.get("UUID") and t.uuid and t.uuid != b.get("UUID").upper():
                print(f"warning: {path} was rebuilt since the trace (UUID differs); using xctrace's names for it", file=sys.stderr)
                t = None
            tables[path] = t
        if tables[path] is None:
            return None
        # A caller's address is a return address: step back into the call instruction.
        return tables[path].lookup(int(f.get("addr"), 16) - (0 if leaf else 1), int(b.get("load-addr"), 16))

    raw = []
    for weight, frames in rows:
        raw.append((weight, [function_of(f, i == 0) or [f.get("name") or f.get("addr") or "?"] for i, f in enumerate(frames)]))
    names = demangle({n for _, fs in raw for aliases in fs for n in aliases})

    def clean(n):
        n = HASH.sub("", names.get(n, n))
        return short(n) if args.short else n

    memo = {}

    def symbol(aliases):
        key = tuple(aliases)
        if key not in memo:
            shown = sorted({clean(n) for n in aliases})
            memo[key] = shown[0] + (f" (+{len(shown) - 1} folded)" if len(shown) > 1 else "")
        return memo[key]

    rows = [(w, [symbol(a) for a in fs]) for w, fs in raw]
    self_w, incl_w, total, kept = collections.Counter(), collections.Counter(), 0, 0
    for weight, syms in rows:
        syms = syms or ["<no backtrace>"]
        if args.filter and not any(args.filter in s for s in syms):
            continue
        kept += 1
        total += weight
        self_w[syms[0]] += weight
        for s in set(syms):
            incl_w[s] += weight

    if not total:
        sys.exit("no samples" + (f" match --filter {args.filter!r}" if args.filter else ""))
    print(f"{kept} samples, {total / 1e6:.1f} ms" + (f" (filter {args.filter!r}, of {len(rows)})" if args.filter else ""))
    for title, table in (("self", self_w), ("inclusive", incl_w)):
        print(f"\n== {title} time, top {args.top} ==")
        print(f"{'ms':>9} {'%':>6}  symbol")
        for s, w in table.most_common(args.top):
            print(f"{w / 1e6:9.1f} {100 * w / total:6.2f}  {s}")


if __name__ == "__main__":
    main()

# Profiling hooke

`bench/src/bin/profile.rs` loops one hooke operation for a wall-clock budget so a sampling
profiler sees nothing else; `xctrace_summary.py` turns an Instruments Time Profiler trace into
self-time and inclusive-time tables.

## Build

```sh
CARGO_TARGET_DIR=$PWD/target/perf CARGO_PROFILE_RELEASE_DEBUG=2 CARGO_PROFILE_RELEASE_SPLIT_DEBUGINFO=packed \
  cargo build -p hooke-bench --release --bin profile
```

`split-debuginfo=packed` writes `target/perf/release/profile.dSYM`; without it (the macOS
default, `unpacked`) there is no dSYM and xctrace names frames from the symbol table only.
Either way the summariser reports out-of-line functions (see below).

## Run

```sh
target/perf/release/profile <op> <workload> <size> [seconds=5] [--split]
```

`op`: `edit` (set_text + layout, alternating the edit's before/after text), `cold`
(invalidate_all + layout), `resize` (alternating viewports), `fork` (fork_edit_layout),
`candidates8` (eight forks on eight threads, no shared cache), `build` (HookeEngine::build,
dropped each iteration). `workload`/`size` are those of `workloads::make` (`text 10k`,
`random 1k`, `deep_chain 1k`, ...). Three untimed warm-up iterations, then it prints
`iterations` and `mean_us`; `--split` with `edit` also prints set_text and layout separately.
It runs on a 1 GiB-stack thread like the benches. The counting allocator is *not* installed,
so allocation shows at its real cost and the profile is hooke's own.

## Record and summarise

```sh
xcrun xctrace record --template 'Time Profiler' --output /tmp/edit.trace \
  --launch -- $PWD/target/perf/release/profile edit text 10k 3
python3 -I tools/profile/xctrace_summary.py /tmp/edit.trace --top 15 --short
python3 -I tools/profile/xctrace_summary.py /tmp/edit.trace --filter flex_base_sizes
```

Pass the binary as an absolute path. The script runs `xctrace export` itself (it also accepts
an exported time-profile XML), needs only the Python standard library plus `nm`, `otool` and
`c++filt` from the Xcode command-line tools, and must see the same binary that was recorded
(it checks the UUID).

Options: `--top N` (default 40), `--filter SUBSTR` (samples with a matching frame),
`--short` (drop generic arguments), `--inline` (see below).

## Reading the tables

- **Function view (default).** Frames in the profiled binary are mapped by address to the
  out-of-line function containing them, so inlined helpers (`Style::tag`, `Option::copied`,
  ...) are charged to their caller. Monomorphisations folded into one body by the linker
  (`Pass<_, Ahem>` and `Pass<_, AhemAtomic>`) print as `name (+1 folded)`; `--short` merges
  them.
- **`--inline`.** With a dSYM, xctrace names each frame after its innermost inlined callee and
  drops the function it was inlined into; `--inline` keeps those names. Self time is then
  finer grained, but inclusive time misses the containing functions, and `--filter` must
  name what xctrace shows.
- Rust names are demangled by `c++filt` (the toolchain here uses v0 mangling, which xctrace
  leaves mangled); `::h<hash>` suffixes are stripped.

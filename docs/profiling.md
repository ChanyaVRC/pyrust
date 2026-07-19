# Profiling PyRust

How to find where the interpreter spends time, so an optimization targets a
real hot spot instead of a guess. Pair this with [benchmark.md](./benchmark.md):
**profile to locate, benchmark to confirm.**

## The `profiling` build profile

`cargo build --profile profiling` (defined in the workspace `Cargo.toml`)
produces `target/profiling/pyrust`. It is codegen-identical to `release` — same
`lto` and `codegen-units = 1`, so the hot paths inline exactly as they ship —
but keeps full DWARF debug info and skips symbol stripping. That combination is
what a profiler needs: optimized code *and* readable function names / source
lines.

Never profile a `debug` build: the numbers are dominated by un-inlined helpers
and bounds checks that do not exist in the shipped binary.

## `tools/profile.sh`

A thin wrapper that builds the profiling binary and runs it under a profiler:

```bash
# perf (default) — sampling profiler, wall-clock weighted
tools/profile.sh crates/pyrust/tests/cases/performance/test_while_cmp.py

# callgrind — deterministic instruction counts (best under WSL, see below)
tools/profile.sh --tool callgrind crates/pyrust/tests/cases/performance/test_while_cmp.py

# flamegraph — interactive SVG (needs `cargo install flamegraph`)
tools/profile.sh --tool flamegraph crates/pyrust/tests/cases/performance/test_range_loop.py
```

Options: `--tool perf|callgrind|cachegrind|flamegraph`, `--no-build` (reuse the
existing binary), `--out DIR`, `--freq HZ`. Outputs land in
`target/profile-out/` by default. Run `tools/profile.sh --help` for the full
list.

## Installing the profilers

| Tool | Install | Notes |
|---|---|---|
| `perf` | `apt install linux-tools-common linux-tools-$(uname -r)` | Sampling; low overhead |
| `valgrind` (callgrind/cachegrind) | `apt install valgrind` | ~20–50× slower but deterministic |
| `flamegraph` | `cargo install flamegraph` | Wraps `perf`; also needs perf installed |

## WSL caveat

Under WSL2 `perf` is frequently missing or unprivileged, and even when it runs,
wall-clock sampling inherits the NTFS timing noise called out in `CLAUDE.md`.
Prefer **`--tool callgrind`** there: it counts executed instructions rather than
time, so it is both available in WSL and immune to that noise. Use its
instruction profile to *locate* a hot spot, then confirm the actual speedup with
`hyperfine` on `/tmp`-installed release binaries (per `CLAUDE.md`).

## Suggested workflow

1. Pick a representative workload — usually one of
   `crates/pyrust/tests/cases/performance/*.py`.
2. `tools/profile.sh --tool callgrind <script.py>` and read the top functions in
   the `callgrind_annotate` output (or open the `.out` file in `kcachegrind`).
3. Form a hypothesis, make a surgical change, and **confirm with a real bench**:
   build `release` binaries for master and your branch, install them to `/tmp`,
   and compare with `hyperfine -N --warmup 5`.
4. Watch the hot-path landmines listed in `CLAUDE.md` (the `vm.rs` `BinOp` /
   `Move` fast paths, `expr.rs::eval_binary` `Eq`/`Ne`, the const-fold passes).

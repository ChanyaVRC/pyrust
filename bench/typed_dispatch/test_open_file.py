# Typed-dispatch microbench: `open(path, mode)` (#399).
#
# `open` is the first single-body typed builtin (#395) — two args, second
# has a default.  The dispatch prelude validates kwargs, checks arity, and
# binds two `PyStr` locals via `try_from_value`.  Body cost (file I/O) is
# expected to dominate, so this bench mostly verifies that the typed
# prelude doesn't add pathological per-call overhead.
#
# N is much smaller than the other microbenches because each call performs
# a real `open`/`close` syscall round-trip — keep total wall-clock under a
# second.  Use `loop_noop_open.py` as the loop-overhead baseline.
import os
path = "/tmp/pyrust_microbench_open.txt"
with open(path, "w") as f:
    f.write("x")

N = 10_000
for _ in range(N):
    f = open(path, "r")
    f.close()

os.remove(path)

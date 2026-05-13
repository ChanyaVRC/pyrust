# Loop-overhead baseline at N=10_000 for `test_open_file.py` (#399).
#
# `open` is body-bound (real file I/O), so its bench uses 100x fewer
# iterations than the call-only benches.  Match that here so the
# per-call subtraction stays correct.
import os
path = "/tmp/pyrust_microbench_open.txt"
with open(path, "w") as f:
    f.write("x")

N = 10_000
for _ in range(N):
    pass

os.remove(path)

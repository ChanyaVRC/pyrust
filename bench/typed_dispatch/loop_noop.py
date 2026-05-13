# Loop-overhead baseline for the typed-dispatch microbench harness (#399).
#
# The harness times each `bench/typed_dispatch/*.py` and subtracts this
# script's wall-clock from the per-call totals to back out the cost of
# the surrounding `for _ in range(N): …` machinery.  Keep N identical to
# every other script in the same directory so the subtraction is sound.
N = 1_000_000
for _ in range(N):
    pass

# Issue #439: ForCount* opcodes used wrapping_add, so range() over near-i64::MAX
# (or near-i64::MIN) bounds wrapped past `stop` and looped forever.
#
# The fix is in the VM's ForCountReg / ForCountConst / ForCountConstInline
# handlers: use checked_add, and exit the loop on overflow.

# --- The exact #439 repro: step=2, start = i64::MAX - 3, stop = i64::MAX ---

def w():
    cnt = 0
    for i in range(9223372036854775804, 9223372036854775807, 2):
        cnt += 1
        if cnt > 10:    # bounds runtime if regression hits
            break
    return cnt

print("repro-439", w())   # 2

# --- Near i64::MAX, step=1 ---

count = 0
last = None
for i in range(2**62, 2**62 + 5):
    count += 1
    last = i
print("near-max-pos-count", count)        # 5
print("near-max-pos-last", last)          # 2**62 + 4

# --- Counter actually reaches i64::MAX, then would overflow on +1 ---

count = 0
last = None
for i in range(9223372036854775800, 9223372036854775807):
    count += 1
    last = i
print("at-max-count", count)              # 7
print("at-max-last", last)                # 9223372036854775806

# --- Near i64::MIN, negative step ---

count = 0
last = None
for i in range(-(2**62), -(2**62) - 5, -1):
    count += 1
    last = i
print("near-min-neg-count", count)        # 5
print("near-min-neg-last", last)          # -(2**62) - 4

# --- Ordinary ranges: fast path must remain correct ---

print("range-10", sum(range(10)))         # 45
print("range-neg10-10", sum(range(-10, 10)))  # -10
print("range-step2", sum(range(0, 10, 2)))    # 20

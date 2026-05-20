# Parity fixture for pass_const_reg_prop handling BinOpInPlace with constant RHS,
# and pass_dead_store_elim removing zero-global-read temps in loops.

# --- BinOpInPlace with i16-range constant (should become BinOpImm) ---

def aug_assign_small_const():
    result = 0
    for _ in range(5):
        result += 1
    return result

print(aug_assign_small_const())  # 5

# --- BinOpInPlace with a constant expression on RHS (folded to a single const) ---

def aug_assign_folded_const():
    total = 0
    step = 3 + 4  # folded to 7 by pass_const_fold
    for _ in range(3):
        total += step
    return total

print(aug_assign_folded_const())  # 21

# --- Multiple augmented assignments in a tight loop ---

def multi_aug():
    a = 0
    b = 0
    for _ in range(4):
        a += 10
        b += 3
    return a, b

print(multi_aug())  # (40, 12)

# --- Large constant that does not fit in i16 ---
# When lhs is a temp, BinOpConst is still emitted; when lhs is a named local
# with a potential __iadd__, the instruction is left as BinOpInPlace.

def large_const_temp():
    # The RHS expression produces a const > i16::MAX; lhs is a temp produced
    # by a prior BinOp, so the optimizer can safely use BinOpConst.
    x = 0
    step = 40000  # > 32767
    for _ in range(2):
        x += step
    return x

print(large_const_temp())  # 80000

# --- Semantics of augmented assignment on user-defined types ---
# The optimizer must NOT change semantics for user objects with __iadd__.

class Counter:
    def __init__(self, n):
        self.n = n
    def __iadd__(self, other):
        self.n += other * 2  # doubles the increment
        return self
    def __repr__(self):
        return f"Counter({self.n})"

def user_iadd():
    c = Counter(0)
    c += 5  # should call __iadd__, not skip it
    return c

print(user_iadd())  # Counter(10)

# --- Dead LoadConst inside a loop (pass_dead_store_elim global read count) ---
# A temp register that is assigned but never used should be removed even when
# there is a loop back-edge after the assignment.

def dead_temp_in_loop():
    result = 0
    for i in range(5):
        result += i
        # _unused_temp would be compiled to a LoadConst that is never read.
        # The optimizer should remove it.
    return result

print(dead_temp_in_loop())  # 10

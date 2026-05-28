# Parity fixture for issue #1526:
# `len` must NOT be treated as pure by the optimizer — the `PyInstance` arm
# dispatches user `__len__` via `invoke_class_method`, which can run
# arbitrary code with observable side effects.

# --- Side-effecting __len__: both calls must fire ---

class SideEffect:
    def __len__(self):
        print("side effect")
        return 1

c = SideEffect()
a = len(c)
b = len(c)
print(a, b)  # side effect / side effect / 1 1

# --- Dead-result len() call: __len__ must still be invoked ---

class Counting:
    count = 0
    def __len__(self):
        Counting.count += 1
        return 0

obj = Counting()
len(obj)   # result unused — must NOT be DCE'd
len(obj)   # result unused — must NOT be DCE'd
print(Counting.count)  # 2

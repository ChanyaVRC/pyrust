# Name-resolution path coverage for the env-only storage model.
#
# Issue #452 removed the dead `fastlocals` / `FunctionLocals` field from
# `Environment`.  These tests exercise every active name-lookup path to
# confirm correct behaviour:
#
#   1. Module-scope names (env.values HashMap)
#   2. Local names inside functions (env.values HashMap via env_assign_local)
#   3. global declaration — reads/writes the module env
#   4. nonlocal declaration — writes the enclosing function's env
#   5. Cell vars (names captured by inner closures)
#   6. Unbound local raises the correct error

# --- 1. Module-scope read/write ---
x = 10
assert x == 10, f"module-scope read: expected 10, got {x}"
print("module-scope:", x)

# --- 2. Function-local names ---
def locals_test():
    a = 1
    b = 2
    return a + b

assert locals_test() == 3
print("function locals:", locals_test())

# --- 3. global declaration ---
g = 0

def write_global():
    global g
    g = 99

write_global()
assert g == 99, f"global write: expected 99, got {g}"
print("global after write:", g)

def read_global():
    global g
    return g

assert read_global() == 99
print("global read:", read_global())

# --- 4. nonlocal declaration ---
def outer_nl():
    n = 0
    def inc():
        nonlocal n
        n += 1
    inc()
    inc()
    return n

assert outer_nl() == 2, f"nonlocal: expected 2, got {outer_nl()}"
print("nonlocal:", outer_nl())

# --- 5. Cell vars (closure capture) ---
def make_counter(start):
    count = start
    def step():
        nonlocal count
        count += 1
        return count
    return step

c = make_counter(10)
assert c() == 11
assert c() == 12
print("closure cell:", c())  # 13

# --- 6. Unbound local ---
def unbound_test():
    try:
        return y
    except Exception as e:
        return str(e)
    y = 1  # dead assignment that makes y local

msg = unbound_test()
assert "y" in msg or "local" in msg, f"unexpected error: {msg}"
print("unbound local error:", msg)

# --- 7. Nested scope: three levels ---
def level1():
    v = "deep"
    def level2():
        def level3():
            return v
        return level3()
    return level2()

assert level1() == "deep"
print("nested closure:", level1())

# --- 8. Multiple assignment in single function ---
def multi_assign():
    x = 1
    y = x + 1
    z = x + y
    return z

assert multi_assign() == 3
print("multi-assign:", multi_assign())

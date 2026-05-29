# Parity fixture for __del__ finalizer called on `del x` — issue #1212.
#
# CPython calls __del__ when the last Python-visible reference is dropped
# via `del x`.  pyrust implements the same for explicit `del` statements.

# ── Module scope: del f calls __del__ when f is the only binding ─────────────

class Module1:
    def __del__(self):
        print("module1: cleaned up")

f = Module1()
del f
print("module1: after del")

# ── Module scope: del f does NOT call __del__ when g still holds a ref ────────

class Module2:
    def __del__(self):
        print("module2: cleaned up")

g = Module2()
h = g
del g          # h still holds a ref — __del__ must NOT fire here
print("module2: after del g")
del h          # last ref gone — __del__ fires
print("module2: after del h")

# ── Module scope: __del__ receives self ──────────────────────────────────────

class Module3:
    def __init__(self, tag):
        self.tag = tag
    def __del__(self):
        print(f"module3: cleaned up {self.tag}")

b = Module3("alpha")
del b
print("module3: after del")

# ── Function scope: del x calls __del__ when x is the only binding ───────────

def fn1():
    class Fn1Inner:
        def __del__(self):
            print("fn1: cleaned up")
    x = Fn1Inner()
    del x
    print("fn1: after del x")

fn1()

# ── Function scope: del x does NOT fire when y still holds a ref ─────────────

def fn2():
    class Fn2Inner:
        def __del__(self):
            print("fn2: cleaned up")
    x = Fn2Inner()
    y = x
    del x          # y still holds ref — must NOT fire
    print("fn2: after del x")
    del y          # last ref — fires
    print("fn2: after del y")

fn2()

# ── Class without __del__ is unaffected ─────────────────────────────────────

class NoDel:
    pass

nd = NoDel()
del nd
print("no_del: after del")

# ── __del__ on a subclass is found via MRO ───────────────────────────────────

class Base:
    def __del__(self):
        print("mro: base __del__")

class Child(Base):
    pass

c = Child()
del c
print("mro: after del c")

# Parity test: generator.close() / generator.throw().

# --- close() runs finally ---
def gen():
    try:
        yield 1
        yield 2
    finally:
        print("gen-finally")

g = gen()
print(next(g))
g.close()
print("after-close")

# --- close() on a generator that ignores GeneratorExit → RuntimeError ---
def stubborn():
    try:
        yield 1
    except GeneratorExit:
        yield 2

g = stubborn()
print(next(g))
try:
    g.close()
    print("ignored-close", "FAIL")
except RuntimeError:
    print("ignored-close", "RuntimeError")

# --- close() on a fresh / exhausted generator is a no-op ---
def short():
    yield 1

f = short()
f.close()
print("close-fresh OK")

s = short()
print(next(s))
try:
    next(s)
except StopIteration:
    pass
s.close()
print("close-exhausted OK")

# --- close() — generator catches GeneratorExit and returns ---
def graceful():
    try:
        yield 1
    except GeneratorExit:
        print("graceful-exit")
        return

g = graceful()
print(next(g))
g.close()
print("after-graceful")

# --- throw() — exception caught inside the generator, yields again ---
def gen2():
    try:
        yield 1
    except ValueError as e:
        yield "caught:" + str(e)

g2 = gen2()
print(next(g2))
print(g2.throw(ValueError("hi")))

# --- throw() — uncaught exception propagates out ---
def gen3():
    yield 1

g3 = gen3()
print(next(g3))
try:
    g3.throw(ValueError("propagate"))
    print("uncaught-throw", "FAIL")
except ValueError as e:
    print("uncaught-throw", "ValueError:" + str(e))

# --- throw() — handler swallows it, generator returns → StopIteration ---
def gen4():
    try:
        yield 1
    except ValueError:
        return

g4 = gen4()
print(next(g4))
try:
    g4.throw(ValueError("done"))
    print("throw-return", "FAIL")
except StopIteration:
    print("throw-return", "StopIteration")

# --- throw() — finally runs and re-raises a different exception ---
def gen5():
    try:
        yield 1
    finally:
        print("gen5-finally")

g5 = gen5()
print(next(g5))
try:
    g5.throw(ValueError("v"))
    print("throw-finally", "FAIL")
except ValueError as e:
    print("throw-finally", "ValueError:" + str(e))

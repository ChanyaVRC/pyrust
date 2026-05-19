# Parity fixture for generator.send() — issue #514.
# Covers: basic send, multi-round send, send(None) as next(), unstarted
# generator TypeError, and done generator StopIteration.

# Basic: sent value is delivered as the yield expression result.
def gen():
    x = yield 1
    print(x)

g = gen()
next(g)
try:
    g.send(42)
except StopIteration:
    pass

# Echo loop: repeated sends.
def echo():
    while True:
        received = yield
        print(received)

g2 = echo()
next(g2)
g2.send("hello")
g2.send(42)
g2.send(True)
g2.close()

# Accumulator: yield total, receive increment.
def accumulator():
    total = 0
    while True:
        n = yield total
        total += n

a = accumulator()
print(next(a))       # 0
print(a.send(10))    # 10
print(a.send(5))     # 15
print(a.send(100))   # 115

# send(None) on an unstarted generator is equivalent to next().
def gen_none():
    x = yield 1
    print("after first yield, x =", x)

gn = gen_none()
r = gn.send(None)   # starts the generator
print("first yield =", r)

# send(non-None) on a just-started generator raises TypeError.
def gen_start():
    yield 1

gs = gen_start()
try:
    gs.send(99)
    print("ERROR: expected TypeError")
except TypeError as e:
    print("TypeError:", e)

# send() with wrong arity raises TypeError with the correct message.
def gen_arity():
    yield 1

ga = gen_arity()
try:
    ga.send(1, 2)
    print("ERROR: expected TypeError")
except TypeError as e:
    print("TypeError wrong arity:", e)

# send on a done generator raises StopIteration.
def gen_done():
    yield 1

gd = gen_done()
next(gd)
try:
    next(gd)
except StopIteration:
    pass
try:
    gd.send(0)
    print("ERROR: expected StopIteration")
except StopIteration:
    print("StopIteration on done generator: ok")
